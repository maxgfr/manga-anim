//! `manga uninstall` : supprime le dossier de l'app (code, venv, données, modèles, caches)
//! et les deux liens posés hors de ce dossier : le binaire et la skill Claude Code.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::home::app_dir;

/// Liens symboliques qui pointent dans le dossier de l'app.
fn outside_links() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    [home.join(".cargo/bin/manga"), home.join(".claude/skills/manga")]
        .into_iter()
        .filter(|p| fs::read_link(p).is_ok_and(|target| target.starts_with(app_dir())))
        .collect()
}

pub fn run(yes: bool) -> Result<()> {
    let app = app_dir();
    let links = outside_links();
    println!("à supprimer :");
    println!("  {}  (code, venv Python, planches, projets, vidéos, modèles, caches)", app.display());
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
    remove_tree(&app)?;
    println!("supprimé.");
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    // Garde-fou : on ne supprime qu'un dossier qui ressemble bien à l'app.
    anyhow::ensure!(path.join("Cargo.toml").exists() && path.join("workers").exists(), "{} n'est pas le dossier de manga-anim", path.display());
    fs::remove_dir_all(path)?;
    Ok(())
}
