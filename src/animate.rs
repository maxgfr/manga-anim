//! Animate mode: shots marked `"mode": "animate"` become clips generated locally by
//! Wan2.2 TI2V-5B (mlx-video) or through OpenRouter. Clips are cached in `clips/<hash>.mp4`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::backend::Backend;
use crate::connect;
use crate::home::data_dir;
use crate::project::Project;
use crate::render::crop_panel;
use crate::timeline::{Mode, Shot};
use crate::worker;

pub const BASE_PROMPT: &str = "Anime adaptation of this manga panel, smooth 2D animation. Subtle natural motion: \
characters breathe, blink and shift slightly, hair and clothes move with the wind, light flickers. \
Keep the exact drawing style, line art and composition. No camera cut, no text.";

/// `manga anime`: the first frame is already a clean anime still, so the colors come from it.
const ANIME_PROMPT: &str = "Shot from a Japanese TV anime, fluid hand-drawn 2D animation. \
Keep the exact characters, colors, art style and framing of the first frame. No camera cut, no text, no speech bubbles.";
const NO_VOICES: &str = "Only these sound effects and quiet room tone: no speech, no voices, no music.";

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
    pub backend: Backend,
    /// `manga anime`: each clip starts from this key image instead of the panel.
    pub keyframes: HashMap<String, PathBuf>,
    /// `manga anime`: clips last as long as their shot and use the anime prompt.
    pub anime: bool,
    /// `manga anime`: the project's art style, the same in every prompt.
    pub art: String,
    /// `manga anime`: the model generates the sound effects along with the picture.
    pub sound: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { frames: 49, steps: None, max_area: 640 * 352, seed: 42, backend: Backend::Local, keyframes: HashMap::new(), anime: false, art: String::new(), sound: false }
    }
}

/// Seconds between two polls of a video job, as the OpenRouter docs suggest.
const POLL_INTERVAL_S: u64 = 30;
/// A clip usually takes up to several minutes; past this, the run stops and says so.
const POLL_TIMEOUT_S: u64 = 30 * 60;

/// Hashed with the panel's pixels. Local keys are unchanged, so existing clips stay valid;
/// steps do not apply to OpenRouter, so changing them must not bill a new clip.
fn clip_params(prompt: &str, width: u32, height: u32, frames: u32, settings: &Settings) -> String {
    let steps = if settings.backend.is_local() { settings.steps } else { None };
    format!("{prompt}|{width}x{height}|{frames}|{steps:?}|{}{}", settings.seed, settings.backend.cache_suffix())
}

/// Clip dimensions: panel aspect ratio, multiples of 32, area ≤ max_area.
pub fn clip_size(pw: u32, ph: u32, max_area: u32) -> (u32, u32) {
    let ar = pw as f64 / ph as f64;
    let h = (max_area as f64 / ar).sqrt();
    let w = h * ar;
    let snap = |x: f64| ((x / 32.0).floor() as u32 * 32).max(256);
    (snap(w), snap(h))
}

/// The fewest frames (4n+1, 24 fps) that cover a shot.
pub fn frames_for(duration: f64) -> u32 {
    (((duration * 24.0 - 1.0) / 4.0).ceil().max(1.0) as u32) * 4 + 1
}

fn shot_frames(shot: &Shot, settings: &Settings) -> u32 {
    match settings.anime {
        // A local clip costs minutes per second and memory is the limit: --frames stays a cap.
        true if settings.backend.is_local() => frames_for(shot.duration).min(settings.frames),
        true => frames_for(shot.duration),
        false => settings.frames,
    }
}

/// When a character speaks, so the mouth moves with the voice that will be laid over the clip.
fn speech_cue(shot: &Shot) -> Option<String> {
    let windows: Vec<String> = shot.lines.iter().filter(|l| l.speaker.is_some()).map(|l| format!("{:.1} to {:.1} s", l.start, l.start + l.duration)).collect();
    (!windows.is_empty()).then(|| format!("The speaking character's mouth moves only from {} and stays closed otherwise.", windows.join(" and from ")))
}

fn shot_prompt(shot: &Shot, settings: &Settings) -> String {
    let mut parts: Vec<String> = shot.prompt.iter().map(|p| format!("{p}.")).collect();
    if !settings.anime {
        parts.push(BASE_PROMPT.to_string());
        return parts.join(" ");
    }
    parts.push(ANIME_PROMPT.to_string());
    if !settings.art.is_empty() {
        parts.push(settings.art.clone());
    }
    parts.extend(speech_cue(shot));
    if settings.sound {
        let effects = shot.sound.as_deref().unwrap_or("the natural sounds of what happens on screen");
        parts.push(format!("Sound: {effects}. {NO_VOICES}"));
    }
    parts.join(" ")
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
    /// Generate the clip's sound as well.
    audio: bool,
    out: String,
}

