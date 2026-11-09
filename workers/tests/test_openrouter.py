"""Runs the OpenRouter worker paths against a local mock server: the real API is never called.

    workers/.venv/bin/python -m unittest discover -s workers/tests
"""

from __future__ import annotations

import base64
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from PIL import Image

WORKERS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(WORKERS))

from mangaw import openrouter  # noqa: E402

KEY = "sk-or-test-secret"
EXIT_REPORTED = 3
JSON = "application/json"


def media(*args: str) -> bytes:
    return subprocess.run(["ffmpeg", "-v", "error", *args, "pipe:1"], check=True, capture_output=True).stdout


def setUpModule() -> None:
    global MP3, MP4, JPEG
    MP3 = media("-f", "lavfi", "-i", "sine=frequency=440:duration=0.5", "-c:a", "libmp3lame", "-f", "mp3")
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "source.mp4"
        subprocess.run(
            ["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=12", "-f", "lavfi", "-i", "sine=duration=1", "-t", "1",
             "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", str(path)],
            check=True,
        )
        MP4 = path.read_bytes()
    buffer = io.BytesIO()
    Image.new("RGB", (64, 48), "red").save(buffer, "JPEG")
    JPEG = buffer.getvalue()


def as_json(payload: dict, status: int = 200, headers: dict | None = None) -> tuple:
    return status, JSON, json.dumps(payload).encode(), headers or {}


def error(status: int, message: str, headers: dict | None = None) -> tuple:
    return as_json({"error": {"code": status, "message": message}}, status, headers)


class Mock:
    """Routes map (method, path) to a response, or to a list served one item at a time."""

    def __init__(self, routes: dict) -> None:
        self.routes = routes
        self.requests: list[tuple[str, str, dict, bytes]] = []
        mock = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args) -> None:
                pass

            def respond(self) -> None:
                body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                mock.requests.append((self.command, self.path, dict(self.headers), body))
                route = mock.routes.get((self.command, self.path), error(404, f"no route {self.path}"))
                if isinstance(route, list):
                    route = route.pop(0) if len(route) > 1 else route[0]
                status, content_type, payload, headers = route
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(payload)))
                for name, value in headers.items():
                    self.send_header(name, value)
                self.end_headers()
                self.wfile.write(payload)

            do_GET = do_POST = respond

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()

    def bodies(self, method: str, path: str) -> list[dict]:
        return [json.loads(r[3]) for r in self.requests if r[0] == method and r[1] == path]

    def count(self, method: str, path: str) -> int:
        return sum(1 for r in self.requests if r[0] == method and r[1] == path)


