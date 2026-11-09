//! `manga uninstall`: removes the app folder (code, venv, data, models, caches)
//! and the two links placed outside it: the binary and the Claude Code skill.
//! If the binary is installed (Homebrew…), only the data and the skill link are
//! removed: the code belongs to the package manager.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::home::{app_dir, data_dir, is_installed};

/// Symlinks that point into the app folder.
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
    println!("to remove:");
    if installed {
        println!("  {}  (Python venv, pages, projects, videos, models, caches)", target.display());
    } else {
        println!("  {}  (code, Python venv, pages, projects, videos, models, caches)", target.display());
    }
    for l in &links {
        println!("  {}  (link)", l.display());
    }
    if !yes {
        println!("\nnothing was removed. Confirm with: manga uninstall --yes");
        return Ok(());
    }
    for l in &links {
        fs::remove_file(l)?;
    }
    if installed {
        remove_data(&target)?;
        println!("removed. The binary remains: brew uninstall manga-anim (or your package manager).");
    } else {
        remove_tree(&target)?;
        println!("removed.");
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    // Safeguard: only remove a folder that really looks like the app.
    anyhow::ensure!(path.join("Cargo.toml").exists() && path.join("workers").exists(), "{} is not the manga-anim folder", path.display());
    fs::remove_dir_all(path)?;
    Ok(())
}

fn remove_data(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    // Safeguard: the folder must contain only what the app creates there.
    const OURS: [&str; 7] = ["library", "projects", "models", "cache", "venv", "venv-draw", ".DS_Store"];
    for entry in fs::read_dir(path)? {
        let name = entry?.file_name();
        anyhow::ensure!(OURS.contains(&name.to_string_lossy().as_ref()), "{} contains {:?}, which does not belong to manga-anim: nothing removed", path.display(), name);
    }
    fs::remove_dir_all(path)?;
    Ok(())
}
