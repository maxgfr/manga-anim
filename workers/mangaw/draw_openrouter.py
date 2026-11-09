"""Panel drawing through OpenRouter's image API, with characters passed as reference images.

Runs in the main venv: it only needs Pillow and the shared HTTP client, not the mflux venv.
Job: {"model": "...", "items": [{"id", "prompt", "refs": [...], "width", "height", "seed", "steps", "out"}]}
"""

from __future__ import annotations

import base64
import binascii
import io
import json
import sys
from pathlib import Path

from PIL import Image, UnidentifiedImageError

from .animate import pick_ratio
from .openrouter import Client, Error, data_url, reported

RESOLUTION = "1K"  # ~1 Mpx, the size used by the local model.


def check_references(model: str, params: dict, items: list[dict]) -> None:
    """Fails before any paid call: an ignored reference would draw the wrong characters."""
    needed = max((len(i["refs"]) for i in items), default=0)
    if not needed:
        return
    descriptor = params.get("input_references")
    if descriptor is None:
        raise Error(f"{model} does not accept reference images: pick a model whose supported_parameters list input_references, or remove the refs from story.json")
    limit = descriptor.get("max") if isinstance(descriptor, dict) else None
    if limit is not None and needed > limit:
        raise Error(f"{model} accepts at most {int(limit)} reference images but a panel uses {needed}: use fewer characters or refs in that panel")


def image_request(model: str, item: dict, params: dict) -> dict:
    """Only sends what the model declares: an absent key means the parameter is unsupported."""
    body = {"model": model, "prompt": item["prompt"]}
    ratios = params.get("aspect_ratio", {})
    if ratio := pick_ratio(item["width"], item["height"], ratios.get("values") if isinstance(ratios, dict) else None):
        body["aspect_ratio"] = ratio
    resolutions = params.get("resolution", {})
    if isinstance(resolutions, dict) and RESOLUTION in resolutions.get("values", []):
        body["resolution"] = RESOLUTION
    if "seed" in params:
        body["seed"] = item["seed"]
    if item["refs"]:
        body["input_references"] = [{"type": "image_url", "image_url": {"url": data_url(r)}} for r in item["refs"]]
    return body


def save_png(response: dict, item: dict) -> None:
    data = response.get("data") or []
    encoded = data[0].get("b64_json") if data else None
    if not encoded:
        raise Error(f"OpenRouter returned no image for {item['id']}: try another seed or model")
    try:
        raw = base64.b64decode(encoded)
    except binascii.Error:
        raise Error(f"OpenRouter returned an image that cannot be read for {item['id']}") from None
    write_png(raw, item)


def write_png(raw: bytes, item: dict) -> None:
    """Providers choose the format they return; the pipeline reads panels as PNG."""
    try:
        image = Image.open(io.BytesIO(raw)).convert("RGB")
    except (UnidentifiedImageError, OSError):
        raise Error(f"the provider returned an image that cannot be read for {item['id']}") from None
    out = Path(item["out"])
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp.png")
    image.save(tmp, "PNG")
    tmp.rename(out)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    items = job["items"]
    with reported():
        client = Client()
        entry = client.model_entry("/images/models", job["model"], "image")
        params = entry.get("supported_parameters") or {}
        check_references(job["model"], params, items)
        for n, item in enumerate(items, 1):
            save_png(client.post_json("/images", image_request(job["model"], item, params)), item)
            print(f"[{n}/{len(items)}] {item['id']} drawn by {job['model']}", file=sys.stderr)


if __name__ == "__main__":
    main()
