"""Génération des clips image→vidéo avec Wan2.2 TI2V-5B via mlx-video.

Un sous-processus par clip : la mémoire (16 Go partagés) est rendue entre deux clips.
Job : {"model_dir": "...", "items": [{"id", "image", "prompt", "width", "height", "frames", "steps", "seed", "out"}]}
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
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
        print(f"[{n}/{len(items)}] {item['id']} {item['width']}x{item['height']} {item['frames']} images", file=sys.stderr)
        subprocess.run(cmd, check=True)
        tmp.rename(out)
        took = time.time() - t0
        eta = (time.time() - started) / n * (len(items) - n)
        print(f"[{n}/{len(items)}] {item['id']} fait en {took / 60:.1f} min, reste ~{eta / 60:.0f} min", file=sys.stderr)


if __name__ == "__main__":
    main()
