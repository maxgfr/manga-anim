"""Installation de FLUX.2 [klein] 4B : téléchargé (~15 Go), enregistré quantifié dans `out_dir`,
puis le téléchargement est supprimé du cache Hugging Face.
Job : {"model": "flux2", "out_dir": "...", "bits": 4}
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path


def install_flux2(job: dict) -> None:
    from huggingface_hub import scan_cache_dir
    from mflux.models.common.config import ModelConfig
    from mflux.models.flux2.variants import Flux2Klein

    config = ModelConfig.flux2_klein_4b()
    tmp = Path(job["out_dir"]).with_suffix(".tmp")
    if tmp.exists():
        shutil.rmtree(tmp)
    Flux2Klein(quantize=job.get("bits", 4), model_config=config).save_model(str(tmp))
    tmp.rename(job["out_dir"])
    for repo in scan_cache_dir().repos:
        if repo.repo_id == config.model_name:
            shutil.rmtree(repo.repo_path)
    print(f"modèle prêt : {job['out_dir']}", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    if job["model"] == "flux2":
        install_flux2(job)
    else:
        raise SystemExit(f"modèle inconnu : {job['model']}")


if __name__ == "__main__":
    main()
