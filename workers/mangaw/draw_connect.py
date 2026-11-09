"""Panel drawing through llm-connect (fal, OpenAI, Google, xAI, Replicate).

Runs in the main venv, like the OpenRouter path.
Job: {"backend": "connect", "provider", "model", "script", "items": [{"id", "prompt", "refs": [...], "width", "height", "seed", "steps", "out"}]}
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from .connect import generate
from .draw_openrouter import write_png
from .openrouter import reported


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    items = job["items"]
    with reported():
        for n, item in enumerate(items, 1):
            source = Path(item["out"]).with_suffix(".src")
            source.parent.mkdir(parents=True, exist_ok=True)
            try:
                generate(job, "image", {k: item[k] for k in ("id", "prompt", "refs", "width", "height", "seed")}, source)
                write_png(source.read_bytes(), item)
            finally:
                source.unlink(missing_ok=True)
            print(f"[{n}/{len(items)}] {item['id']} drawn by {job['model']} on {job['provider']}", file=sys.stderr)


if __name__ == "__main__":
    main()
