"""Speech synthesis, locally with Kokoro-82M (mlx-audio) or through OpenRouter.

Job: {"model": "...", "items": [{"id", "text", "voice", "speed", "out"}], "results": "/…/durations.json"}
Local: the language comes from the first letter of the voice (ff_siwis → f = French).
OpenRouter: the job also has "backend": "openrouter", and voices are the model's own ids. The
results file is updated after each line, so an interrupted run keeps the lines already paid for.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import wave
from pathlib import Path

SAMPLE_RATE = 24000


def mp3_to_wav(mp3: Path, item: dict) -> float:
    """Writes the pipeline's WAV (mono, 16-bit, SAMPLE_RATE) and returns its real duration.

    MP3 is requested because it describes itself: the docs give no sample rate for raw PCM.
    Speed is applied here because providers either ignore `speed` or reject non-default values.
    """
    from .openrouter import Error

    out = Path(item["out"])
    tmp = out.with_suffix(".tmp.wav")
    cmd = ["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "mp3", "-i", str(mp3)]
    if abs(item["speed"] - 1.0) > 1e-3:
        cmd += ["-filter:a", f"atempo={item['speed']}"]
    cmd += ["-ar", str(SAMPLE_RATE), "-ac", "1", "-c:a", "pcm_s16le", "-f", "wav", str(tmp)]
    try:
        done = subprocess.run(cmd, capture_output=True, text=True)
    except FileNotFoundError:
        raise Error("ffmpeg not found: install it with `brew install ffmpeg`") from None
    if done.returncode != 0:
        tmp.unlink(missing_ok=True)
        raise Error(f"the audio returned for {item['id']} could not be converted: {' '.join(done.stderr.split())[:300]}")
    with wave.open(str(tmp), "rb") as w:
        duration = w.getnframes() / w.getframerate()
    if duration <= 0:
        tmp.unlink(missing_ok=True)
        raise Error(f"OpenRouter returned empty audio for {item['id']}")
    tmp.rename(out)
    return duration


def write_results(path: str, durations: dict) -> None:
    tmp = f"{path}.tmp"
    Path(tmp).write_text(json.dumps(durations, indent=2))
    os.replace(tmp, path)


def run_openrouter(job: dict) -> None:
    from .openrouter import Client, Error, reported

    items = job["items"]
    durations = {}
    with reported():
        client = Client()
        for n, item in enumerate(items, 1):
            body = {"model": job["model"], "input": item["text"], "voice": item["voice"], "response_format": "mp3"}
            raw, content_type = client.post_binary("/audio/speech", body)
            if "mpeg" not in content_type and "mp3" not in content_type:
                raise Error(f"OpenRouter returned {content_type} for {item['id']} instead of MP3: this model may only output PCM, which manga cannot convert safely")
            out = Path(item["out"])
            out.parent.mkdir(parents=True, exist_ok=True)
            mp3 = out.with_suffix(".tmp.mp3")
            try:
                mp3.write_bytes(raw)
                durations[item["id"]] = mp3_to_wav(mp3, item)
            finally:
                mp3.unlink(missing_ok=True)
            write_results(job["results"], durations)
            print(f"[{n}/{len(items)}] {item['id']} {durations[item['id']]:.1f}s  {item['text'][:60]}", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    if job.get("backend") == "openrouter":
        run_openrouter(job)
        return
    import numpy as np
    import soundfile as sf
    from mlx_audio.tts.utils import load_model

    model = load_model(job["model"])
    durations = {}
    for n, item in enumerate(job["items"], 1):
        chunks = [
            np.asarray(r.audio, dtype=np.float32).reshape(-1)
            for r in model.generate(text=item["text"], voice=item["voice"], speed=item["speed"], lang_code=item["voice"][0])
        ]
        audio = np.concatenate(chunks) if chunks else np.zeros(SAMPLE_RATE // 4, np.float32)
        Path(item["out"]).parent.mkdir(parents=True, exist_ok=True)
        sf.write(item["out"], audio, SAMPLE_RATE)
        durations[item["id"]] = len(audio) / SAMPLE_RATE
        print(f"[{n}/{len(job['items'])}] {item['id']} {durations[item['id']]:.1f}s  {item['text'][:60]}", file=sys.stderr)
    Path(job["results"]).write_text(json.dumps(durations, indent=2))


if __name__ == "__main__":
    main()
