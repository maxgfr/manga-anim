"""Image→video clip generation, with Wan2.2 TI2V-5B via mlx-video, OpenRouter or llm-connect.

Local: one subprocess per clip, so memory (16 GB shared) is released between clips.
Job: {"model_dir": "...", "items": [{"id", "image", "prompt", "width", "height", "frames", "steps", "seed", "out"}]}
OpenRouter: {"backend": "openrouter", "model", "poll_interval_s", "poll_timeout_s", "items": [same]}
llm-connect: same, with "backend": "connect", "provider" and "script".
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

FPS = 24


def pick_duration(wanted: float, supported: list[int] | None) -> int | None:
    """The shortest supported length that covers the clip, else the longest one."""
    if not supported:
        return None
    return min((d for d in supported if d >= wanted), default=max(supported))


def ratio_value(ratio: str) -> float | None:
    match = re.fullmatch(r"(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)", ratio)
    return float(match[1]) / float(match[2]) if match else None


def pick_ratio(width: int, height: int, supported: list[str] | None) -> str | None:
    """The supported aspect ratio closest to the panel's."""
    known = [(r, v) for r in supported or [] if (v := ratio_value(r))]
    return min(known, key=lambda k: abs(k[1] - width / height))[0] if known else None


def resolution_height(label: str) -> float | None:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)(p|K)", label)
    return float(match[1]) * (1024 if match[2] == "K" else 1) if match else None


def pick_resolution(short_side: int, supported: list[str] | None) -> str | None:
    """The smallest supported resolution that covers the clip, else the largest one."""
    known = sorted((h, r) for r in supported or [] if (h := resolution_height(r)))
    if not known:
        return None
    return next((r for h, r in known if h >= short_side), known[-1][1])


def video_request(model: str, item: dict, caps: dict) -> dict:
    """Only sends what the model declares, so an unsupported value cannot be rejected or billed."""
    from .openrouter import data_url

    body = {
        "model": model,
        "prompt": item["prompt"],
        "frame_images": [{"type": "image_url", "image_url": {"url": data_url(item["image"])}, "frame_type": "first_frame"}],
    }
    optional = {
        "duration": pick_duration(item["frames"] / FPS, caps.get("supported_durations")),
        "aspect_ratio": pick_ratio(item["width"], item["height"], caps.get("supported_aspect_ratios")),
        "resolution": pick_resolution(min(item["width"], item["height"]), caps.get("supported_resolutions")),
    }
    body.update({k: v for k, v in optional.items() if v is not None})
    if caps.get("generate_audio"):
        body["generate_audio"] = False  # The render discards clip audio: do not pay for it.
    if caps.get("seed"):
        body["seed"] = item["seed"]
    return body


def wait_for_video(client, job_id: str, interval: float, timeout: float) -> None:
    """Polls until the job completes; a failed or stalled job stops the run."""
    from .openrouter import Error, Transient, failure_message

    path = f"/videos/{urllib.parse.quote(job_id, safe='')}"
    deadline = time.monotonic() + timeout
    while True:
        try:
            state = client.parse(client.call("GET", path)[0])
        except Transient as e:
            print(f"{job_id}: {e}, still waiting", file=sys.stderr)
            state = {"status": "pending"}
        status = state.get("status")
        if status == "completed":
            return
        if status not in ("pending", "in_progress"):
            detail = client.one_line(failure_message(state.get("error"))) or "no detail given"
            raise Error(f"OpenRouter video job {job_id} ended as {status}: {detail}")
        left = deadline - time.monotonic()
        if left <= 0:
            raise Error(f"OpenRouter video job {job_id} still not finished after {timeout:.0f} s: it may complete and be billed anyway, rerun later")
        time.sleep(min(interval, left))


