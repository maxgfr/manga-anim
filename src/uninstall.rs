//! `manga uninstall` : supprime le dossier de l'app (code, venv, données, modèles, caches)
//! et les deux liens posés hors de ce dossier : le binaire et la skill Claude Code.
//! Si le binaire est installé (Homebrew…), seuls les données et le lien de la skill sont
//! supprimés : le code appartient au gestionnaire de paquets.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::home::{app_dir, data_dir, is_installed};

/// Liens symboliques qui pointent dans le dossier de l'app.
fn outside_links() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    [home.join(".cargo/bin/manga"), home.join(".claude/skills/manga")]
        .into_iter()
        .filter(|p| fs::read_link(p).is_ok_and(|target| target.starts_with(app_dir())))
        .collect()
}

pub fn run(yes: bool) -> Result<()> {
    let installed = is_installed();
    let target = if installed { data_dir() } else { app_dir() };
    let links = outside_links();
    println!("à supprimer :");
    if installed {
        println!("  {}  (venv Python, planches, projets, vidéos, modèles, caches)", target.display());
    } else {
        println!("  {}  (code, venv Python, planches, projets, vidéos, modèles, caches)", target.display());
    }
    for l in &links {
        println!("  {}  (lien)", l.display());
    }
    if !yes {
        println!("\nrien n'a été supprimé. Confirme avec : manga uninstall --yes");
        return Ok(());
    }
    for l in &links {
        fs::remove_file(l)?;
    }
    if installed {
        remove_data(&target)?;
        println!("supprimé. Reste le binaire : brew uninstall manga-anim (ou ton gestionnaire de paquets).");
    } else {
        remove_tree(&target)?;
        println!("supprimé.");
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    // Garde-fou : on ne supprime qu'un dossier qui ressemble bien à l'app.
    anyhow::ensure!(path.join("Cargo.toml").exists() && path.join("workers").exists(), "{} n'est pas le dossier de manga-anim", path.display());
    fs::remove_dir_all(path)?;
    Ok(())
}

fn remove_data(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    // Garde-fou : le dossier ne doit contenir que ce que l'app y crée.
    const OURS: [&str; 7] = ["library", "projects", "models", "cache", "venv", "venv-draw", ".DS_Store"];
    for entry in fs::read_dir(path)? {
        let name = entry?.file_name();
        anyhow::ensure!(OURS.contains(&name.to_string_lossy().as_ref()), "{} contient {:?}, qui n'est pas à manga-anim : rien supprimé", path.display(), name);
    }
    fs::remove_dir_all(path)?;
    Ok(())
}
