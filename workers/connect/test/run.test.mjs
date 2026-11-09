// run.mjs against a local mock server: nothing here calls a real provider or uses a real key.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, test } from "node:test";

import { baseUrlOverride, BASE_URL_ENV, EXIT_REPORTED, pickDuration, pickRatio, pickResolution, PROVIDERS, referenceProblem, videoParams } from "../run.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const RUN = path.join(HERE, "..", "run.mjs");
const KEY = "test-key-not-real-1234567890";
const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==", "base64");
const MP3 = Buffer.from([0xff, 0xfb, 0x90, 0x44, 0, 0, 0, 0]);

let dir;
let server;
let requests;
let routes;
let base;

function json(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

beforeEach(async () => {
  dir = mkdtempSync(path.join(tmpdir(), "manga-connect-"));
  requests = [];
  routes = new Map();
  server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    requests.push({ method: req.method, url: req.url, headers: req.headers, body });
    const route = routes.get(`${req.method} ${req.url}`) ?? routes.get(`${req.method} *`);
    if (!route) return json(res, 404, { error: { message: `no route ${req.url}` } });
    route(req, res, body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${server.address().port}/v1`;
});

afterEach(async () => {
  await new Promise((resolve) => server.close(resolve));
  rmSync(dir, { recursive: true, force: true });
});

function catalog(entries) {
  const file = path.join(dir, "catalog.json");
  writeFileSync(file, JSON.stringify(entries));
  return file;
}

function run(request, { env = {}, entries = [] } = {}) {
  const environment = { ...process.env, [BASE_URL_ENV]: base, MANGA_CONNECT_CATALOG: catalog(entries), NO_PROXY: "*", no_proxy: "*", ...env };
  for (const name of ["OPENAI_API_KEY", "FAL_KEY", "REPLICATE_API_TOKEN", "ELEVENLABS_API_KEY"]) if (!(name in env)) delete environment[name];
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [RUN], { env: environment, stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d));
    child.stderr.on("data", (d) => (stderr += d));
    child.on("error", reject);
    child.on("close", (code) => {
      assert.ok(!stdout.includes(KEY) && !stderr.includes(KEY), `the key leaked: ${stdout}${stderr}`);
      resolve({ code, stdout, stderr });
    });
    child.stdin.end(JSON.stringify(request));
  });
}

function assertReported(result, ...texts) {
  assert.equal(result.code, EXIT_REPORTED, result.stderr);
  const lines = result.stderr.trim().split("\n");
  assert.ok(!result.stderr.includes("    at "), result.stderr);
  for (const text of texts) assert.ok(lines.at(-1).includes(text), `${text} not in: ${lines.at(-1)}`);
}

const imageRequest = (provider, model, item = {}) => ({
  kind: "image",
  provider,
  model,
  out: path.join(dir, "panels", "x.src"),
  item: { id: "p1", prompt: "two heroes", refs: [], width: 1360, height: 768, seed: 7, ...item },
});

test("image: the prompt is sent, the bytes are written and the key stays in the header", async () => {
  routes.set("POST /v1/images/generations", (req, res) => json(res, 200, { created: 1, data: [{ b64_json: PNG.toString("base64") }] }));
  const entries = [{ provider: "openai", id: "gpt-image-x", operations: ["text-to-image"], aspectRatios: ["1:1", "16:9"], seed: true }];
  const result = await run(imageRequest("openai", "gpt-image-x"), { env: { OPENAI_API_KEY: KEY }, entries });
  assert.equal(result.code, 0, result.stderr);
  const out = path.join(dir, "panels", "x.src");
  assert.deepEqual(readFileSync(out), PNG);
  assert.equal(JSON.parse(result.stdout.trim()).mediaType, "image/png");
  assert.equal(requests[0].headers.authorization, `Bearer ${KEY}`);
  const body = JSON.parse(requests[0].body);
  assert.equal(body.prompt, "two heroes");
  assert.equal(body.model, "gpt-image-x");
  assert.ok(!existsSync(`${out}.part`));
});

test("image: fal gets the seed, the ratio and every reference", async () => {
  const origin = base.replace("/v1", "");
  routes.set("POST /v1/fal-ai/flux-2/edit", (req, res) => json(res, 200, { images: [{ url: `${origin}/img.png`, content_type: "image/png", width: 1, height: 1 }] }));
  routes.set("GET /img.png", (req, res) => {
    res.writeHead(200, { "content-type": "image/png" });
    res.end(PNG);
  });
  const ref = path.join(dir, "ref.png");
  writeFileSync(ref, PNG);
  const entries = [{ provider: "fal", id: "fal-ai/flux-2/edit", operations: ["image-edit"], aspectRatios: ["1:1", "16:9"], seed: true }];
  const result = await run(imageRequest("fal", "fal-ai/flux-2/edit", { refs: [ref, ref] }), { env: { FAL_KEY: KEY }, entries });
  assert.equal(result.code, 0, result.stderr);
  assert.equal(requests[0].headers.authorization, `Key ${KEY}`);
  const body = JSON.parse(requests[0].body);
  assert.equal(body.seed, 7);
  assert.equal(body.image_urls.length, 2);
  assert.ok(body.image_urls[0].startsWith("data:image/png;base64,"));
  assert.ok(body.image_size);
});

test("image: references go to the edit endpoint", async () => {
  routes.set("POST /v1/images/edits", (req, res) => json(res, 200, { created: 1, data: [{ b64_json: PNG.toString("base64") }] }));
  const ref = path.join(dir, "ref.png");
  writeFileSync(ref, PNG);
  const result = await run(imageRequest("openai", "gpt-image-x", { refs: [ref] }), { env: { OPENAI_API_KEY: KEY }, entries: [{ provider: "openai", id: "gpt-image-x", operations: ["text-to-image", "image-edit"] }] });
  assert.equal(result.code, 0, result.stderr);
  assert.equal(requests[0].url, "/v1/images/edits");
  assert.ok(requests[0].body.includes(PNG.subarray(0, 8)));
});

test("image: a model the catalog lists without edits is refused before billing", async () => {
  const ref = path.join(dir, "ref.png");
  writeFileSync(ref, PNG);
  const result = await run(imageRequest("openai", "t2i", { refs: [ref] }), { env: { OPENAI_API_KEY: KEY }, entries: [{ provider: "openai", id: "t2i", operations: ["text-to-image"] }] });
  assertReported(result, "does not accept reference images");
  assert.equal(requests.length, 0);
});

test("image: several references on a Replicate model that takes one are refused before billing", async () => {
  const ref = path.join(dir, "ref.png");
  writeFileSync(ref, PNG);
  const result = await run(imageRequest("replicate", "acme/edit", { refs: [ref, ref] }), { env: { REPLICATE_API_TOKEN: KEY }, entries: [{ provider: "replicate", id: "acme/edit", operations: ["image-edit"] }] });
  assertReported(result, "cannot take 2 reference images");
  assert.equal(requests.length, 0);
});

test("speech: audio is written as returned", async () => {
  routes.set("POST /v1/audio/speech", (req, res) => {
    res.writeHead(200, { "content-type": "audio/mpeg" });
    res.end(MP3);
  });
  const out = path.join(dir, "a.src");
  const result = await run({ kind: "speech", provider: "openai", model: "tts-1", out, item: { text: "Hi", voice: "alloy" } }, { env: { OPENAI_API_KEY: KEY } });
  assert.equal(result.code, 0, result.stderr);
  assert.deepEqual(readFileSync(out), MP3);
  const body = JSON.parse(requests[0].body);
  assert.deepEqual([body.input, body.voice, body.model], ["Hi", "alloy", "tts-1"]);
});

test("speech: ElevenLabs takes its key in its own header", async () => {
  routes.set("POST *", (req, res) => {
    res.writeHead(200, { "content-type": "audio/mpeg" });
    res.end(MP3);
  });
  const out = path.join(dir, "a.src");
  const result = await run({ kind: "speech", provider: "elevenlabs", model: "eleven_flash_v2", out, item: { text: "Hi", voice: "voice1" } }, { env: { ELEVENLABS_API_KEY: KEY } });
  assert.equal(result.code, 0, result.stderr);
  assert.equal(requests[0].headers["xi-api-key"], KEY);
  assert.ok(requests[0].url.includes("voice1"), requests[0].url);
});

const videoRequest = (over = {}) => ({
  kind: "video",
  provider: "replicate",
  model: "acme/wan",
  out: path.join(dir, "clip.src.mp4"),
  poll_interval_s: 0.01,
  poll_timeout_s: 5,
  item: { id: "p1", image: path.join(dir, "panel.png"), prompt: "wind", width: 640, height: 352, frames: 49, seed: 42 },
  ...over,
});
const predicted = (extra) => ({ id: "job1", status: "processing", output: null, urls: { get: `${base}/predictions/job1` }, ...extra });

test("video: start, poll, download, with only declared parameters", async () => {
  writeFileSync(path.join(dir, "panel.png"), PNG);
  const states = [predicted({}), predicted({ status: "succeeded", output: `${base.replace("/v1", "")}/file.mp4` })];
  routes.set("POST /v1/models/acme/wan/predictions", (req, res) => json(res, 201, predicted({ status: "starting" })));
  routes.set("GET /v1/predictions/job1", (req, res) => json(res, 200, states.length > 1 ? states.shift() : states[0]));
  routes.set("GET /file.mp4", (req, res) => {
    res.writeHead(200, { "content-type": "video/mp4" });
    res.end(Buffer.from("fake-mp4"));
  });
  const entries = [{ provider: "replicate", id: "acme/wan", operations: ["image-to-video"], durations: [4, 8], aspectRatios: ["16:9", "9:16"], resolutions: ["720p", "480p"], audio: true, seed: true }];
  const result = await run(videoRequest(), { env: { REPLICATE_API_TOKEN: KEY }, entries });
  assert.equal(result.code, 0, result.stderr);
  assert.equal(readFileSync(path.join(dir, "clip.src.mp4"), "utf8"), "fake-mp4");
  const input = JSON.parse(requests[0].body).input;
  assert.ok(input.image.startsWith("data:image/png;base64,"), input.image);
  assert.deepEqual([input.prompt, input.duration, input.aspect_ratio, input.resolution, input.seed], ["wind", 4, "16:9", "480p", 42]);
  assert.equal(input.generate_audio ?? input.audio ?? false, false);
  assert.equal(requests.filter((r) => r.url === "/v1/predictions/job1").length, 2);
  assert.ok(!("authorization" in requests.find((r) => r.url === "/file.mp4").headers));
});

test("video: a model unknown to the catalog still runs, with provider defaults", async () => {
  writeFileSync(path.join(dir, "panel.png"), PNG);
  routes.set("POST /v1/models/acme/wan/predictions", (req, res) => json(res, 201, predicted({ status: "succeeded", output: `${base.replace("/v1", "")}/file.mp4` })));
  routes.set("GET /v1/predictions/job1", (req, res) => json(res, 200, predicted({ status: "succeeded", output: `${base.replace("/v1", "")}/file.mp4` })));
  routes.set("GET /file.mp4", (req, res) => {
    res.writeHead(200, { "content-type": "video/mp4" });
    res.end(Buffer.from("x"));
  });
  const result = await run(videoRequest(), { env: { REPLICATE_API_TOKEN: KEY } });
  assert.equal(result.code, 0, result.stderr);
  assert.ok(result.stderr.includes("not in the replicate catalog"), result.stderr);
  const input = JSON.parse(requests[0].body).input;
  assert.equal(input.duration, undefined);
  assert.equal(input.aspect_ratio, undefined);
});

test("video: a text-only model is refused before billing", async () => {
  writeFileSync(path.join(dir, "panel.png"), PNG);
  const result = await run(videoRequest(), { env: { REPLICATE_API_TOKEN: KEY }, entries: [{ provider: "replicate", id: "acme/wan", operations: ["text-to-video"] }] });
  assertReported(result, "cannot start from an image");
  assert.equal(requests.length, 0);
});

test("video: a failed job is one line naming the provider message", async () => {
  writeFileSync(path.join(dir, "panel.png"), PNG);
  routes.set("POST /v1/models/acme/wan/predictions", (req, res) => json(res, 201, predicted({ status: "starting" })));
  routes.set("GET /v1/predictions/job1", (req, res) => json(res, 200, predicted({ status: "failed", error: "nsfw panel" })));
  assertReported(await run(videoRequest(), { env: { REPLICATE_API_TOKEN: KEY } }), "nsfw panel");
});

test("video: a job that never ends stops at the deadline and says it may be billed", async () => {
  writeFileSync(path.join(dir, "panel.png"), PNG);
  routes.set("POST /v1/models/acme/wan/predictions", (req, res) => json(res, 201, predicted({ status: "starting" })));
  routes.set("GET /v1/predictions/job1", (req, res) => json(res, 200, predicted({})));
  assertReported(await run(videoRequest({ poll_timeout_s: 0.05 }), { env: { REPLICATE_API_TOKEN: KEY } }), "still not finished", "billed");
});

test("errors: a kind a provider does not offer names the supported ones", async () => {
  assertReported(await run({ kind: "video", provider: "openai", model: "m", out: "/tmp/x", item: {} }), "fal, google, xai, replicate");
  assertReported(await run({ kind: "speech", provider: "replicate", model: "m", out: "/tmp/x", item: {} }), "fal, openai, google, elevenlabs");
  assertReported(await run({ kind: "image", provider: "elevenlabs", model: "m", out: "/tmp/x", item: {} }), "fal, openai, google, xai, replicate");
  assert.equal(requests.length, 0);
});

test("errors: a missing key names its variable", async () => {
  const result = await run({ kind: "speech", provider: "openai", model: "tts-1", out: "/tmp/x", item: { text: "a", voice: "b" } }, { env: { OPENAI_API_KEY: "" } });
  assertReported(result, "OPENAI_API_KEY is not set");
  assert.equal(requests.length, 0);
});

test("errors: a provider failure is one line and the key is scrubbed", async () => {
  routes.set("POST /v1/audio/speech", (req, res) => json(res, 400, { error: { message: `bad voice\nfor ${KEY}` } }));
  const result = await run({ kind: "speech", provider: "openai", model: "tts-1", out: "/tmp/x", item: { text: "a", voice: "b" } }, { env: { OPENAI_API_KEY: KEY } });
  assertReported(result, "bad voice");
  assert.ok(!existsSync("/tmp/x.part"));
});

test("base URL override is loopback only", () => {
  const saved = process.env[BASE_URL_ENV];
  try {
    delete process.env[BASE_URL_ENV];
    assert.equal(baseUrlOverride(), undefined);
    process.env[BASE_URL_ENV] = "http://127.0.0.1:9/v1/";
    assert.equal(baseUrlOverride(), "http://127.0.0.1:9/v1");
    process.env[BASE_URL_ENV] = "https://evil.example/v1";
    assert.throws(() => baseUrlOverride(), /loopback/);
  } finally {
    if (saved === undefined) delete process.env[BASE_URL_ENV];
    else process.env[BASE_URL_ENV] = saved;
  }
});

test("a non-loopback base URL stops the run before any request", async () => {
  const result = await run({ kind: "speech", provider: "openai", model: "m", out: "/tmp/x", item: {} }, { env: { [BASE_URL_ENV]: "https://evil.example/v1", OPENAI_API_KEY: KEY } });
  assertReported(result, "loopback");
});

test("parameter pickers", () => {
  assert.equal(pickDuration(2.04, [8, 4, 6]), 4);
  assert.equal(pickDuration(20, [8, 4]), 8);
  assert.equal(pickDuration(2, null), undefined);
  assert.equal(pickRatio(1360, 768, ["1:1", "16:9", "9:16", "auto"]), "16:9");
  assert.equal(pickRatio(1, 1, null), undefined);
  assert.equal(pickResolution(352, ["1080p", "480p", "720p"]), "480p");
  assert.equal(pickResolution(2000, ["480p", "1K"]), "1K");
  assert.equal(pickResolution(352, []), undefined);
  assert.deepEqual(videoParams(null, { frames: 49, width: 640, height: 352, seed: 1 }), {});
});

test("reference checks", () => {
  const settings = { provider: "fal", model: "fal-ai/x" };
  assert.equal(referenceProblem(settings, null, undefined, 0), null);
  assert.equal(referenceProblem(settings, null, undefined, 2), null);
  assert.match(referenceProblem(settings, null, false, 1), /does not accept/);
  assert.match(referenceProblem(settings, { operations: ["text-to-image"] }, undefined, 1), /does not accept/);
  assert.equal(referenceProblem(settings, { operations: [] }, undefined, 1), null);
  assert.equal(referenceProblem({ provider: "replicate", model: "black-forest-labs/flux-2-pro" }, null, undefined, 8), null);
  assert.match(referenceProblem({ provider: "replicate", model: "black-forest-labs/flux-2-pro" }, null, undefined, 9), /cannot take 9/);
  assert.match(referenceProblem({ provider: "replicate", model: "acme/x" }, null, undefined, 2), /cannot take 2/);
});

test("the Rust provider table matches what llm-connect exposes", () => {
  const backend = readFileSync(path.join(HERE, "..", "..", "..", "src", "backend.rs"), "utf8");
  const declared = {};
  for (const [, id, env, url, kinds] of backend.matchAll(/ConnectProvider \{ id: "(\w+)", env: "(\w+)", key_url: "([^"]+)", kinds: &\[([^\]]+)\] \}/g)) {
    declared[id] = { env, url, kinds: kinds.split(", ").map((k) => k.replace("Kind::", "").toLowerCase()) };
  }
  assert.equal(Object.keys(declared).length, 6);
  for (const [kind, providers] of Object.entries(PROVIDERS)) {
    for (const [id, info] of Object.entries(providers)) {
      assert.equal(declared[id]?.env, info.env, id);
      assert.equal(declared[id]?.url, info.keyUrl, id);
      assert.ok(declared[id].kinds.includes(kind), `${id} ${kind}`);
    }
    const expected = Object.keys(declared).filter((id) => declared[id].kinds.includes(kind)).sort();
    assert.deepEqual(Object.keys(providers).sort(), expected, kind);
  }
});
