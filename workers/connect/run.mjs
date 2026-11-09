// Generates one image, one speech line or one video clip through llm-connect.
//
// The request arrives on stdin: {kind, provider, model, out, item, poll_interval_s?, poll_timeout_s?}.
// The API key comes only from the environment variable llm-connect documents for the provider: it
// is never read from the request and never written anywhere. The raw bytes go to `out` and one
// JSON line {mediaType, warnings, costUsd?} goes to stdout. A failure prints one line on stderr
// and exits with EXIT_REPORTED, the code the callers treat as "already explained".

import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { generateImage } from "ai";
import { describeError, scrubSecret } from "llm-connect";
import { IMAGE_PROVIDER_INFO, loadImageModel } from "llm-connect/image";
import { generateSpeechAudio, SPEECH_PROVIDER_INFO } from "llm-connect/speech";
import { checkVideo, downloadVideo, startVideo, VIDEO_PROVIDER_INFO } from "llm-connect/video";
import { listImageModels, listVideoModels } from "llm-models";

export const EXIT_REPORTED = 3;
// Used only by the tests, and restricted to loopback so it can never send the key elsewhere.
export const BASE_URL_ENV = "MANGA_CONNECT_BASE_URL";
// Test only, honored only together with BASE_URL_ENV: a JSON array replacing the live catalog.
export const CATALOG_ENV = "MANGA_CONNECT_CATALOG";