#[derive(Serialize)]
struct Job {
    model_dir: String,
    items: Vec<Item>,
}

/// Never holds the API key: the worker reads it from the environment.
#[derive(Serialize)]
struct OpenRouterJob<'a> {
    backend: &'static str,
    model: &'a str,
    poll_interval_s: u64,
    poll_timeout_s: u64,
    items: Vec<Item>,
}

/// Never holds an API key: the worker reads it from the environment.
#[derive(Serialize)]
struct ConnectJob<'a> {
    #[serde(flatten)]
    connect: connect::Fields<'a>,
    poll_interval_s: u64,
    poll_timeout_s: u64,
    items: Vec<Item>,
}

/// The clips still to generate, and shot → clip for every animated shot.
fn plan_clips(project: &Project, shots: &[&Shot], settings: &Settings) -> Result<(Vec<Item>, HashMap<String, PathBuf>)> {
    let mut clips = HashMap::new();
    let work = project.path("work/animate");
    fs::create_dir_all(&work)?;
    fs::create_dir_all(project.path("clips"))?;
    let mut items = Vec::new();
    let mut pages = HashMap::new();
    for shot in shots.iter().filter(|s| s.mode == Mode::Animate) {
        let panel = match settings.keyframes.get(&shot.id) {
            Some(key) => image::open(key).with_context(|| format!("unreadable key image: {}", key.display()))?,
            None => {
                if !pages.contains_key(&shot.page) {
                    let img = image::open(project.path("pages").join(&shot.page)).with_context(|| format!("unreadable page: {}", shot.page))?;
                    pages.insert(shot.page.clone(), img);
                }
                crop_panel(&pages[&shot.page], &shot.bbox)
            }
        };
        let image_path = work.join(format!("{}.png", shot.id));
        panel.save(&image_path)?;
        let prompt = shot_prompt(shot, settings);
        let frames = shot_frames(shot, settings);
        let (width, height) = clip_size(panel.width(), panel.height(), settings.max_area);
        let mut h = Sha256::new();
        h.update(panel.as_bytes());
        h.update(clip_params(&prompt, width, height, frames, settings));
        let out = project.path(format!("clips/{}.mp4", &hex::encode(h.finalize())[..16]));
        if !out.exists() {
            items.push(Item {
                id: shot.id.clone(),
                image: image_path.to_string_lossy().into(),
                prompt,
                width,
                height,
                frames,
                steps: settings.steps,
                seed: settings.seed,
                audio: settings.sound,
                out: out.to_string_lossy().into(),
            });
        }
        clips.insert(shot.id.clone(), out);
    }
    Ok((items, clips))
}

/// The ids of the shots whose clip is not generated yet.
pub fn pending(project: &Project, shots: &[&Shot], settings: &Settings) -> Result<Vec<String>> {
    Ok(plan_clips(project, shots, settings)?.0.into_iter().map(|i| i.id).collect())
}

