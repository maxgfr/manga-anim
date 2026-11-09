//! Locations on disk.
//!
//! When run from a clone of the repo, everything lives in that folder: code, Python venv, data,
//! models and caches. When installed (Homebrew or another prefix: `bin/manga` next to
//! `share/manga-anim/{workers,skills}`), the code stays in the prefix and everything else goes
//! into a single data folder. Either way, deleting these folders removes everything.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `<prefix>/share/manga-anim` if the binary is installed, `None` if it runs from the repo.
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

/// Code folder: the install prefix, otherwise the repo that built the binary.
pub fn app_dir() -> PathBuf {
    installed_share().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Data root: `library/`, `projects/`, `models/`, `cache/` (and `venv/` if installed).
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

/// Worker caches (Hugging Face, torch, uv…), redirected here rather than to `~/.cache`.
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn library_dir() -> PathBuf {
    data_dir().join("library")
}

pub fn projects_dir() -> PathBuf {
    data_dir().join("projects")
}

/// uv project of the ML workers.
pub fn workers_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MANGA_WORKERS") {
        return PathBuf::from(dir);
    }
    app_dir().join("workers")
}

/// Python venv of the workers: in the repo, or in the data folder if the code is installed
/// (the install prefix is replaced on every update).
pub fn venv_dir() -> PathBuf {
    if is_installed() { data_dir().join("venv") } else { workers_dir().join(".venv") }
}

/// Separate venv for the drawing worker: mflux needs transformers 5, Magi transformers 4.
pub fn draw_venv_dir() -> PathBuf {
    if is_installed() { data_dir().join("venv-draw") } else { workers_dir().join("draw/.venv") }
}

/// The Node worker and its `node_modules`: in the repo, or in the data folder if the code is
/// installed (the install prefix is replaced on every update).
pub fn connect_dir() -> PathBuf {
    if is_installed() { data_dir().join("connect") } else { workers_dir().join("connect") }
}

/// Font used for captions. Arial Unicode covers Latin, kana, kanji and hangul.
pub fn caption_font() -> PathBuf {
    if let Ok(font) = std::env::var("MANGA_FONT") {
        return PathBuf::from(font);
    }
    PathBuf::from("/System/Library/Fonts/Supplemental/Arial Unicode.ttf")
}

/// Readable folder identifier: lowercase letters, digits and hyphens.
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

/// "Natural" sort: `page2` before `page10`.
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
        assert_eq!(slugify("Vinland Saga — Volume 12!"), "vinland-saga-volume-12");
        assert_eq!(slugify("進撃の巨人"), "進撃の巨人");
    }

    #[test]
    fn natural_sort() {
        let mut v = vec!["p10.png", "p2.png", "p1.png"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["p1.png", "p2.png", "p10.png"]);
    }
}
