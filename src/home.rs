//! Emplacements sur disque.
//!
//! Lancé depuis un clone du dépôt, tout vit dans ce dossier : code, venv Python, données,
//! modèles et caches. Installé (Homebrew ou autre préfixe : `bin/manga` à côté de
//! `share/manga-anim/{workers,skills}`), le code reste dans le préfixe et tout le reste va
//! dans un seul dossier de données. Dans les deux cas, supprimer ces dossiers supprime tout.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `<préfixe>/share/manga-anim` si le binaire est installé, `None` s'il tourne depuis le dépôt.
fn installed_share() -> Option<&'static Path> {
    static SHARE: OnceLock<Option<PathBuf>> = OnceLock::new();
    SHARE
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
            let share = exe.parent()?.parent()?.join("share/manga-anim");
            share.join("workers/pyproject.toml").exists().then_some(share)
        })
        .as_deref()
}

pub fn is_installed() -> bool {
    installed_share().is_some()
}

/// Dossier du code : le préfixe d'installation, sinon le dépôt qui a compilé le binaire.
pub fn app_dir() -> PathBuf {
    installed_share().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Racine des données : `library/`, `projects/`, `models/`, `cache/` (et `venv/` si installé).
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MANGA_HOME") {
        return PathBuf::from(dir);
    }
    if is_installed() {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        return home.join("Library/Application Support/manga-anim");
    }
    app_dir().join("data")
}

/// Caches des workers (Hugging Face, torch, uv…), redirigés ici plutôt que dans `~/.cache`.
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn library_dir() -> PathBuf {
    data_dir().join("library")
}

pub fn projects_dir() -> PathBuf {
    data_dir().join("projects")
}

/// Projet uv des workers ML.
pub fn workers_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MANGA_WORKERS") {
        return PathBuf::from(dir);
    }
    app_dir().join("workers")
}

/// Venv Python des workers : dans le dépôt, ou dans les données si le code est installé
/// (le préfixe d'installation est remplacé à chaque mise à jour).
pub fn venv_dir() -> PathBuf {
    if is_installed() { data_dir().join("venv") } else { workers_dir().join(".venv") }
}

/// Venv du worker de dessin, à part : mflux veut transformers 5, Magi transformers 4.
pub fn draw_venv_dir() -> PathBuf {
    if is_installed() { data_dir().join("venv-draw") } else { workers_dir().join("draw/.venv") }
}

/// Police utilisée pour les sous-titres. Arial Unicode couvre latin, kana, kanji et hangul.
pub fn caption_font() -> PathBuf {
    if let Ok(font) = std::env::var("MANGA_FONT") {
        return PathBuf::from(font);
    }
    PathBuf::from("/System/Library/Fonts/Supplemental/Arial Unicode.ttf")
}

/// Identifiant de dossier lisible : minuscules, chiffres et tirets.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Tri « naturel » : `page2` avant `page10`.
pub fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut key = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                n.push(d);
                chars.next();
            }
            key.push((0, n.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            let mut t = String::new();
            while let Some(&d) = chars.peek().filter(|d| !d.is_ascii_digit()) {
                t.extend(d.to_lowercase());
                chars.next();
            }
            key.push((1, 0, t));
        }
    }
    key
}

pub fn is_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "avif")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug() {
        assert_eq!(slugify("Vinland Saga — Tome 12!"), "vinland-saga-tome-12");
        assert_eq!(slugify("進撃の巨人"), "進撃の巨人");
    }

    #[test]
    fn natural_sort() {
        let mut v = vec!["p10.png", "p2.png", "p1.png"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["p1.png", "p2.png", "p10.png"]);
    }
}
