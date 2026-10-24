//! Appel des workers Python : `uv run --project workers python -m mangaw.<tâche> <job.json>`.
//!
//! Le job est un fichier JSON écrit par Rust ; le worker écrit ses sorties là où le job
//! l'indique et Rust les relit avec serde, ce qui valide le contrat entre les deux côtés.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::home::{cache_dir, workers_dir};

pub fn run(task: &str, job: &impl Serialize, job_path: &Path) -> Result<()> {
    if let Some(parent) = job_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(job_path, serde_json::to_string_pretty(job)?)?;
    let workers = workers_dir();
    let cache = cache_dir();
    let status = Command::new("uv")
        .args(["run", "--quiet", "--project"])
        .arg(&workers)
        .args(["python", "-m", &format!("mangaw.{task}")])
        .arg(job_path)
        // Modèles et caches dans le dossier de l'app, jamais dans ~/.cache.
        .env("HF_HOME", cache.join("huggingface"))
        .env("TORCH_HOME", cache.join("torch"))
        .env("UV_CACHE_DIR", cache.join("uv"))
        .env("XDG_CACHE_HOME", &cache)
        .env("MPLCONFIGDIR", cache.join("matplotlib"))
        .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env("TRANSFORMERS_VERBOSITY", "error")
        .env("PYTHONWARNINGS", "ignore")
        .status()
        .with_context(|| "uv introuvable : installe-le (brew install uv)")?;
    if !status.success() {
        bail!("le worker {task} a échoué ({status}). Job : {}", job_path.display());
    }
    Ok(())
}
