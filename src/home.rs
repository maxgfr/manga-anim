//! Emplacements sur disque : données (`MANGA_HOME`) et workers Python.

use std::path::{Path, PathBuf};

/// Racine des données : `library/` (planches importées) et `projects/`.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MANGA_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".manga-anim")
}

pub fn library_dir() -> PathBuf {
    data_dir().join("library")
}

pub fn projects_dir() -> PathBuf {
    data_dir().join("projects")
}

/// Projet uv des workers ML. Par défaut celui du dépôt qui a compilé le binaire.
pub fn workers_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MANGA_WORKERS") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("workers")
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
