"""Panel drawing with FLUX.2 [klein] 4B (mflux), with characters passed as reference images.

The model is loaded only once: without references, editing behaves like text → image.
Job (written by `manga story build`):
{"model_dir": "...", "items": [{"id", "prompt", "refs": [...], "width", "height", "seed", "steps", "out"}]}
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path


def main() -> None:
    from mflux.models.common.config import ModelConfig
    from mflux.models.flux2.variants import Flux2KleinEdit

    job = json.loads(Path(sys.argv[1]).read_text())
    items = job["items"]
    t0 = time.time()
    model = Flux2KleinEdit(model_path=job["model_dir"], model_config=ModelConfig.flux2_klein_4b())
    print(f"flux2 loaded in {time.time() - t0:.0f}s", file=sys.stderr)
    started = time.time()
    for n, item in enumerate(items, 1):
        out = Path(item["out"])
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = out.with_suffix(".tmp.png")
        t0 = time.time()
        image = model.generate_image(
            seed=item["seed"],
            prompt=item["prompt"],
            num_inference_steps=item["steps"],
            width=item["width"],
            height=item["height"],
            image_paths=item["refs"] or None,
        )
        image.image.save(tmp)
        tmp.rename(out)
        eta = (time.time() - started) / n * (len(items) - n)
        print(f"[{n}/{len(items)}] {item['id']} {item['width']}x{item['height']} in {time.time() - t0:.0f}s, ~{eta / 60:.0f} min left", file=sys.stderr)


if __name__ == "__main__":
    main()