def fit_clip(source: Path, item: dict) -> None:
    """Same output as the local path: the requested size, H.264, no audio."""
    from .openrouter import Error

    out = Path(item["out"])
    tmp = out.with_suffix(".tmp.mp4")
    w, h = item["width"], item["height"]
    cmd = [
        "ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(source), "-an",
        "-vf", f"scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1",
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", str(tmp),
    ]
    try:
        done = subprocess.run(cmd, capture_output=True, text=True)
    except FileNotFoundError:
        raise Error("ffmpeg not found: install it with `brew install ffmpeg`") from None
    if done.returncode != 0:
        tmp.unlink(missing_ok=True)
        raise Error(f"the video downloaded for {item['id']} could not be converted: {' '.join(done.stderr.split())[:300]}")
    tmp.rename(out)


def run_openrouter(job: dict) -> None:
    from .openrouter import Client, Error, reported

    items = job["items"]
    with reported():
        client = Client()
        caps = client.model_entry("/videos/models", job["model"], "video")
        frames = caps.get("supported_frame_images")
        if frames is not None and "first_frame" not in frames:
            raise Error(f"{job['model']} cannot start from an image: pick an image-to-video model, or render with --no-animate")
        for n, item in enumerate(items, 1):
            out = Path(item["out"])
            out.parent.mkdir(parents=True, exist_ok=True)
            print(f"[{n}/{len(items)}] {item['id']} submitted to {job['model']}", file=sys.stderr)
            submitted = client.post_json("/videos", video_request(job["model"], item, caps))
            job_id = submitted.get("id")
            if not job_id:
                raise Error("OpenRouter did not return a video job id")
            wait_for_video(client, str(job_id), job["poll_interval_s"], job["poll_timeout_s"])
            source = out.with_suffix(".src.mp4")
            try:
                source.write_bytes(client.call("GET", f"/videos/{urllib.parse.quote(str(job_id), safe='')}/content?index=0")[0])
                fit_clip(source, item)
            finally:
                source.unlink(missing_ok=True)
            print(f"[{n}/{len(items)}] {item['id']} done", file=sys.stderr)


def run_connect(job: dict) -> None:
    from .connect import generate
    from .openrouter import reported

    items = job["items"]
    with reported():
        for n, item in enumerate(items, 1):
            out = Path(item["out"])
            out.parent.mkdir(parents=True, exist_ok=True)
            print(f"[{n}/{len(items)}] {item['id']} submitted to {job['model']} on {job['provider']}", file=sys.stderr)
            source = out.with_suffix(".src.mp4")
            try:
                request = {k: item[k] for k in ("id", "image", "prompt", "width", "height", "frames", "seed")}
                generate(job, "video", request, source, poll_interval_s=job["poll_interval_s"], poll_timeout_s=job["poll_timeout_s"])
                fit_clip(source, item)
            finally:
                source.unlink(missing_ok=True)
            print(f"[{n}/{len(items)}] {item['id']} done", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    if job.get("backend") == "openrouter":
        run_openrouter(job)
        return
    if job.get("backend") == "connect":
        run_connect(job)
        return
    items = job["items"]
    started = time.time()
    for n, item in enumerate(items, 1):
        out = Path(item["out"])
        tmp = out.with_suffix(".tmp.mp4")
        cmd = [
            sys.executable, "-m", "mlx_video.models.wan_2.generate",
            "--model-dir", job["model_dir"],
            "--image", item["image"],
            "--prompt", item["prompt"],
            "--width", str(item["width"]),
            "--height", str(item["height"]),
            "--num-frames", str(item["frames"]),
            "--seed", str(item["seed"]),
            "--output-path", str(tmp),
        ]
        if item.get("steps"):
            cmd += ["--steps", str(item["steps"])]
        t0 = time.time()
        print(f"[{n}/{len(items)}] {item['id']} {item['width']}x{item['height']} {item['frames']} frames", file=sys.stderr)
        subprocess.run(cmd, check=True)
        tmp.rename(out)
        took = time.time() - t0
        eta = (time.time() - started) / n * (len(items) - n)
        print(f"[{n}/{len(items)}] {item['id']} done in {took / 60:.1f} min, ~{eta / 60:.0f} min left", file=sys.stderr)


if __name__ == "__main__":
    main()
