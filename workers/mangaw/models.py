"""Installation des modèles lourds.

`wan` : télécharge Wan-AI/Wan2.2-TI2V-5B (~34 Go), le convertit au format MLX avec le
transformer quantifié (4 bits par défaut), puis supprime les poids PyTorch d'origine.
Job : {"model": "wan", "src_dir": "...", "out_dir": "...", "bits": 4}
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path


def install_wan(job: dict) -> None:
    from huggingface_hub import snapshot_download
    from mlx_video.models.wan_2.convert import convert_wan_checkpoint

    src = Path(job["src_dir"])
    snapshot_download("Wan-AI/Wan2.2-TI2V-5B", local_dir=src, allow_patterns=["*.json", "*.safetensors", "*.pth"])
    convert_wan_checkpoint(str(src), job["out_dir"], "bfloat16", "2.2", quantize=True, bits=job.get("bits", 4))
    if not job.get("keep_source"):
        shutil.rmtree(src)
    print(f"modèle prêt : {job['out_dir']}", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    if job["model"] == "wan":
        install_wan(job)
    else:
        raise SystemExit(f"modèle inconnu : {job['model']}")


if __name__ == "__main__":
    main()
