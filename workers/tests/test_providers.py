"""The API review through other providers and the llm-connect worker paths, against local mocks.

No real API is called. The connect paths use a stub Node script in place of workers/connect/run.mjs
(which has its own tests in workers/connect/test/): here only the Python side is under test.

    workers/.venv/bin/python -m unittest discover -s workers/tests
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import wave
from pathlib import Path
from unittest import mock

from PIL import Image

WORKERS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(WORKERS))

from mangaw import openrouter, review  # noqa: E402
from test_openrouter import KEY, Case, Mock, as_json, error, media  # noqa: E402

PAGE = {
    "page": "0001.png",
    "width": 100,
    "height": 100,
    "panels": [],
    "characters": [],
    "detector": "vision",
    "texts": [
        {"id": "p_t0", "bbox": [0, 0, 10, 10], "panel": 0, "text": "H3LLO", "speaker": None, "essential": True},
        {"id": "p_t1", "bbox": [20, 20, 30, 30], "panel": 0, "text": "BOOM", "speaker": None, "essential": True},
    ],
}
GOOD = {
    "texts": [
        {"id": "p_t1", "text": "Boom", "speaker": None, "essential": False},
        {"id": "p_t0", "text": "Hello", "speaker": "Ana", "essential": True},
    ]
}


def reply(content, finish: str = "stop", **message) -> tuple:
    return as_json({"choices": [{"finish_reason": finish, "message": {"content": content, **message}}], "usage": {"prompt_tokens": 11, "completion_tokens": 7}})


class ParseItems(unittest.TestCase):
    def test_plain_json(self) -> None:
        self.assertEqual(review.parse_items(json.dumps(GOOD)), GOOD["texts"])

    def test_code_fence_and_prose(self) -> None:
        wrapped = f"Here is the review:\n```json\n{json.dumps(GOOD)}\n```\nDone."
        self.assertEqual(review.parse_items(wrapped), GOOD["texts"])

    def test_invalid_shapes_are_refused(self) -> None:
        cases = [
            "no json at all",
            "{not json",
            json.dumps({"other": []}),
            json.dumps([1, 2]),
            json.dumps({"texts": [{"id": 1, "text": "a", "speaker": None, "essential": True}]}),
            json.dumps({"texts": [{"id": "a", "text": "a", "essential": True}]} | {"x": 1}).replace('"text": "a"', '"text": 3'),
            json.dumps({"texts": [{"id": "a", "text": "a", "speaker": 3, "essential": True}]}),
            json.dumps({"texts": [{"id": "a", "text": "a", "speaker": None, "essential": "yes"}]}),
        ]
        for text in cases:
            with self.assertRaises(ValueError, msg=text):
                review.parse_items(text)

    def test_content_parts_are_joined(self) -> None:
        self.assertEqual(review.message_text([{"type": "text", "text": "ab"}, {"type": "text", "text": "c"}]), "abc")
        self.assertEqual(review.message_text(None), "")


class ChatReview(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.image = Path(tmp.name) / "page.png"
        Image.new("RGB", (3136, 1000), "white").save(self.image)

    def reviewer(self, routes: dict, provider: str = "openrouter", env: str = "OPENROUTER_API_KEY") -> tuple[review.ChatReviewer, Mock]:
        server = Mock(routes)
        self.addCleanup(server.close)
        patched = mock.patch.dict(os.environ, {env: KEY, openrouter.BASE_URL_ENV: server.url, "no_proxy": "*", "NO_PROXY": "*"})
        patched.start()
        self.addCleanup(patched.stop)
        return review.ChatReviewer("en", True, ["Kit"], provider, "acme/vision"), server

    def test_request_contract_and_merge(self) -> None:
        reviewer, server = self.reviewer({("POST", "/chat/completions"): reply(json.dumps(GOOD))})
        result = reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertEqual(result["detector"], "api")
        self.assertEqual([(t["id"], t["text"], t["speaker"], t["essential"]) for t in result["texts"]], [("p_t1", "Boom", None, False), ("p_t0", "Hello", "Ana", True)])
        self.assertEqual(reviewer.tokens, [11, 7])
        self.assertEqual(reviewer.known, ["Kit", "Ana"])
        method, path, headers, raw = server.requests[0]
        self.assertEqual(headers["Authorization"], f"Bearer {KEY}")
        body = json.loads(raw)
        self.assertEqual(body["model"], "acme/vision")
        self.assertEqual(body["response_format"]["json_schema"]["schema"], review.SCHEMA)
        self.assertNotIn("max_tokens", body)
        image_part, text_part = body["messages"][1]["content"]
        self.assertTrue(image_part["image_url"]["url"].startswith("data:image/jpeg;base64,"))
        request = json.loads(text_part["text"])
        self.assertEqual(request["reading_direction"], "right-to-left")
        self.assertEqual(request["known_speakers"], ["Kit"])
        # 3136 px is downscaled to 1568: boxes follow the scale sent with the image.
        self.assertEqual(request["boxes"][1]["bbox"], [10, 10, 15, 15])
        self.assertIn("JSON", body["messages"][0]["content"])

    def test_forgotten_bubble_keeps_ocr_and_invented_id_is_ignored(self) -> None:
        partial = {"texts": [{"id": "p_t0", "text": "Hello", "speaker": None, "essential": True}, {"id": "invented", "text": "x", "speaker": None, "essential": True}]}
        reviewer, _ = self.reviewer({("POST", "/chat/completions"): reply(json.dumps(partial))})
        result = reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertEqual([t["id"] for t in result["texts"]], ["p_t0", "p_t1"])
        self.assertEqual(result["texts"][1]["text"], "BOOM")

    def test_page_without_text_is_not_sent(self) -> None:
        reviewer, server = self.reviewer({})
        self.assertEqual(reviewer.review(str(self.image), {**PAGE, "texts": []})["detector"], "api")
        self.assertEqual(server.requests, [])

    def test_model_without_structured_output_is_asked_for_plain_json(self) -> None:
        refused = error(400, "response_format is not supported by this model")
        reviewer, server = self.reviewer({("POST", "/chat/completions"): [refused, reply(f"```json\n{json.dumps(GOOD)}\n```")]})
        result = reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertEqual(result["detector"], "api")
        bodies = server.bodies("POST", "/chat/completions")
        self.assertIn("response_format", bodies[0])
        self.assertNotIn("response_format", bodies[1])

    def test_unrelated_400_is_not_retried(self) -> None:
        reviewer, server = self.reviewer({("POST", "/chat/completions"): error(400, "bad image")})
        with self.assertRaises(openrouter.Error) as caught:
            reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertIn("bad image", str(caught.exception))
        self.assertEqual(server.count("POST", "/chat/completions"), 1)

    def test_unparseable_reply_is_a_clear_error(self) -> None:
        reviewer, _ = self.reviewer({("POST", "/chat/completions"): reply("I cannot read this page")})
        with self.assertRaises(openrouter.Error) as caught:
            reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertIn("acme/vision", str(caught.exception))
        self.assertIn("structured output", str(caught.exception))

    def test_truncated_or_refused_keeps_ocr(self) -> None:
        for response in (reply('{"texts": [', finish="length"), reply("", finish="content_filter"), reply("", refusal="no")):
            reviewer, _ = self.reviewer({("POST", "/chat/completions"): response})
            page = json.loads(json.dumps(PAGE))
            self.assertEqual(reviewer.review(str(self.image), page), page)

    def test_no_choices_is_a_clear_error(self) -> None:
        reviewer, _ = self.reviewer({("POST", "/chat/completions"): as_json({"choices": []})})
        with self.assertRaises(openrouter.Error):
            reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))

    def test_each_provider_reads_its_own_key(self) -> None:
        reviewer, server = self.reviewer({("POST", "/chat/completions"): reply(json.dumps(GOOD))}, "google", "GEMINI_API_KEY")
        reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertEqual(server.requests[0][2]["Authorization"], f"Bearer {KEY}")
        self.assertEqual(reviewer.client.key_env, "GEMINI_API_KEY")

    def test_missing_key_names_the_variable(self) -> None:
        environment = {k: v for k, v in os.environ.items() if k != "OPENAI_API_KEY"}
        with mock.patch.dict(os.environ, environment, clear=True), self.assertRaises(openrouter.Error) as caught:
            review.ChatReviewer("en", False, [], "openai", "m")
        self.assertIn("OPENAI_API_KEY is not set", str(caught.exception))

    def test_401_names_the_provider_variable_and_redacts(self) -> None:
        reviewer, _ = self.reviewer({("POST", "/chat/completions"): error(401, f"bad {KEY}")}, "openai", "OPENAI_API_KEY")
        with self.assertRaises(openrouter.Error) as caught:
            reviewer.review(str(self.image), json.loads(json.dumps(PAGE)))
        self.assertIn("(401)", str(caught.exception))
        self.assertIn("OPENAI_API_KEY", str(caught.exception))
        self.assertNotIn(KEY, str(caught.exception))


STUB = """
import { readFileSync, writeFileSync } from "node:fs";
const request = JSON.parse(readFileSync(0, "utf8"));
const log = process.env.STUB_LOG;
if (log) writeFileSync(log, JSON.stringify(request));
if (process.env.STUB_FAIL) { console.error("first line\\nprovider refused the panel"); process.exit(3); }
if (process.env.STUB_CRASH) { console.error("boom"); process.exit(1); }
writeFileSync(request.out, readFileSync(process.env.STUB_PAYLOAD));
console.error("warning: speed ignored");
console.log(JSON.stringify({ mediaType: "application/octet-stream", warnings: [] }));
"""


@unittest.skipUnless(shutil.which("node"), "node is not installed")
class ConnectWorkers(Case):
    def setUp(self) -> None:
        super().setUp()
        self.stub = self.dir / "stub.mjs"
        self.stub.write_text(STUB)
        self.log = self.dir / "request.json"

    def run_connect(self, module: str, job: dict, payload: bytes, **env: str) -> subprocess.CompletedProcess:
        payload_path = self.dir / "payload.bin"
        payload_path.write_bytes(payload)
        job = {"backend": "connect", "provider": "fal", "model": "acme/m", "script": str(self.stub), **job}
        job_path = self.dir / "job.json"
        job_path.write_text(json.dumps(job))
        environment = {**os.environ, "PYTHONPATH": str(WORKERS), "STUB_PAYLOAD": str(payload_path), "STUB_LOG": str(self.log), "FAL_KEY": KEY, **env}
        done = subprocess.run([sys.executable, "-m", f"mangaw.{module}", str(job_path)], cwd=WORKERS, env=environment, capture_output=True, text=True, timeout=60)
        self.assertNotIn(KEY, job_path.read_text() + done.stdout + done.stderr)
        return done

    def request(self) -> dict:
        return json.loads(self.log.read_text())

    def test_speech_is_converted_to_the_pipeline_wav(self) -> None:
        mp3 = media("-f", "lavfi", "-i", "sine=frequency=440:duration=0.5", "-c:a", "libmp3lame", "-f", "mp3")
        out = self.dir / "audio/line0.wav"
        results = self.dir / "durations.json"
        done = self.run_connect("tts", {"items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(out)}], "results": str(results)}, mp3)
        self.assertEqual(done.returncode, 0, done.stderr)
        with wave.open(str(out), "rb") as w:
            self.assertEqual((w.getframerate(), w.getnchannels(), w.getsampwidth()), (24000, 1, 2))
        self.assertAlmostEqual(json.loads(results.read_text())["line0"], 0.5, delta=0.1)
        request = self.request()
        self.assertEqual((request["kind"], request["provider"], request["model"]), ("speech", "fal", "acme/m"))
        self.assertEqual(request["item"], {"text": "Hi", "voice": "alloy"})
        self.assertNotIn("speed", request["item"])
        self.assertEqual(list((self.dir / "audio").iterdir()), [out])
        self.assertIn("warning: speed ignored", done.stderr)

    def test_wav_answers_are_accepted(self) -> None:
        pcm = media("-f", "lavfi", "-i", "sine=frequency=440:duration=0.5", "-ar", "24000", "-f", "wav")
        out = self.dir / "audio/line0.wav"
        done = self.run_connect("tts", {"items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 2.0, "out": str(out)}], "results": str(self.dir / "d.json")}, pcm)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertAlmostEqual(json.loads((self.dir / "d.json").read_text())["line0"], 0.25, delta=0.1)

    def test_a_reported_failure_is_one_line(self) -> None:
        out = self.dir / "audio/line0.wav"
        done = self.run_connect("tts", {"items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(out)}], "results": str(self.dir / "d.json")}, b"", STUB_FAIL="1")
        self.assert_one_line_error(done, "provider refused the panel")

    def test_a_crash_is_a_clear_error(self) -> None:
        out = self.dir / "audio/line0.wav"
        done = self.run_connect("tts", {"items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(out)}], "results": str(self.dir / "d.json")}, b"", STUB_CRASH="1")
        self.assert_one_line_error(done, "failed (1)", "boom")

    def test_unreadable_audio_is_refused(self) -> None:
        out = self.dir / "audio/line0.wav"
        done = self.run_connect("tts", {"items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(out)}], "results": str(self.dir / "d.json")}, b"\0" * 64)
        self.assert_one_line_error(done, "could not be converted")
        self.assertFalse(out.exists())

    def test_panel_is_written_as_png(self) -> None:
        import io

        buffer = io.BytesIO()
        Image.new("RGB", (64, 48), "red").save(buffer, "JPEG")
        out = self.dir / "panels/abc.png"
        item = {"id": "p1", "prompt": "two heroes", "refs": ["/tmp/ref.png"], "width": 1360, "height": 768, "seed": 7, "steps": 4, "out": str(out)}
        done = self.run_connect("draw_connect", {"items": [item]}, buffer.getvalue())
        self.assertEqual(done.returncode, 0, done.stderr)
        with Image.open(out) as image:
            self.assertEqual(image.format, "PNG")
        request = self.request()
        self.assertEqual(request["kind"], "image")
        self.assertEqual(request["item"], {k: item[k] for k in ("id", "prompt", "refs", "width", "height", "seed")})
        self.assertEqual(sorted(p.name for p in (self.dir / "panels").iterdir()), ["abc.png"])

    def test_unreadable_image_is_refused(self) -> None:
        out = self.dir / "panels/abc.png"
        done = self.run_connect("draw_connect", {"items": [{"id": "p1", "prompt": "a", "refs": [], "width": 8, "height": 8, "seed": 1, "steps": 1, "out": str(out)}]}, b"not an image")
        self.assert_one_line_error(done, "cannot be read")

    def test_video_is_fitted_and_polling_settings_are_passed(self) -> None:
        image = self.dir / "panel.png"
        Image.new("RGB", (64, 36), "white").save(image)
        source = self.dir / "source.mp4"
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=12", "-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(source)], check=True)
        out = self.dir / "clips/abc.mp4"
        item = {"id": "p1", "image": str(image), "prompt": "wind", "width": 640, "height": 352, "frames": 49, "steps": None, "seed": 42, "out": str(out)}
        done = self.run_connect("animate", {"poll_interval_s": 1, "poll_timeout_s": 9, "items": [item]}, source.read_bytes())
        self.assertEqual(done.returncode, 0, done.stderr)
        probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=codec_name,width,height", "-of", "json", str(out)], check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(probe.stdout)["streams"], [{"codec_name": "h264", "width": 640, "height": 352}])
        self.assertEqual(sorted(p.name for p in (self.dir / "clips").iterdir()), ["abc.mp4"])
        request = self.request()
        self.assertEqual((request["kind"], request["poll_interval_s"], request["poll_timeout_s"]), ("video", 1, 9))
        self.assertNotIn("steps", request["item"])

    @unittest.skipUnless((WORKERS / "connect/node_modules/llm-connect").exists(), "the connect worker is not installed (npm ci)")
    def test_real_worker_end_to_end_against_a_mock(self) -> None:
        mp3 = media("-f", "lavfi", "-i", "sine=frequency=440:duration=0.5", "-c:a", "libmp3lame", "-f", "mp3")
        server = self.mock({("POST", "/v1/audio/speech"): (200, "audio/mpeg", mp3, {})})
        job = {"backend": "connect", "provider": "openai", "model": "tts-1", "script": str(WORKERS / "connect/run.mjs"), "results": str(self.dir / "d.json"),
               "items": [{"id": "line0", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(self.dir / "audio/line0.wav")}]}
        job_path = self.dir / "job.json"
        job_path.write_text(json.dumps(job))
        environment = {**os.environ, "PYTHONPATH": str(WORKERS), "OPENAI_API_KEY": KEY, "MANGA_CONNECT_BASE_URL": f"{server.url}/v1", "NO_PROXY": "*", "no_proxy": "*"}
        done = subprocess.run([sys.executable, "-m", "mangaw.tts", str(job_path)], cwd=WORKERS, env=environment, capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertNotIn(KEY, done.stdout + done.stderr + job_path.read_text())
        self.assertEqual({k.lower(): v for k, v in server.requests[0][2].items()}["authorization"], f"Bearer {KEY}")
        self.assertAlmostEqual(json.loads((self.dir / "d.json").read_text())["line0"], 0.5, delta=0.1)

    def test_failed_item_keeps_earlier_durations(self) -> None:
        mp3 = media("-f", "lavfi", "-i", "sine=frequency=440:duration=0.5", "-c:a", "libmp3lame", "-f", "mp3")
        items = [{"id": f"line{i}", "text": "Hi", "voice": "alloy", "speed": 1.0, "out": str(self.dir / f"audio/line{i}.wav")} for i in range(2)]
        results = self.dir / "durations.json"
        calls = self.dir / "calls"
        fail_second = """
import { existsSync, readFileSync, writeFileSync } from "node:fs";
const request = JSON.parse(readFileSync(0, "utf8"));
const seen = existsSync(process.env.STUB_CALLS);
writeFileSync(process.env.STUB_CALLS, "1");
if (seen) { console.error("out of credits"); process.exit(3); }
writeFileSync(request.out, readFileSync(process.env.STUB_PAYLOAD));
console.log("{}");
"""
        self.stub.write_text(fail_second)
        done = self.run_connect("tts", {"items": items, "results": str(results)}, mp3, STUB_CALLS=str(calls))
        self.assert_one_line_error(done, "out of credits")
        self.assertEqual(list(json.loads(results.read_text())), ["line0"])


if __name__ == "__main__":
    unittest.main()