const FPS = 24;
const CATALOG_TIMEOUT_MS = 20_000;
const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);
const TRANSIENT_CODES = new Set(["rate-limit", "overloaded", "server", "auth-or-network"]);
const IMAGE_TYPES = { ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp" };

// What manga exposes for each kind: the library's providers, minus the aggregators it reaches
// through its own --backend openrouter.
export const PROVIDERS = {
  image: pick(IMAGE_PROVIDER_INFO, ["fal", "openai", "google", "xai", "replicate"]),
  speech: pick(SPEECH_PROVIDER_INFO, ["fal", "openai", "google", "elevenlabs"]),
  video: pick(VIDEO_PROVIDER_INFO, ["fal", "google", "xai", "replicate"]),
};

function pick(info, names) {
  return Object.fromEntries(names.map((n) => [n, info[n]]));
}

class Reported extends Error {}

export function providerInfo(kind, provider) {
  const supported = PROVIDERS[kind];
  if (!supported) throw new Reported(`unknown generation kind "${kind}"`);
  const info = Object.hasOwn(supported, provider) ? supported[provider] : null;
  if (!info) throw new Reported(`${kind} is not available through ${provider}: use one of ${Object.keys(supported).join(", ")}`);
  return info;
}

export function baseUrlOverride() {
  const raw = (process.env[BASE_URL_ENV] ?? "").trim();
  if (!raw) return undefined;
  if (!LOOPBACK.has(new URL(raw).hostname)) throw new Reported(`${BASE_URL_ENV} is for tests only and must point to a loopback address`);
  return raw.replace(/\/+$/, "");
}

export function pickDuration(wanted, supported) {
  if (!supported?.length) return undefined;
  const covering = supported.filter((d) => d >= wanted);
  return covering.length ? Math.min(...covering) : Math.max(...supported);
}

function ratioValue(ratio) {
  const match = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(ratio);
  return match ? Number(match[1]) / Number(match[2]) : null;
}

export function pickRatio(width, height, supported) {
  const known = (supported ?? []).filter(ratioValue);
  if (!known.length) return undefined;
  const target = width / height;
  return known.reduce((best, r) => (Math.abs(ratioValue(r) - target) < Math.abs(ratioValue(best) - target) ? r : best));
}

function resolutionHeight(label) {
  const match = /^(\d+(?:\.\d+)?)(p|k|K)$/.exec(label);
  return match ? Number(match[1]) * (match[2] === "p" ? 1 : 1024) : null;
}

export function pickResolution(shortSide, supported) {
  const known = (supported ?? []).filter(resolutionHeight).sort((a, b) => resolutionHeight(a) - resolutionHeight(b));
  if (!known.length) return undefined;
  return known.find((r) => resolutionHeight(r) >= shortSide) ?? known.at(-1);
}

/** Only what the model declares is sent, so an unsupported value cannot be rejected or billed. */
export function videoParams(entry, item) {
  if (!entry) return {};
  const params = {
    // 4n+1 frames: the extra frame must not bill one more second (73 frames is a 3 s clip).
    durationS: pickDuration((item.frames - 1) / FPS, entry.durations),
    aspectRatio: pickRatio(item.width, item.height, entry.aspectRatios),
    resolution: pickResolution(Math.min(item.width, item.height), entry.resolutions),
    ...(entry.audio === true ? { audio: false } : {}), // The render discards clip audio: do not pay for it.
    ...(entry.seed === true ? { seed: item.seed } : {}),
  };
  return Object.fromEntries(Object.entries(params).filter(([, v]) => v !== undefined));
}

/** Why the references cannot be honored, or null. Ignored references would draw the wrong characters. */
export function referenceProblem({ provider, model }, entry, supportsFileInputs, count) {
  if (!count) return null;
  if (supportsFileInputs === false || (entry?.operations?.length && !entry.operations.includes("image-edit")))
    return `${model} does not accept reference images: pick an image-edit model (manga models search image), or remove the refs from story.json`;
  if (provider === "replicate" && count > (/^black-forest-labs\/flux-2-/.test(model) ? 8 : 1))
    return `${model} on Replicate cannot take ${count} reference images: use a flux-2 model (up to 8), or use fewer refs in that panel`;
  return null;
}

async function catalogModels(kind) {
  if (process.env[CATALOG_ENV] && process.env[BASE_URL_ENV]) return JSON.parse(await readFile(process.env[CATALOG_ENV], "utf8"));
  const list = kind === "video" ? listVideoModels : listImageModels;
  let timer;
  try {
    return await Promise.race([
      list(),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("timed out")), CATALOG_TIMEOUT_MS);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/** The model's capabilities in the free public catalog, or null when they cannot be known. */
export async function catalogEntry(kind, provider, model, warn = () => {}) {
  try {
    const entry = (await catalogModels(kind)).find((m) => m.provider === provider && m.id === model) ?? null;
    if (!entry) warn(`${model} is not in the ${provider} catalog: its capabilities cannot be checked, provider defaults apply`);
    return entry;
  } catch {
    warn(`the ${provider} model catalog could not be read: capabilities cannot be checked, provider defaults apply`);
    return null;
  }
}

function imageType(file) {
  return IMAGE_TYPES[path.extname(file).toLowerCase()] ?? "image/png";
}

async function runImage(settings, request, warn) {
  const { item } = request;
  const entry = await catalogEntry("image", settings.provider, settings.model, warn);
  const model = await loadImageModel(settings);
  const problem = referenceProblem(settings, entry, model.supportsFileInputs, item.refs.length);
  if (problem) throw new Reported(problem);
  const aspectRatio = entry ? pickRatio(item.width, item.height, entry.aspectRatios) : undefined;
  const images = await Promise.all(item.refs.map((r) => readFile(r)));
  const result = await generateImage({
    model,
    prompt: images.length ? { text: item.prompt, images } : item.prompt,
    n: 1,
    ...(aspectRatio ? { aspectRatio } : {}),
    ...(entry?.seed === true ? { seed: item.seed } : {}),
    // Without this flag fal only reads the first image.
    ...(settings.provider === "fal" && images.length > 1 ? { providerOptions: { fal: { useMultipleImages: true } } } : {}),
  });
  const [image] = result.images;
  if (!image) throw new Reported(`${settings.model} returned no image for ${item.id}: try another seed or model`);
  return { data: image.uint8Array, mediaType: image.mediaType, warnings: result.warnings.map(warningLine) };
}

function warningLine(warning) {
  return warning.message ?? warning.details ?? warning.type;
}

async function runSpeech(settings, request) {
  // No format is forced: providers answer in a container that describes itself (Gemini: WAV).
  const { audio, warnings } = await generateSpeechAudio(settings, { text: request.item.text, voice: request.item.voice });
  return { data: audio.data, mediaType: audio.mediaType, warnings };
}

async function waitForVideo(settings, job, request) {
  const interval = request.poll_interval_s * 1000;
  const deadline = Date.now() + request.poll_timeout_s * 1000;
  for (;;) {
    let status;
    try {
      status = await checkVideo(settings, job);
    } catch (error) {
      if (!TRANSIENT_CODES.has(error.code)) throw error;
      status = { status: "pending" };
    }
    if (status.status === "completed") return status;
    if (status.status === "error") throw new Reported(`${settings.model} video job failed: ${status.message}`);
    const left = deadline - Date.now();
    if (left <= 0) throw new Reported(`${settings.model} video job still not finished after ${request.poll_timeout_s} s: it may complete and be billed anyway, rerun later`);
    await new Promise((resolve) => setTimeout(resolve, Math.min(interval, left)));
  }
}

async function runVideo(settings, request, warn) {
  const { item } = request;
  const entry = await catalogEntry("video", settings.provider, settings.model, warn);
  if (entry?.operations?.length && !entry.operations.includes("image-to-video"))
    throw new Reported(`${settings.model} cannot start from an image: pick an image-to-video model, or render with --no-animate`);
  const data = await readFile(item.image);
  const job = await startVideo(settings, {
    prompt: item.prompt,
    image: { data, mediaType: imageType(item.image) },
    ...videoParams(entry, item),
  });
  const status = await waitForVideo(settings, job, request);
  const [video] = status.videos;
  if (!video) throw new Reported(`${settings.model} returned no video for ${item.id}`);
  const downloaded = await downloadVideo(video);
  return { data: downloaded.data, mediaType: downloaded.mediaType, warnings: job.warnings, costUsd: status.costUsd };
}

const RUNNERS = { image: runImage, speech: runSpeech, video: runVideo };

export async function generate(request, apiKey, warn = () => {}) {
  const settings = { provider: request.provider, apiKey, model: request.model };
  const baseUrl = baseUrlOverride();
  if (baseUrl) settings.baseUrl = baseUrl;
  const { data, mediaType, warnings = [], costUsd } = await RUNNERS[request.kind](settings, request, warn);
  await mkdir(path.dirname(request.out), { recursive: true });
  const tmp = `${request.out}.part`;
  await writeFile(tmp, data);
  await rename(tmp, request.out);
  return { mediaType, warnings, ...(costUsd !== undefined ? { costUsd } : {}) };
}

function oneLine(text, apiKey) {
  return scrubSecret(String(text), apiKey).split(/\s+/).join(" ").trim().slice(0, 400);
}

async function main() {
  let apiKey = "";
  const warn = (message) => process.stderr.write(`warning: ${oneLine(message, apiKey)}\n`);
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    const request = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const info = providerInfo(request.kind, request.provider);
    apiKey = (process.env[info.env] ?? "").trim();
    if (!apiKey) throw new Reported(`${info.env} is not set: create a key at ${info.keyUrl} and export it`);
    const result = await generate(request, apiKey, warn);
    result.warnings.forEach(warn);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    const message = error instanceof Reported ? error.message : describeError(error, apiKey).message || String(error?.message ?? error);
    process.stderr.write(`${oneLine(message, apiKey) || "the generation failed with no detail"}\n`);
    process.exitCode = EXIT_REPORTED;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
