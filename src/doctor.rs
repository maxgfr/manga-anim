//! `manga doctor`: what is installed, what is missing, and how to install it.

use std::path::Path;
use std::process::Command;

use crate::{animate, backend, story};
use crate::home::{app_dir, cache_dir, caption_font, data_dir, venv_dir, workers_dir};

fn tool(name: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(name).args(args).output().ok()?;
    let text = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout }).to_string();
    Some(text.lines().next().unwrap_or("").trim().to_string())
}

fn line(ok: bool, what: &str, detail: &str) {
    println!("{} {what:<22} {detail}", if ok { "✓" } else { "✗" });
}

fn hf_cached(repo: &str) -> bool {
    cache_dir().join("huggingface/hub").join(format!("models--{}", repo.replace('/', "--"))).exists()
}

fn size_gb(path: &Path) -> f64 {
    walkdir::WalkDir::new(path).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum::<u64>() as f64
        / 1024f64.powi(3)
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
    println!("app:     {} ({:.1} GB)", app_dir().display(), size_gb(&app_dir()));
    println!("data:    {} ({:.1} GB: pages, projects, models, caches)", data_dir().display(), size_gb(&data_dir()));
    println!("workers: {}", workers_dir().display());
    println!("remove everything: manga uninstall\n");
    match tool("ffmpeg", &["-version"]) {
        Some(v) => line(true, "ffmpeg", &v),
        None => line(false, "ffmpeg", "brew install ffmpeg"),
    }
    match tool("uv", &["--version"]) {
        Some(v) => line(true, "uv", &v),
        None => line(false, "uv", "brew install uv"),
    }
    let venv = venv_dir();
    let sync = format!("UV_PROJECT_ENVIRONMENT=\"{}\" uv sync --project \"{}\" --extra ja", venv.display(), workers_dir().display());
    line(venv.exists(), "Python workers", if venv.exists() { "ready" } else { &sync });
    let font = caption_font();
    line(font.exists(), "caption font", &font.display().to_string());
    line(tool("pdftoppm", &["-v"]).is_some(), "PDF import", "pdftoppm (brew install poppler)");
    let rar = tool("unar", &["-v"]).is_some() || tool("7z", &[]).is_some();
    line(rar, "CBR/7z import", if rar { "unar or 7z" } else { "optional: brew install unar" });

    println!();
    line(hf_cached("ragavsachdeva/magiv2"), "Magi v2 (analysis)", "downloaded on first `manga analyze` (~2 GB)");
    line(hf_cached("mlx-community/Kokoro-82M-bf16"), "Kokoro (voice)", "downloaded on first `manga voice` (~350 MB)");
    let manga_ocr = venv.join("lib/python3.12/site-packages/manga_ocr").exists();
    line(manga_ocr, "manga-ocr (Japanese)", if manga_ocr { "installed" } else { "optional: cd workers && uv sync --extra ja (add --extra animate if you use Wan: sync removes extras it is not given)" });
    let wan = animate::model_dir();
    let wan_ok = wan.join("config.json").exists();
    line(
        wan_ok,
        "Wan2.2 (animation)",
        if wan_ok { "installed" } else { "optional: cd workers && uv sync --extra ja --extra animate (without ja, sync removes it), then manga models install wan (~34 GB)" },
    );
    let flux = story::model_dir().exists();
    line(flux, "FLUX.2 klein (drawing)", if flux { "installed" } else { "optional, for manga story: manga models install flux2 (~15 GB, ~5 GB kept)" });
    let key = backend::key_configured();
    line(key, "OpenRouter key", if key { "set (--backend openrouter is billed and sends data to third parties)" } else { "optional: export OPENROUTER_API_KEY for --backend openrouter" });

    println!();
    let ram = sysctl("hw.memsize").map(|b| b as f64 / 1024f64.powi(3));
    if let Some(gb) = ram {
        line(gb >= 16.0, "memory", &format!("{gb:.0} GB{}", if gb < 24.0 { ": animation possible at low resolution only" } else { "" }));
    }
    let probe = app_dir();
    if let Some(gb) = free_gb(&probe) {
        line(gb > 10.0, "free disk", &format!("{gb:.0} GB"));
    }
}
