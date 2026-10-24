//! `manga doctor` : ce qui est installé, ce qui manque, et comment l'installer.

use std::path::Path;
use std::process::Command;

use crate::animate;
use crate::home::{caption_font, data_dir, workers_dir};

fn tool(name: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(name).args(args).output().ok()?;
    let text = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout }).to_string();
    Some(text.lines().next().unwrap_or("").trim().to_string())
}

fn line(ok: bool, what: &str, detail: &str) {
    println!("{} {what:<22} {detail}", if ok { "✓" } else { "✗" });
}

fn hf_cached(repo: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".cache/huggingface/hub").join(format!("models--{}", repo.replace('/', "--"))).exists()
}

fn sysctl(key: &str) -> Option<u64> {
    tool("sysctl", &["-n", key])?.parse().ok()
}

fn free_gb(path: &Path) -> Option<f64> {
    let out = Command::new("df").arg("-k").arg(path).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kb: f64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb / 1024.0 / 1024.0)
}

pub fn run() {
    println!("données : {}\nworkers : {}\n", data_dir().display(), workers_dir().display());
    match tool("ffmpeg", &["-version"]) {
        Some(v) => line(true, "ffmpeg", &v),
        None => line(false, "ffmpeg", "brew install ffmpeg"),
    }
    match tool("uv", &["--version"]) {
        Some(v) => line(true, "uv", &v),
        None => line(false, "uv", "brew install uv"),
    }
    let venv = workers_dir().join(".venv");
    line(venv.exists(), "workers Python", if venv.exists() { "prêts" } else { "cd workers && uv sync" });
    let font = caption_font();
    line(font.exists(), "police sous-titres", &font.display().to_string());
    line(tool("pdftoppm", &["-v"]).is_some(), "import PDF", "pdftoppm (brew install poppler)");
    let rar = tool("unar", &["-v"]).is_some() || tool("7z", &[]).is_some();
    line(rar, "import CBR/7z", if rar { "unar ou 7z" } else { "optionnel : brew install unar" });

    println!();
    line(hf_cached("ragavsachdeva/magiv2"), "Magi v2 (analyse)", "téléchargé au premier `manga analyze` (~2 Go)");
    line(hf_cached("mlx-community/Kokoro-82M-bf16"), "Kokoro (voix)", "téléchargé au premier `manga voice` (~350 Mo)");
    let manga_ocr = venv.join("lib/python3.12/site-packages/manga_ocr").exists();
    line(manga_ocr, "manga-ocr (japonais)", if manga_ocr { "installé" } else { "optionnel : cd workers && uv sync --extra ja" });
    let wan = animate::model_dir();
    let wan_ok = wan.join("config.json").exists();
    line(wan_ok, "Wan2.2 (animation)", if wan_ok { "installé" } else { "optionnel : manga models install wan (~34 Go)" });

    println!();
    let ram = sysctl("hw.memsize").map(|b| b as f64 / 1024f64.powi(3));
    if let Some(gb) = ram {
        line(gb >= 16.0, "mémoire", &format!("{gb:.0} Go{}", if gb < 24.0 { " : animation possible en basse résolution seulement" } else { "" }));
    }
    let probe = if data_dir().exists() { data_dir() } else { Path::new(&std::env::var("HOME").unwrap_or_default()).to_path_buf() };
    if let Some(gb) = free_gb(&probe) {
        line(gb > 10.0, "disque libre", &format!("{gb:.0} Go"));
    }
}
