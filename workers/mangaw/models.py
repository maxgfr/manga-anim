"""Installation of the heavy models.

`wan`: downloads Wan-AI/Wan2.2-TI2V-5B (~34 GB), converts it to MLX format with a
quantized transformer (4-bit by default), then deletes the original PyTorch weights.
Job: {"model": "wan", "src_dir": "...", "out_dir": "...", "bits": 4}
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path


def install_wan(job: dict) -> None:
    from huggingface_hub import snapshot_download

    try:
        from mlx_video.models.wan_2.convert import convert_wan_checkpoint
    except ModuleNotFoundError as e:
        if e.name is None or not e.name.startswith("mlx_video"):
            raise
        raise SystemExit(
            "mlx-video is not installed: run `uv sync --extra ja --extra animate` in the workers folder, then rerun this command "
            "(keep --extra ja: syncing without it removes the Japanese OCR)."
        ) from None

    src = Path(job["src_dir"])
    snapshot_download("Wan-AI/Wan2.2-TI2V-5B", local_dir=src, allow_patterns=["*.json", "*.safetensors", "*.pth"])
    convert_wan_checkpoint(str(src), job["out_dir"], "bfloat16", "2.2", quantize=True, bits=job.get("bits", 4))
    if not job.get("keep_source"):
        shutil.rmtree(src)
    print(f"model ready: {job['out_dir']}", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    if job["model"] == "wan":
        install_wan(job)
    else:
        raise SystemExit(f"unknown model: {job['model']}")


if __name__ == "__main__":
    main()