class Case(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def mock(self, routes: dict) -> Mock:
        server = Mock(routes)
        self.addCleanup(server.close)
        return server

    def run_worker(self, module: str, job: dict, server: Mock | None = None, key: str | None = KEY) -> subprocess.CompletedProcess:
        job_path = self.dir / "job.json"
        job_path.write_text(json.dumps(job))
        env = {k: v for k, v in os.environ.items() if k != openrouter.KEY_ENV}
        env.update(PYTHONPATH=str(WORKERS), no_proxy="*", NO_PROXY="*")
        if server:
            env[openrouter.BASE_URL_ENV] = server.url
        if key is not None:
            env[openrouter.KEY_ENV] = key
        done = subprocess.run([sys.executable, "-m", f"mangaw.{module}", str(job_path)], cwd=WORKERS, env=env, capture_output=True, text=True, timeout=60)
        self.assertNotIn(KEY, done.stdout + done.stderr)
        self.assertNotIn(KEY, job_path.read_text())
        return done

    def assert_one_line_error(self, done: subprocess.CompletedProcess, *expected: str) -> None:
        self.assertEqual(done.returncode, EXIT_REPORTED, done.stderr)
        lines = done.stderr.strip().splitlines()
        self.assertNotIn("Traceback", done.stderr)
        for text in expected:
            self.assertIn(text, lines[-1])


class Tts(Case):
    def job(self, *speeds: float) -> dict:
        items = [{"id": f"line{i}", "text": f"Hello {i}", "voice": "alloy", "speed": s, "out": str(self.dir / "audio" / f"line{i}.wav")} for i, s in enumerate(speeds)]
        return {"backend": "openrouter", "model": "acme/tts", "items": items, "results": str(self.dir / "durations.json")}

    def audio(self, *responses) -> dict:
        return {("POST", "/audio/speech"): list(responses)}

    def test_writes_pipeline_wav_with_real_duration(self) -> None:
        server = self.mock(self.audio((200, "audio/mpeg", MP3, {})))
        done = self.run_worker("tts", self.job(1.0), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        with wave.open(str(self.dir / "audio/line0.wav"), "rb") as w:
            self.assertEqual((w.getframerate(), w.getnchannels(), w.getsampwidth()), (24000, 1, 2))
            actual = w.getnframes() / w.getframerate()
        durations = json.loads((self.dir / "durations.json").read_text())
        self.assertAlmostEqual(durations["line0"], actual, places=3)
        self.assertAlmostEqual(actual, 0.5, delta=0.1)
        request = server.requests[0]
        self.assertEqual(request[2]["Authorization"], f"Bearer {KEY}")
        self.assertEqual(json.loads(request[3]), {"model": "acme/tts", "input": "Hello 0", "voice": "alloy", "response_format": "mp3"})

    def test_speed_is_applied_locally(self) -> None:
        server = self.mock(self.audio((200, "audio/mpeg", MP3, {})))
        self.assertEqual(self.run_worker("tts", self.job(2.0), server).returncode, 0)
        self.assertNotIn("speed", server.bodies("POST", "/audio/speech")[0])
        self.assertAlmostEqual(json.loads((self.dir / "durations.json").read_text())["line0"], 0.25, delta=0.1)

    def test_missing_key(self) -> None:
        server = self.mock(self.audio((200, "audio/mpeg", MP3, {})))
        self.assert_one_line_error(self.run_worker("tts", self.job(1.0), server, key=None), "OPENROUTER_API_KEY is not set")
        self.assertEqual(server.requests, [])

    def test_401(self) -> None:
        server = self.mock(self.audio(error(401, "bad key")))
        self.assert_one_line_error(self.run_worker("tts", self.job(1.0), server), "(401)", "OPENROUTER_API_KEY")

    def test_402_keeps_finished_lines(self) -> None:
        server = self.mock(self.audio((200, "audio/mpeg", MP3, {}), error(402, "no credits")))
        done = self.run_worker("tts", self.job(1.0, 1.0), server)
        self.assert_one_line_error(done, "(402)", "credits")
        self.assertEqual(list(json.loads((self.dir / "durations.json").read_text())), ["line0"])

    def test_error_with_200_status(self) -> None:
        server = self.mock(self.audio(as_json({"error": {"code": 502, "message": "provider exploded"}})))
        self.assert_one_line_error(self.run_worker("tts", self.job(1.0), server), "provider exploded")

    def test_error_detail_is_one_redacted_line(self) -> None:
        server = self.mock(self.audio(error(400, f"voice not found\nfor key {KEY}")))
        self.assert_one_line_error(self.run_worker("tts", self.job(1.0), server), "(400)", "voice not found for key ***")

    def test_pcm_response_is_refused(self) -> None:
        server = self.mock(self.audio((200, "audio/pcm", b"\0" * 2048, {})))
        self.assert_one_line_error(self.run_worker("tts", self.job(1.0), server), "audio/pcm", "PCM")
        self.assertFalse((self.dir / "audio/line0.wav").exists())


CAPS = {
    "id": "acme/video",
    "canonical_slug": "acme/video-1",
    "supported_durations": [8, 4, 6],
    "supported_resolutions": ["1080p", "480p", "720p"],
    "supported_aspect_ratios": ["16:9", "9:16", "1:1"],
    "supported_frame_images": ["first_frame", "last_frame"],
    "generate_audio": True,
    "seed": True,
}
SUBMITTED = as_json({"id": "job1", "polling_url": "http://ignored.example/poll", "status": "pending"}, 202)
PENDING = as_json({"id": "job1", "status": "pending"})
RUNNING = as_json({"id": "job1", "status": "in_progress"})
COMPLETED = as_json({"id": "job1", "status": "completed", "unsigned_urls": ["http://ignored.example/file.mp4"]})


class Video(Case):
    def job(self, timeout: float = 20) -> dict:
        image = self.dir / "panel.png"
        Image.new("RGB", (64, 36), "white").save(image)
        item = {"id": "p1", "image": str(image), "prompt": "wind", "width": 640, "height": 352, "frames": 49, "steps": None, "seed": 42, "out": str(self.dir / "clips/abc.mp4")}
        return {"backend": "openrouter", "model": "acme/video", "poll_interval_s": 0.01, "poll_timeout_s": timeout, "items": [item]}

    def routes(self, caps: dict = CAPS, submit=SUBMITTED, poll=None) -> dict:
        return {
            ("GET", "/videos/models"): as_json({"data": [caps]}),
            ("POST", "/videos"): submit,
            ("GET", "/videos/job1"): poll if poll is not None else [PENDING, RUNNING, COMPLETED],
            ("GET", "/videos/job1/content?index=0"): (200, "video/mp4", MP4, {}),
        }

    def probe(self, path: Path) -> list[dict]:
        out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height", "-of", "json", str(path)], check=True, capture_output=True, text=True)
        return json.loads(out.stdout)["streams"]

    def test_submit_poll_download(self) -> None:
        server = self.mock(self.routes())
        done = self.run_worker("animate", self.job(), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.probe(self.dir / "clips/abc.mp4"), [{"codec_name": "h264", "codec_type": "video", "width": 640, "height": 352}])
        self.assertEqual(sorted(p.name for p in (self.dir / "clips").iterdir()), ["abc.mp4"])
        body = server.bodies("POST", "/videos")[0]
        frame = body.pop("frame_images")[0]
        self.assertEqual((frame["type"], frame["frame_type"]), ("image_url", "first_frame"))
        self.assertTrue(frame["image_url"]["url"].startswith("data:image/png;base64,"))
        self.assertEqual(body, {"model": "acme/video", "prompt": "wind", "duration": 4, "aspect_ratio": "16:9", "resolution": "480p", "generate_audio": False, "seed": 42})
        self.assertEqual(server.count("GET", "/videos/job1"), 3)

    def test_only_declared_parameters_are_sent(self) -> None:
        caps = {"id": "acme/video", "supported_durations": None, "supported_resolutions": None, "supported_aspect_ratios": None, "supported_frame_images": None, "generate_audio": None, "seed": None}
        server = self.mock(self.routes(caps=caps))
        self.assertEqual(self.run_worker("animate", self.job(), server).returncode, 0)
        self.assertEqual(sorted(server.bodies("POST", "/videos")[0]), ["frame_images", "model", "prompt"])

    def test_failed_job_reports_its_error(self) -> None:
        failed = as_json({"id": "job1", "status": "failed", "error": "content policy violation"})
        server = self.mock(self.routes(poll=[PENDING, failed]))
        self.assert_one_line_error(self.run_worker("animate", self.job(), server), "job1", "failed", "content policy violation")
        self.assertFalse((self.dir / "clips/abc.mp4").exists())

    def test_polling_times_out(self) -> None:
        server = self.mock(self.routes(poll=[PENDING]))
        self.assert_one_line_error(self.run_worker("animate", self.job(timeout=0.05), server), "job1", "still not finished")

    def test_temporary_poll_failure_is_tolerated(self) -> None:
        unavailable = error(503, "busy", {"Retry-After": "0"})
        server = self.mock(self.routes(poll=[unavailable, RUNNING, COMPLETED]))
        self.assertEqual(self.run_worker("animate", self.job(), server).returncode, 0)

    def test_submit_is_retried_when_rate_limited(self) -> None:
        limited = error(429, "slow down", {"Retry-After": "0"})
        server = self.mock(self.routes(submit=[limited, SUBMITTED]))
        self.assertEqual(self.run_worker("animate", self.job(), server).returncode, 0)
        self.assertEqual(server.count("POST", "/videos"), 2)

    def test_401(self) -> None:
        routes = self.routes()
        routes[("GET", "/videos/models")] = error(401, "bad key")
        server = self.mock(routes)
        self.assert_one_line_error(self.run_worker("animate", self.job(), server), "(401)")
        self.assertEqual(server.count("POST", "/videos"), 0)

    def test_402_on_submit(self) -> None:
        server = self.mock(self.routes(submit=error(402, "no credits")))
        self.assert_one_line_error(self.run_worker("animate", self.job(), server), "(402)", "credits")

    def test_missing_key(self) -> None:
        self.assert_one_line_error(self.run_worker("animate", self.job(), self.mock(self.routes()), key=None), "OPENROUTER_API_KEY is not set")

    def test_model_without_first_frame_is_refused_before_billing(self) -> None:
        server = self.mock(self.routes(caps={**CAPS, "supported_frame_images": []}))
        self.assert_one_line_error(self.run_worker("animate", self.job(), server), "cannot start from an image")
        self.assertEqual(server.count("POST", "/videos"), 0)

    def test_unknown_model(self) -> None:
        server = self.mock(self.routes(caps={**CAPS, "id": "other/model", "canonical_slug": "other/model"}))
        self.assert_one_line_error(self.run_worker("animate", self.job(), server), "unknown OpenRouter video model")

    def test_key_does_not_follow_a_redirect_to_another_origin(self) -> None:
        storage = self.mock({("GET", "/file.mp4"): (200, "video/mp4", MP4, {})})
        routes = self.routes()
        routes[("GET", "/videos/job1/content?index=0")] = (302, "text/plain", b"", {"Location": f"{storage.url}/file.mp4"})
        server = self.mock(routes)
        self.assertEqual(self.run_worker("animate", self.job(), server).returncode, 0)
        self.assertEqual([r[1] for r in storage.requests], ["/file.mp4"])
        self.assertNotIn("Authorization", storage.requests[0][2])


IMAGE_CAPS = {
    "id": "acme/image",
    "supported_parameters": {
        "input_references": {"type": "range", "min": 0, "max": 2},
        "aspect_ratio": {"type": "enum", "values": ["1:1", "16:9", "9:16", "auto"]},
        "resolution": {"type": "enum", "values": ["1K", "2K"]},
        "seed": {"type": "boolean"},
    },
}


class Draw(Case):
    def job(self, refs: int = 1) -> dict:
        paths = []
        for i in range(refs):
            path = self.dir / f"ref{i}.png"
            Image.new("RGB", (8, 8), "blue").save(path)
            paths.append(str(path))
        item = {"id": "p1", "prompt": "two heroes", "refs": paths, "width": 1360, "height": 768, "seed": 7, "steps": 4, "out": str(self.dir / "panels/abc.png")}
        return {"model": "acme/image", "items": [item]}

    def routes(self, caps: dict = IMAGE_CAPS, post=None) -> dict:
        image = {"created": 1, "data": [{"b64_json": base64.b64encode(JPEG).decode(), "media_type": "image/jpeg"}]}
        return {("GET", "/images/models"): as_json({"data": [caps]}), ("POST", "/images"): post if post is not None else as_json(image)}

    def test_references_are_sent_and_png_is_written(self) -> None:
        server = self.mock(self.routes())
        done = self.run_worker("draw_openrouter", self.job(), server)
        self.assertEqual(done.returncode, 0, done.stderr)
        with Image.open(self.dir / "panels/abc.png") as image:
            self.assertEqual(image.format, "PNG")
        body = server.bodies("POST", "/images")[0]
        reference = body.pop("input_references")[0]
        self.assertEqual(reference["type"], "image_url")
        self.assertTrue(reference["image_url"]["url"].startswith("data:image/png;base64,"))
        self.assertEqual(body, {"model": "acme/image", "prompt": "two heroes", "aspect_ratio": "16:9", "resolution": "1K", "seed": 7})

    def test_undeclared_parameters_are_not_sent(self) -> None:
        server = self.mock(self.routes(caps={"id": "acme/image", "supported_parameters": {}}))
        self.assertEqual(self.run_worker("draw_openrouter", self.job(refs=0), server).returncode, 0)
        self.assertEqual(server.bodies("POST", "/images")[0], {"model": "acme/image", "prompt": "two heroes"})

    def test_model_without_references_is_refused_before_billing(self) -> None:
        server = self.mock(self.routes(caps={"id": "acme/image", "supported_parameters": {"seed": {"type": "boolean"}}}))
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), server), "does not accept reference images")
        self.assertEqual(server.count("POST", "/images"), 0)

    def test_too_many_references_are_refused_before_billing(self) -> None:
        server = self.mock(self.routes())
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(refs=3), server), "at most 2")
        self.assertEqual(server.count("POST", "/images"), 0)

    def test_401(self) -> None:
        routes = self.routes()
        routes[("GET", "/images/models")] = error(401, "bad key")
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), self.mock(routes)), "(401)")

    def test_402(self) -> None:
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), self.mock(self.routes(post=error(402, "no credits")))), "(402)", "credits")

    def test_missing_key(self) -> None:
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), self.mock(self.routes()), key=None), "OPENROUTER_API_KEY is not set")

    def test_failed_generation(self) -> None:
        server = self.mock(self.routes(post=error(502, "generation failed", {"Retry-After": "0"})))
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), server), "(502)", "generation failed")
        self.assertEqual(server.count("POST", "/images"), openrouter.RETRIES + 1)

    def test_no_image_returned(self) -> None:
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), self.mock(self.routes(post=as_json({"data": []})))), "no image")

    def test_unreadable_image(self) -> None:
        post = as_json({"data": [{"b64_json": base64.b64encode(b"not an image").decode()}]})
        self.assert_one_line_error(self.run_worker("draw_openrouter", self.job(), self.mock(self.routes(post=post))), "cannot be read")


class BaseUrl(unittest.TestCase):
    def setUp(self) -> None:
        saved = os.environ.get(openrouter.BASE_URL_ENV)
        self.addCleanup(lambda: os.environ.__setitem__(openrouter.BASE_URL_ENV, saved) if saved is not None else os.environ.pop(openrouter.BASE_URL_ENV, None))

    def test_default_is_openrouter(self) -> None:
        os.environ.pop(openrouter.BASE_URL_ENV, None)
        self.assertEqual(openrouter.base_url(), "https://openrouter.ai/api/v1")

    def test_override_is_loopback_only(self) -> None:
        os.environ[openrouter.BASE_URL_ENV] = "http://127.0.0.1:9/api/"
        self.assertEqual(openrouter.base_url(), "http://127.0.0.1:9/api")
        os.environ[openrouter.BASE_URL_ENV] = "https://evil.example/api"
        with self.assertRaises(openrouter.Error):
            openrouter.base_url()


if __name__ == "__main__":
    unittest.main()