/// Generates the missing clips. Returns shot → clip for every animated shot.
pub fn ensure_clips(project: &Project, shots: &[&Shot], settings: &Settings) -> Result<HashMap<String, PathBuf>> {
    if !shots.iter().any(|s| s.mode == Mode::Animate) {
        return Ok(HashMap::new());
    }
    let model = model_dir();
    if settings.backend.is_local() && !model.join("config.json").exists() {
        bail!(
            "animation model missing ({}). Run `uv sync --extra ja --extra animate` in the workers folder, then \
             `manga models install wan` (~34 GB to download), or render in motion mode with --no-animate",
            model.display()
        );
    }
    let (items, clips) = plan_clips(project, shots, settings)?;
    if !items.is_empty() {
        let job_path = project.path("work/animate_job.json");
        match &settings.backend {
            Backend::Local => {
                println!("{} clips to generate with Wan2.2 (several minutes each)", items.len());
                let job = Job { model_dir: model.to_string_lossy().into(), items };
                worker::run("animate", &job, &job_path)?;
            }
            Backend::OpenRouter { model } => {
                settings.backend.check_key()?;
                println!("{} clips to generate with {model} on OpenRouter (billed per use; panels are sent to third parties)", items.len());
                let job = OpenRouterJob { backend: "openrouter", model, poll_interval_s: POLL_INTERVAL_S, poll_timeout_s: POLL_TIMEOUT_S, items };
                worker::run("animate", &job, &job_path)?;
            }
            Backend::Connect { provider, model } => {
                settings.backend.check_key()?;
                let script = connect::ensure_worker()?;
                println!("{} clips to generate with {model} on {provider} (billed per use; panels are sent to {provider})", items.len());
                let job = ConnectJob { connect: connect::Fields::new(provider, model, &script), poll_interval_s: POLL_INTERVAL_S, poll_timeout_s: POLL_TIMEOUT_S, items };
                worker::run("animate", &job, &job_path)?;
            }
        }
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

    fn openrouter(model: &str) -> Backend {
        Backend::OpenRouter { model: model.into() }
    }

    #[test]
    fn local_clip_key_is_unchanged() {
        let settings = Settings { steps: Some(20), ..Default::default() };
        assert_eq!(clip_params("wind", 640, 352, settings.frames, &settings), "wind|640x352|49|Some(20)|42");
    }

    #[test]
    fn clip_key_depends_on_backend_and_model() {
        let key = |backend| clip_params("wind", 640, 352, 49, &Settings { backend, ..Default::default() });
        assert_ne!(key(Backend::Local), key(openrouter("a/b")));
        assert_ne!(key(openrouter("a/b")), key(openrouter("a/c")));
    }

    #[test]
    fn connect_clip_key_depends_on_provider_and_model() {
        let key = |provider: &str, model: &str| clip_params("wind", 640, 352, 49, &Settings { backend: Backend::Connect { provider: provider.into(), model: model.into() }, ..Default::default() });
        assert_eq!(key("fal", "m"), "wind|640x352|49|None|42|connect|fal|m");
        assert_ne!(key("fal", "m"), key("google", "m"));
        assert_ne!(key("fal", "m"), key("fal", "n"));
    }

    #[test]
    fn openrouter_clip_key_ignores_steps() {
        let key = |steps| clip_params("wind", 640, 352, 49, &Settings { steps, backend: openrouter("a/b"), ..Default::default() });
        assert_eq!(key(Some(20)), key(None));
    }

    #[test]
    fn frames_cover_the_shot() {
        assert_eq!(frames_for(2.0), 49);
        assert_eq!(frames_for(0.01), 5);
        let frames = frames_for(6.3);
        assert!(frames % 4 == 1 && frames as f64 >= 6.3 * 24.0 && (frames as f64) < 6.3 * 24.0 + 4.0);
    }

    fn talking_shot(sound: Option<&str>) -> Shot {
        Shot {
            id: "k".into(),
            page: "p.png".into(),
            bbox: [0.0, 0.0, 1.0, 1.0],
            duration: 5.0,
            motion: crate::timeline::Motion::Static,
            mode: Mode::Animate,
            prompt: Some("He turns".into()),
            skip: false,
            extra: 0.0,
            sound: sound.map(String::from),
            characters: Vec::new(),
            lines: vec![crate::timeline::Line { text_id: "t".into(), text: "hi".into(), speaker: Some("Kit".into()), start: 0.4, duration: 1.5, audio: None }],
        }
    }

    #[test]
    fn anime_prompt_carries_style_timing_and_sound() {
        let settings = Settings { anime: true, art: "Fixed style.".into(), sound: true, ..Default::default() };
        let prompt = shot_prompt(&talking_shot(Some("door knock")), &settings);
        assert!(prompt.starts_with("He turns."));
        assert!(prompt.contains("Fixed style."));
        assert!(prompt.contains("mouth moves only from 0.4 to 1.9 s and stays closed otherwise"));
        assert!(prompt.contains("Sound: door knock."));
        assert!(prompt.contains("no speech, no voices, no music"));
    }

    #[test]
    fn a_silent_clip_asks_for_no_sound() {
        let settings = Settings { anime: true, ..Default::default() };
        assert!(!shot_prompt(&talking_shot(Some("door knock")), &settings).contains("Sound:"));
    }

    #[test]
    fn clip_sizes() {
        let (w, h) = clip_size(1600, 900, 640 * 352);
        assert!(w % 32 == 0 && h % 32 == 0 && w * h <= 640 * 352);
        assert!(w > h);
        let (w, h) = clip_size(400, 1600, 640 * 352);
        assert!(h > w && w >= 256);
    }
}
