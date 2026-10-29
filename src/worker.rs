//! Appel des workers Python : `uv run --project workers python -m mangaw.<tâche> <job.json>`.
//!
//! Le job est un fichier JSON écrit par Rust ; le worker écrit ses sorties là où le job
//! l'indique et Rust les relit avec serde, ce qui valide le contrat entre les deux côtés.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::home::{cache_dir, draw_venv_dir, is_installed, venv_dir, workers_dir};

/// Projet uv d'un worker. Le dessin a le sien : ses dépendances sont incompatibles avec Magi.
#[derive(Clone, Copy)]
pub enum Env {
    Main,
    Draw,
}

impl Env {
    fn project(self) -> PathBuf {
        match self {
            Env::Main => workers_dir(),
            Env::Draw => workers_dir().join("draw"),
        }
    }

    fn venv(self) -> PathBuf {
        match self {
            Env::Main => venv_dir(),
            Env::Draw => draw_venv_dir(),
        }
    }

    fn package(self) -> &'static str {
        match self {
            Env::Main => "mangaw",
            Env::Draw => "mangadraw",
        }
    }
}

pub fn run(task: &str, job: &impl Serialize, job_path: &Path) -> Result<()> {
    run_in(Env::Main, task, job, job_path)
}

pub fn run_in(env: Env, task: &str, job: &impl Serialize, job_path: &Path) -> Result<()> {
    if let Some(parent) = job_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(job_path, serde_json::to_string_pretty(job)?)?;
    let cache = cache_dir();
    let mut cmd = Command::new("uv");
    cmd.args(["run", "--quiet", "--project"]).arg(env.project());
    if is_installed() {
        // Le préfixe d'installation n'est pas à nous : on n'y réécrit pas uv.lock.
        cmd.arg("--frozen");
    }
    let status = cmd
        .args(["python", "-m", &format!("{}.{task}", env.package())])
        .arg(job_path)
        // Modèles et caches dans le dossier de l'app, jamais dans ~/.cache.
        .env("UV_PROJECT_ENVIRONMENT", env.venv())
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
