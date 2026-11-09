//! Animate mode: shots marked `"mode": "animate"` become clips generated
//! locally by Wan2.2 TI2V-5B (mlx-video). Clips are cached in `clips/<hash>.mp4`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::home::data_dir;
use crate::project::Project;
use crate::render::crop_panel;
use crate::timeline::{Mode, Shot};
use crate::worker;

pub const BASE_PROMPT: &str = "Anime adaptation of this manga panel, smooth 2D animation. Subtle natural motion: \
characters breathe, blink and shift slightly, hair and clothes move with the wind, light flickers. \
Keep the exact drawing style, line art and composition. No camera cut, no text.";

pub fn model_dir() -> PathBuf {
    data_dir().join("models/wan2.2-ti2v-5b-mlx")
}

#[derive(Debug, Clone)]
pub struct Settings {
    /// Generated frames (4n+1). 49 ≈ 2 s at 24 fps.
    pub frames: u32,
    pub steps: Option<u32>,
    /// Maximum clip area in pixels: RAM is the limiting factor.
    pub max_area: u32,
    pub seed: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { frames: 49, steps: None, max_area: 640 * 352, seed: 42 }
    }
}

/// Clip dimensions: panel aspect ratio, multiples of 32, area ≤ max_area.
pub fn clip_size(pw: u32, ph: u32, max_area: u32) -> (u32, u32) {
    let ar = pw as f64 / ph as f64;
    let h = (max_area as f64 / ar).sqrt();
    let w = h * ar;
    let snap = |x: f64| ((x / 32.0).floor() as u32 * 32).max(256);
    (snap(w), snap(h))
}

#[derive(Serialize)]
struct Item {
    id: String,
    image: String,
    prompt: String,
    width: u32,
    height: u32,
    frames: u32,
    steps: Option<u32>,
    seed: i64,
    out: String,
}

#[derive(Serialize)]
struct Job {
    model_dir: String,
    items: Vec<Item>,
}

/// Generates the missing clips. Returns shot → clip for every animated shot.
pub fn ensure_clips(project: &Project, shots: &[&Shot], settings: &Settings) -> Result<HashMap<String, PathBuf>> {
    let animated: Vec<&&Shot> = shots.iter().filter(|s| s.mode == Mode::Animate).collect();
    let mut clips = HashMap::new();
    if animated.is_empty() {
        return Ok(clips);
    }
    let model = model_dir();
    if !model.join("config.json").exists() {
        bail!(
            "animation model missing ({}). Install it with `manga models install wan` (~34 GB to download), \
             or render in motion mode with --no-animate",
            model.display()
        );
    }
    let work = project.path("work/animate");
    fs::create_dir_all(&work)?;
    fs::create_dir_all(project.path("clips"))?;
    let mut items = Vec::new();
    let mut pages = HashMap::new();
    for shot in animated {
        if !pages.contains_key(&shot.page) {
            let img = image::open(project.path("pages").join(&shot.page)).with_context(|| format!("unreadable page: {}", shot.page))?;
            pages.insert(shot.page.clone(), img);
        }
        let panel = crop_panel(&pages[&shot.page], &shot.bbox);
        let image_path = work.join(format!("{}.png", shot.id));
        panel.save(&image_path)?;
        let prompt = match &shot.prompt {
            Some(p) => format!("{p}. {BASE_PROMPT}"),
            None => BASE_PROMPT.to_string(),
        };
        let (width, height) = clip_size(panel.width(), panel.height(), settings.max_area);
        let mut h = Sha256::new();
        h.update(panel.as_bytes());
        h.update(format!("{prompt}|{width}x{height}|{}|{:?}|{}", settings.frames, settings.steps, settings.seed));
        let out = project.path(format!("clips/{}.mp4", &hex::encode(h.finalize())[..16]));
        if !out.exists() {
            items.push(Item {
                id: shot.id.clone(),
                image: image_path.to_string_lossy().into(),
                prompt,
                width,
                height,
                frames: settings.frames,
                steps: settings.steps,
                seed: settings.seed,
                out: out.to_string_lossy().into(),
            });
        }
        clips.insert(shot.id.clone(), out);
    }
    if !items.is_empty() {
        println!("{} clips to generate with Wan2.2 (several minutes each)", items.len());
        let job = Job { model_dir: model.to_string_lossy().into(), items };
        worker::run("animate", &job, &project.path("work/animate_job.json"))?;
    }
    for (id, clip) in &clips {
        if !clip.exists() {
            bail!("clip not generated for shot {id}");
        }
    }
    Ok(clips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_sizes() {
        let (w, h) = clip_size(1600, 900, 640 * 352);
        assert!(w % 32 == 0 && h % 32 == 0 && w * h <= 640 * 352);
        assert!(w > h);
        let (w, h) = clip_size(400, 1600, 640 * 352);
        assert!(h > w && w >= 256);
    }
}
