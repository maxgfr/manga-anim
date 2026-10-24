"""Synthèse vocale locale avec Kokoro-82M (mlx-audio).

Job : {"model": "...", "items": [{"id", "text", "voice", "speed", "out"}], "results": "/…/durations.json"}
La langue vient de la première lettre de la voix (ff_siwis → f = français).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

SAMPLE_RATE = 24000


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
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
