//! Renders the timeline to video: one MP4 per shot (cached), then concatenation.
//!
//! Two styles. `scroll` (default): the full-width page scrolls from panel to panel as it is
//! read, like a YouTube manga reading video. `panels`: the cropped panel, animated by the
//! camera (zoom or pan), on a blurred background. In both cases a generated clip
//! replaces the animated panel, with captions and voice.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView};
use sha2::{Digest, Sha256};

use crate::captions::{self, CaptionStyle};
use crate::home::caption_font;
use crate::project::Project;
use crate::timeline::{Motion, Shot, Timeline};

/// Bump when the rendering of a shot changes, to invalidate the cache.
const RENDER_VERSION: u32 = 3;
const FIT: f64 = 0.92;
const PAN_ZOOM: f64 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Style {
    /// The full-width page scrolls from panel to panel during playback.
    Scroll,
    /// One panel at a time on a blurred background, with zoom or pan.
    Panels,
    /// `manga anime`: key images and clips fill the frame. Needs the key images, so not a `--style`.
    #[value(skip)]
    Anime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// 1920×1080.
    Landscape,
    /// 1080×1920, for TikTok, YouTube Shorts and Reels.
    Vertical,
}

impl Format {
    pub fn frame(self) -> (u32, u32) {
        match self {
            Format::Landscape => (1920, 1080),
            Format::Vertical => (1080, 1920),
        }
    }
}

/// The shape of the output of `manga render`.
pub struct Output {
    pub format: Format,
    pub max_duration: Option<f64>,
    pub split: bool,
}

#[derive(Clone)]
pub struct RenderOptions {
    pub out: PathBuf,
    pub style: Style,
    /// Ids or numbers (1 = first shot) to render; empty = all.
    pub shots: Vec<String>,
    pub show_speaker: bool,
    pub captions: bool,
    pub jobs: usize,
    /// `manga anime`: shot → key image.
    pub keyframes: HashMap<String, PathBuf>,
}

/// Crops the panel with a small margin.
pub fn crop_panel(page: &DynamicImage, bbox: &[f64; 4]) -> DynamicImage {
    let (w, h) = page.dimensions();
    let margin = 0.008 * w.max(h) as f64;
    let x1 = (bbox[0] - margin).clamp(0.0, w as f64 - 2.0) as u32;
    let y1 = (bbox[1] - margin).clamp(0.0, h as f64 - 2.0) as u32;
    let x2 = ((bbox[2] + margin).clamp(x1 as f64 + 2.0, w as f64)) as u32;
    let y2 = ((bbox[3] + margin).clamp(y1 as f64 + 2.0, h as f64)) as u32;
    page.crop_imm(x1, y1, x2 - x1, y2 - y1)
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32 * 2).max(2)
}

/// How the panel fills the frame: zoompan on a still image, or a sliding window.
#[derive(Debug, PartialEq)]
pub enum Camera {
    /// Whole image visible (fw×fh), zoom from `from` to `to`.
    Zoom { fw: u32, fh: u32, from: f64, to: f64 },
    /// Image enlarged to sw×sh, sliding vw×vh window. `axis_x`: horizontal direction.
    Pan { sw: u32, sh: u32, vw: u32, vh: u32, axis_x: bool, reverse: bool },
    /// Video clip scaled to fw×fh.
    Clip { fw: u32, fh: u32 },
    /// Page strip already at frame width, full-frame window whose top goes from y0 to y1.
    Scroll { y0: f64, y1: f64 },
    /// Anime: the clip fills the frame and holds its last image if the shot runs longer.
    Cover,
    /// Anime: a key image without a clip fills the frame, zooming from `from` to `to`.
    CoverStill { from: f64, to: f64 },
}

pub fn camera(pw: u32, ph: u32, frame: (u32, u32), motion: Motion, clip: bool) -> Camera {
    let (bw, bh) = (frame.0 as f64 * FIT, frame.1 as f64 * FIT);
    let ar = pw as f64 / ph as f64;
    let (fw, fh) = if ar > bw / bh { (bw, bw / ar) } else { (bh * ar, bh) };
    let zoom = |from, to| Camera::Zoom { fw: even(fw), fh: even(fh), from, to };
    if clip {
        return Camera::Clip { fw: even(fw), fh: even(fh) };
    }
    match motion {
        Motion::Static => zoom(1.0, 1.0),
        Motion::ZoomIn => zoom(1.0, 1.1),
        Motion::ZoomOut => zoom(1.1, 1.0),
        Motion::PanDown | Motion::PanUp => {
            let vw = (fw * PAN_ZOOM).min(bw);
            let (sw, sh) = (vw, vw / ar);
            if sh - bh < 16.0 {
                return zoom(1.0, 1.1);
            }
            Camera::Pan { sw: even(sw), sh: even(sh), vw: even(vw), vh: even(bh), axis_x: false, reverse: motion == Motion::PanUp }
        }
        Motion::PanLeft | Motion::PanRight => {
            let vh = (fh * PAN_ZOOM).min(bh);
            let (sw, sh) = (vh * ar, vh);
            if sw - bw < 16.0 {
                return zoom(1.0, 1.1);
            }
            Camera::Pan { sw: even(sw), sh: even(sh), vw: even(bw), vh: even(vh), axis_x: true, reverse: motion == Motion::PanLeft }
        }
    }
}

pub struct CaptionPlacement {
    pub w: u32,
    pub h: u32,
    pub start: f64,
    pub end: f64,
}

/// ffmpeg filtergraph for a shot. Inputs: 0 = panel (image or clip), 1..=n = captions,
/// then the audio tracks.
#[allow(clippy::too_many_arguments)]
pub fn filter_graph(cam: &Camera, duration: f64, fps: u32, frame: (u32, u32), captions: &[CaptionPlacement], audio_starts: &[f64], clip_sound: bool) -> String {
    let (w, h) = frame;
    let frames = (duration * fps as f64).ceil() as u32;
    let ease = format!("(1-cos(PI*min(t/{duration:.3}\\,1)))/2");
    let mut g = String::new();
    match cam {
        Camera::Zoom { fw, fh, from, to } => {
            // Source image at 2× to avoid zoompan jitter on small steps.
            let z = format!("{from}+({to}-{from})*on/{frames}");
            write!(
                g,
                "[0:v]scale={}:{}:flags=lanczos,zoompan=z='{z}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d={frames}:s={fw}x{fh}:fps={fps},setsar=1[fg];",
                fw * 2,
                fh * 2
            )
            .unwrap();
        }
        Camera::Pan { sw, sh, vw, vh, axis_x, reverse } => {
            let p = if *reverse { format!("(1-{ease})") } else { ease.clone() };
            let (x, y) = if *axis_x { (format!("({sw}-{vw})*{p}"), "0".into()) } else { ("0".into(), format!("({sh}-{vh})*{p}")) };
            write!(g, "[0:v]scale={sw}:{sh}:flags=lanczos,fps={fps},crop={vw}:{vh}:x='{x}':y='{y}',setsar=1[fg];").unwrap();
        }
        Camera::Clip { fw, fh } => {
            write!(g, "[0:v]scale={fw}:{fh}:flags=lanczos,fps={fps},setsar=1[fg];").unwrap();
        }
        // Constant speed over the shot: the scroll follows the reading, without jerks.
        Camera::Scroll { y0, y1 } => {
            write!(g, "[0:v]fps={fps},crop={w}:{h}:x=0:y='{y0:.2}+({:.2})*min(t/{duration:.3}\\,1)',setsar=1[v0];", y1 - y0).unwrap();
        }
        Camera::Cover => {
            write!(g, "[0:v]scale={w}:{h}:force_original_aspect_ratio=increase:flags=lanczos,crop={w}:{h},fps={fps},tpad=stop_mode=clone:stop=-1,setsar=1[v0];").unwrap();
        }
        Camera::CoverStill { from, to } => {
            let (sw, sh) = (w * 2, h * 2);
            let z = format!("{from}+({to}-{from})*on/{frames}");
            write!(
                g,
                "[0:v]scale={sw}:{sh}:force_original_aspect_ratio=increase:flags=lanczos,crop={sw}:{sh},\
                 zoompan=z='{z}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d={frames}:s={w}x{h}:fps={fps},setsar=1[v0];"
            )
            .unwrap();
        }
    }
    if !matches!(cam, Camera::Scroll { .. } | Camera::Cover | Camera::CoverStill { .. }) {
        let (bw, bh) = if h > w { (270, 480) } else { (480, 270) };
        write!(
            g,
            "[fg]split[fga][fgb];\
             [fgb]scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},gblur=sigma=10,\
             colorchannelmixer=rr=0.45:gg=0.45:bb=0.45,scale={w}:{h},setsar=1[bg];\
             [bg][fga]overlay=x=(W-w)/2:y=(H-h)/2[v0];"
        )
        .unwrap();
    }
    // On a phone, the app's own buttons cover the bottom of a vertical video.
    let margin = (h as f64 * if h > w { 0.16 } else { 0.045 }) as u32;
    for (i, c) in captions.iter().enumerate() {
        write!(
            g,
            "[v{i}][{}:v]overlay=x={}:y={}:enable='between(t\\,{:.3}\\,{:.3})'[v{}];",
            i + 1,
            (w - c.w.min(w)) / 2,
            h.saturating_sub(c.h + margin),
            c.start,
            c.end,
            i + 1
        )
        .unwrap();
    }
    write!(g, "[v{}]format=yuv420p[vout]", captions.len()).unwrap();

    let first_audio = 1 + captions.len();
    // The clip's own sound effects, at the same level from one shot to the next.
    let bed = "[0:a]loudnorm=I=-20:TP=-2,aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
    if audio_starts.is_empty() {
        match clip_sound {
            true => write!(g, ";{bed},apad[aout]").unwrap(),
            false => write!(g, ";anullsrc=r=48000:cl=stereo[aout]").unwrap(),
        }
    } else {
        for (i, start) in audio_starts.iter().enumerate() {
            let ms = (start * 1000.0).round() as u64;
            write!(g, ";[{}:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,adelay={ms}:all=1[a{i}]", first_audio + i).unwrap();
        }
        let inputs: String = (0..audio_starts.len()).map(|i| format!("[a{i}]")).collect();
        let voices = format!("{inputs}amix=inputs={}:normalize=0:duration=longest,volume=1.8", audio_starts.len());
        if clip_sound {
            // The effects duck under the voices.
            write!(
                g,
                ";{voices},asplit=2[vo][key];{bed},apad[bed];[bed][key]sidechaincompress=threshold=0.03:ratio=8:attack=20:release=350[fx];\
                 [vo][fx]amix=inputs=2:normalize=0:duration=longest,apad[aout]"
            )
            .unwrap();
        } else {
            write!(g, ";{voices},apad[aout]").unwrap();
        }
    }
    g
}

/// Camera path in scroll style: for each shot, the top of the window at the start and
/// at the end, in pixels of the page scaled to frame width.
///
/// Each panel passes through the center while it is read (a panel taller than the frame is
/// traversed top to bottom), the camera slides to the next one halfway between two shots
/// and never moves back up. A page's overview shot holds the top of the page. A page
/// too short to scroll (double-page spread) has no path: rendered in panels style.
pub fn scroll_path(shots: &[&Shot], page_size: impl Fn(&str) -> Option<(u32, u32)>, frame: (u32, u32)) -> HashMap<String, (f64, f64)> {
    let (w, h) = (frame.0 as f64, frame.1 as f64);
    let mut path = HashMap::new();
    for group in shots.chunk_by(|a, b| a.page == b.page) {
        let Some((pw, ph)) = page_size(&group[0].page) else { continue };
        let s = w / pw as f64;
        let max_top = ph as f64 * s - h;
        if max_top < 1.0 {
            continue;
        }
        let mut ranges: Vec<(f64, f64)> = Vec::new();
        for shot in group {
            let (top, bottom) = (shot.bbox[1] * s, shot.bbox[3] * s);
            let (lo, hi) = if shot.id.ends_with("_page") {
                (0.0, 0.0)
            } else if bottom - top <= h {
                let c = (top + bottom - h) / 2.0;
                (c, c)
            } else {
                (top, bottom - h)
            };
            let floor = ranges.last().map_or(0.0, |r| r.1);
            let lo = lo.clamp(0.0, max_top).max(floor);
            ranges.push((lo, hi.clamp(0.0, max_top).max(lo)));
        }
        for (i, shot) in group.iter().enumerate() {
            let start = if i == 0 { ranges[i].0 } else { (ranges[i - 1].1 + ranges[i].0) / 2.0 };
            let end = ranges.get(i + 1).map_or(ranges[i].1, |n| (ranges[i].1 + n.0) / 2.0);
            path.insert(shot.id.clone(), (start, end));
        }
    }
    path
}

/// Strip of the page covering the path, scaled to frame width, and the path relative to the strip.
fn scroll_band(page: &DynamicImage, frame: (u32, u32), (y0, y1): (f64, f64)) -> (DynamicImage, Camera) {
    let (pw, ph) = page.dimensions();
    let s = frame.0 as f64 / pw as f64;
    let top = ((y0.min(y1) / s).floor() as u32).min(ph - 1);
    let bottom = (((y0.max(y1) + frame.1 as f64) / s).ceil() as u32).clamp(top + 1, ph);
    let band_h = (((bottom - top) as f64 * s).round() as u32).max(frame.1);
    let band = page.crop_imm(0, top, pw, bottom - top).resize_exact(frame.0, band_h, image::imageops::FilterType::Lanczos3);
    let room = (band_h - frame.1) as f64;
    let shift = |y: f64| (y - top as f64 * s).clamp(0.0, room);
    (band, Camera::Scroll { y0: shift(y0), y1: shift(y1) })
}

fn cache_key(project: &Project, tl: &Timeline, shot: &Shot, clip: Option<&Path>, scroll: Option<(f64, f64)>, opts: &RenderOptions) -> Result<String> {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(shot)?);
    h.update(format!("{}x{}@{} v{RENDER_VERSION} spk={} cap={} scroll={scroll:?}", tl.width, tl.height, tl.fps, opts.show_speaker, opts.captions));
    let page = fs::metadata(project.path("pages").join(&shot.page))?;
    h.update(format!("{}:{:?}", page.len(), page.modified().ok()));
    for line in &shot.lines {
        if let Some(a) = &line.audio {
            let m = fs::metadata(project.path(a)).with_context(|| format!("missing audio: {a} (manga voice …)"))?;
            h.update(format!("{a}:{}:{:?}", m.len(), m.modified().ok()));
        }
    }
    if let Some(c) = clip {
        h.update(c.to_string_lossy().as_bytes());
    }
    if opts.style == Style::Anime {
        h.update(format!("anime {:?}", opts.keyframes.get(&shot.id)));
    }
    Ok(hex::encode(h.finalize())[..12].to_string())
}

/// Whether a generated clip came with a sound track.
fn has_audio(clip: &Path) -> bool {
    Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(clip)
        .output()
        .is_ok_and(|o| !o.stdout.is_empty())
}

/// The loudest level of a `max_volume: -12.3 dB` line from ffmpeg's volumedetect.
fn max_volume(report: &str) -> Option<f64> {
    report.lines().find_map(|l| l.split("max_volume:").nth(1)?.trim().strip_suffix("dB")?.trim().parse().ok())
}

/// Below this, a clip's sound track is silence or noise floor, not sound effects.
const AUDIBLE_DB: f64 = -45.0;

/// Whether a clip has sound anyone would hear. A model can return a silent track.
pub fn is_audible(clip: &Path) -> bool {
    let probe = Command::new("ffmpeg").args(["-hide_banner", "-nostats", "-i"]).arg(clip).args(["-af", "volumedetect", "-vn", "-f", "null", "-"]).output();
    probe.is_ok_and(|o| max_volume(&String::from_utf8_lossy(&o.stderr)).is_some_and(|db| db > AUDIBLE_DB))
}

#[allow(clippy::too_many_arguments)]
fn render_shot(project: &Project, tl: &Timeline, shot: &Shot, clip: Option<&Path>, scroll: Option<(f64, f64)>, style: &CaptionStyle, opts: &RenderOptions, out: &Path) -> Result<()> {
    let work = out.with_extension("parts");
    fs::create_dir_all(&work)?;
    let frame = (tl.width, tl.height);
    let panel_path = work.join("panel.png");
    let cam = if opts.style == Style::Anime {
        let key = opts.keyframes.get(&shot.id).with_context(|| format!("no key image for shot {}", shot.id))?;
        image::open(key).with_context(|| format!("unreadable key image: {}", key.display()))?.save(&panel_path)?;
        match (clip, shot.motion) {
            (Some(_), _) => Camera::Cover,
            (None, Motion::Static) => Camera::CoverStill { from: 1.0, to: 1.0 },
            (None, Motion::ZoomOut) => Camera::CoverStill { from: 1.08, to: 1.0 },
            (None, _) => Camera::CoverStill { from: 1.0, to: 1.08 },
        }
    } else {
        let page = image::open(project.path("pages").join(&shot.page)).with_context(|| format!("unreadable page: {}", shot.page))?;
        match scroll.filter(|_| clip.is_none()) {
            Some(path) => {
                let (band, cam) = scroll_band(&page, frame, path);
                band.save(&panel_path)?;
                cam
            }
            None => {
                let panel = crop_panel(&page, &shot.bbox);
                panel.save(&panel_path)?;
                camera(panel.width(), panel.height(), frame, shot.motion, clip.is_some())
            }
        }
    };
    let mut caption_paths = Vec::new();
    let mut placements = Vec::new();
    for (i, line) in shot.lines.iter().enumerate().filter(|_| opts.captions) {
        let img = captions::render(style, &line.text, line.speaker.as_deref());
        let path = work.join(format!("cap{i}.png"));
        img.save(&path)?;
        let end = shot.lines.get(i + 1).map(|n| n.start).unwrap_or(shot.duration).min(line.start + line.duration + 0.4);
        placements.push(CaptionPlacement { w: img.width(), h: img.height(), start: line.start, end });
        caption_paths.push(path);
    }
    let audio: Vec<(f64, PathBuf)> = shot.lines.iter().filter_map(|l| l.audio.as_ref().map(|a| (l.start, project.path(a)))).collect();
    let starts: Vec<f64> = audio.iter().map(|a| a.0).collect();
    let clip_sound = matches!(cam, Camera::Cover) && clip.is_some_and(has_audio);
    let graph = filter_graph(&cam, shot.duration, tl.fps, frame, &placements, &starts, clip_sound);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    let dur = format!("{:.3}", shot.duration);
    match (&cam, clip) {
        (Camera::Clip { .. }, Some(c)) => cmd.args(["-stream_loop", "-1", "-t", &dur, "-i"]).arg(c),
        (Camera::Cover, Some(c)) => cmd.arg("-i").arg(c),
        (Camera::Pan { .. } | Camera::Scroll { .. }, _) => cmd.args(["-loop", "1", "-framerate", &tl.fps.to_string(), "-t", &dur, "-i"]).arg(&panel_path),
        _ => cmd.arg("-i").arg(&panel_path),
    };
    for c in &caption_paths {
        cmd.arg("-i").arg(c);
    }
    for (_, a) in &audio {
        cmd.arg("-i").arg(a);
    }
    cmd.args(["-filter_complex", &graph, "-map", "[vout]", "-map", "[aout]", "-t", &dur, "-r", &tl.fps.to_string()])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "2"])
        .arg(out);
    let output = cmd.output().context("ffmpeg not found (brew install ffmpeg)")?;
    if !output.status.success() {
        fs::write(work.join("filter.txt"), &graph).ok();
        bail!("ffmpeg failed on shot {}: {}", shot.id, String::from_utf8_lossy(&output.stderr).trim());
    }
    fs::remove_dir_all(&work).ok();
    Ok(())
}

pub fn select<'a>(tl: &'a Timeline, wanted: &[String]) -> Result<Vec<&'a Shot>> {
    let active: Vec<&Shot> = tl.shots.iter().filter(|s| !s.skip).collect();
    if wanted.is_empty() {
        return Ok(active);
    }
    wanted
        .iter()
        .map(|w| match w.parse::<usize>() {
            Ok(n) if n >= 1 && n <= tl.shots.len() => Ok(&tl.shots[n - 1]),
            _ => tl.shots.iter().find(|s| &s.id == w).with_context(|| format!("unknown shot: {w}")),
        })
        .collect()
}

/// Groups shots, in order and without cutting one, so each group lasts at most `max` seconds.
/// Without `split`, only the first group is kept. A shot longer than `max` is a group of its own.
pub fn parts(shots: &[&Shot], max: Option<f64>, split: bool) -> Vec<Vec<String>> {
    let Some(max) = max else { return vec![shots.iter().map(|s| s.id.clone()).collect()] };
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut length = f64::INFINITY;
    for shot in shots {
        if length + shot.duration > max {
            groups.push(Vec::new());
            length = 0.0;
        }
        groups.last_mut().unwrap().push(shot.id.clone());
        length += shot.duration;
    }
    if !split {
        groups.truncate(1);
    }
    groups
}

fn part_path(out: &Path, n: usize) -> PathBuf {
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    let extension = out.extension().unwrap_or_default().to_string_lossy();
    out.with_file_name(format!("{stem}-part{n}.{extension}"))
}

/// Renders one file per group of shots, numbered when there are several.
pub fn run_parts(project: &Project, tl: &Timeline, clips: &HashMap<String, PathBuf>, opts: RenderOptions, groups: Vec<Vec<String>>) -> Result<Vec<PathBuf>> {
    let several = groups.len() > 1;
    groups
        .into_iter()
        .enumerate()
        .map(|(i, shots)| {
            let out = if several { part_path(&opts.out, i + 1) } else { opts.out.clone() };
            run(project, tl, clips, &RenderOptions { out, shots, ..opts.clone() })
        })
        .collect()
}

pub fn run(project: &Project, tl: &Timeline, clips: &HashMap<String, PathBuf>, opts: &RenderOptions) -> Result<PathBuf> {
    let shots = select(tl, &opts.shots)?;
    if shots.is_empty() {
        bail!("no shots to render");
    }
    let style = CaptionStyle::load(&caption_font(), tl.height, tl.width, opts.show_speaker)?;
    let cache = project.path("work/render");
    fs::create_dir_all(&cache)?;
    // The path depends on neighboring shots: computed over the whole timeline, even for a preview.
    let scroll = match opts.style {
        Style::Scroll => {
            let active: Vec<&Shot> = tl.shots.iter().filter(|s| !s.skip).collect();
            scroll_path(&active, |page| image::image_dimensions(project.path("pages").join(page)).ok(), (tl.width, tl.height))
        }
        Style::Panels | Style::Anime => HashMap::new(),
    };

    let mut todo = Vec::new();
    let mut parts = Vec::new();
    for shot in &shots {
        let clip = clips.get(&shot.id).map(PathBuf::as_path);
        let path_y = scroll.get(&shot.id).copied();
        let key = cache_key(project, tl, shot, clip, path_y, opts)?;
        let path = cache.join(format!("{}-{key}.mp4", shot.id));
        if !path.exists() {
            todo.push((*shot, clip, path_y, path.clone()));
        }
        parts.push(path);
    }
    println!("{} shots, {} to render, duration {:.0}s", shots.len(), todo.len(), shots.iter().map(|s| s.duration).sum::<f64>());

    let bar = indicatif::ProgressBar::new(todo.len() as u64);
    let next = AtomicUsize::new(0);
    let errors = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..opts.jobs.max(1) {
            scope.spawn(|| {
                while let Some((shot, clip, path_y, path)) = todo.get(next.fetch_add(1, Ordering::SeqCst)) {
                    let tmp = path.with_extension("tmp.mp4");
                    match render_shot(project, tl, shot, *clip, *path_y, &style, opts, &tmp).and_then(|_| Ok(fs::rename(&tmp, path)?)) {
                        Ok(()) => bar.inc(1),
                        Err(e) => errors.lock().unwrap().push(e),
                    }
                }
            });
        }
    });
    bar.finish_and_clear();
    if let Some(e) = errors.into_inner().unwrap().into_iter().next() {
        return Err(e);
    }

    let list = cache.join("concat.txt");
    let body: String = parts.iter().map(|p| format!("file '{}'\n", p.display().to_string().replace('\'', r"'\''"))).collect();
    fs::write(&list, body)?;
    if let Some(parent) = opts.out.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&opts.out)
        .status()?;
    if !status.success() {
        bail!("ffmpeg concatenation failed");
    }
    // Shots no longer in the timeline clutter the cache.
    let keep: Vec<&PathBuf> = parts.iter().collect();
    let all_ids: Vec<&str> = tl.shots.iter().map(|s| s.id.as_str()).collect();
    for entry in fs::read_dir(&cache)?.flatten() {
        let p = entry.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let id = name.rsplit_once('-').map(|(id, _)| id).unwrap_or("");
        if name.ends_with(".mp4") && !keep.contains(&&p) && (!all_ids.contains(&id) || opts.shots.is_empty()) {
            fs::remove_file(&p).ok();
        }
    }
    Ok(opts.out.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_choices() {
        let frame = (1920, 1080);
        // Square panel, zoom: fits in 92% of the height.
        assert_eq!(camera(1000, 1000, frame, Motion::ZoomIn, false), Camera::Zoom { fw: 994, fh: 994, from: 1.0, to: 1.1 });
        // Very tall panel: vertical pan over an enlarged version.
        match camera(400, 1600, frame, Motion::PanDown, false) {
            Camera::Pan { sw, sh, vh, axis_x: false, reverse: false, .. } => {
                assert!(sh > vh && sw == 398);
            }
            other => panic!("{other:?}"),
        }
        // Pan requested with no room to slide: falls back to a zoom.
        assert!(matches!(camera(1000, 1000, frame, Motion::PanLeft, false), Camera::Zoom { .. }));
    }

    #[test]
    fn graph_wires_captions_and_audio() {
        let cam = Camera::Zoom { fw: 994, fh: 994, from: 1.0, to: 1.1 };
        let caps = [CaptionPlacement { w: 800, h: 100, start: 0.4, end: 2.0 }];
        let g = filter_graph(&cam, 3.0, 30, (1920, 1080), &caps, &[0.4], false);
        assert!(g.contains("zoompan=z='1+(1.1-1)*on/90'"));
        assert!(g.contains("[v0][1:v]overlay=x=560:y=932:enable='between(t\\,0.400\\,2.000)'[v1]"));
        assert!(g.contains("[2:a]aresample=48000"));
        assert!(g.contains("adelay=400:all=1"));
        assert!(g.contains("[v1]format=yuv420p[vout]"));
        let silent = filter_graph(&cam, 3.0, 30, (1920, 1080), &[], &[], false);
        assert!(silent.ends_with("anullsrc=r=48000:cl=stereo[aout]"));
    }

    fn shot(id: &str, page: &str, bbox: [f64; 4]) -> Shot {
        Shot {
            id: id.into(),
            page: page.into(),
            bbox,
            duration: 3.0,
            motion: Motion::Static,
            mode: crate::timeline::Mode::Motion,
            prompt: None,
            skip: false,
            extra: 0.0,
            sound: None,
            characters: Vec::new(),
            lines: Vec::new(),
        }
    }

    #[test]
    fn anime_clip_fills_the_frame_and_holds_its_last_image() {
        let g = filter_graph(&Camera::Cover, 6.0, 30, (1920, 1080), &[], &[], false);
        assert!(g.starts_with("[0:v]scale=1920:1080:force_original_aspect_ratio=increase:flags=lanczos,crop=1920:1080,fps=30,tpad=stop_mode=clone:stop=-1,setsar=1[v0];"));
        assert!(!g.contains("gblur"), "no blurred background");
    }

    #[test]
    fn anime_still_fills_the_frame() {
        let g = filter_graph(&Camera::CoverStill { from: 1.0, to: 1.08 }, 3.0, 30, (1920, 1080), &[], &[], false);
        assert!(g.contains("crop=3840:2160,zoompan=z='1+(1.08-1)*on/90'"));
        assert!(g.contains(":s=1920x1080:fps=30,setsar=1[v0];"));
        assert!(!g.contains("gblur"), "no blurred background");
    }

    #[test]
    fn clip_sound_ducks_under_the_voices() {
        let g = filter_graph(&Camera::Cover, 3.0, 30, (1920, 1080), &[], &[0.4], true);
        assert!(g.contains("[0:a]loudnorm"));
        assert!(g.contains("[bed][key]sidechaincompress"));
        assert!(g.contains("[vo][fx]amix=inputs=2"));
        assert!(g.ends_with("apad[aout]"));
    }

    #[test]
    fn clip_sound_plays_alone_on_a_silent_shot() {
        let g = filter_graph(&Camera::Cover, 3.0, 30, (1920, 1080), &[], &[], true);
        assert!(g.contains("[0:a]loudnorm") && g.ends_with("apad[aout]"));
        assert!(!g.contains("anullsrc") && !g.contains("sidechain"));
    }

    #[test]
    fn vertical_keeps_captions_above_the_app_buttons() {
        let cam = Camera::Cover;
        let caps = [CaptionPlacement { w: 800, h: 100, start: 0.0, end: 1.0 }];
        let g = filter_graph(&cam, 3.0, 30, (1080, 1920), &caps, &[], false);
        assert!(g.contains("overlay=x=140:y=1513"), "{g}");
        assert!(g.contains("scale=1080:1920:force_original_aspect_ratio=increase"));
    }

    fn timed(id: &str, duration: f64) -> Shot {
        Shot { duration, ..shot(id, "1.png", [0.0, 0.0, 1.0, 1.0]) }
    }

    #[test]
    fn parts_never_cut_a_shot() {
        let shots = [timed("a", 20.0), timed("b", 30.0), timed("c", 20.0), timed("d", 90.0), timed("e", 5.0)];
        let refs: Vec<&Shot> = shots.iter().collect();
        assert_eq!(parts(&refs, Some(60.0), true), [vec!["a", "b"], vec!["c"], vec!["d"], vec!["e"]]);
        assert_eq!(parts(&refs, Some(60.0), false), [vec!["a", "b"]], "without split, the first part only");
        assert_eq!(parts(&refs, None, false).len(), 1);
    }

    #[test]
    fn loudness_is_read_from_the_ffmpeg_report() {
        assert_eq!(max_volume("[Parsed_volumedetect_0 @ 0x1] mean_volume: -22.3 dB\n[Parsed_volumedetect_0 @ 0x1] max_volume: -14.9 dB\n"), Some(-14.9));
        assert_eq!(max_volume("no audio here"), None);
    }

    #[test]
    fn part_files_are_numbered() {
        assert_eq!(part_path(Path::new("/o/a-vertical.mp4"), 2), Path::new("/o/a-vertical-part2.mp4"));
    }

    #[test]
    fn scroll_follows_panels_down_the_page() {
        // 1000×1500 page → 1920×2880 in the frame: 1800 px of possible scroll.
        let shots = [
            shot("p1_page", "1.png", [0.0, 0.0, 1000.0, 1500.0]),
            shot("p1_k0", "1.png", [0.0, 0.0, 1000.0, 400.0]),
            shot("p1_k1", "1.png", [500.0, 420.0, 1000.0, 800.0]),
            shot("p1_k2", "1.png", [0.0, 420.0, 480.0, 800.0]),
            shot("p1_k3", "1.png", [0.0, 820.0, 1000.0, 1500.0]),
            shot("p2_k0", "2.png", [0.0, 0.0, 3000.0, 1500.0]),
        ];
        let refs: Vec<&Shot> = shots.iter().collect();
        let size = |p: &str| Some(if p == "1.png" { (1000, 1500) } else { (3000, 1500) });
        let path = scroll_path(&refs, size, (1920, 1080));
        assert_eq!(path["p1_page"], (0.0, 0.0), "overview shot: top of the page");
        // Row of panels 1 and 2 (806–1536 px), centered in the frame.
        let row = (806.4 + 1536.0) / 2.0 - 540.0;
        // Panel 0 (0–768 px) already visible at the top: no move above 0, then halfway.
        assert_eq!(path["p1_k0"], (0.0, row / 2.0));
        // Panels 1 and 2 on the same row: the camera arrives during 1, leaves during 2.
        assert_eq!(path["p1_k1"], (row / 2.0, row));
        assert_eq!(path["p1_k2"].0, row);
        // Last panel taller than the frame: traversed to the bottom of the page.
        assert_eq!(path["p1_k3"].1, 1800.0);
        assert!(!path.contains_key("p2_k0"), "double-page spread: no scrolling");
    }

    #[test]
    fn scroll_graph_crops_without_background() {
        let g = filter_graph(&Camera::Scroll { y0: 10.0, y1: 250.0 }, 4.0, 30, (1920, 1080), &[], &[], false);
        assert!(g.starts_with("[0:v]fps=30,crop=1920:1080:x=0:y='10.00+(240.00)*min(t/4.000\\,1)',setsar=1[v0];"));
        assert!(!g.contains("gblur"));
    }

    #[test]
    fn scroll_band_keeps_path_inside() {
        let page = DynamicImage::new_rgb8(1000, 1500);
        let (band, cam) = scroll_band(&page, (1920, 1080), (500.0, 900.0));
        assert_eq!(band.width(), 1920);
        let Camera::Scroll { y0, y1 } = cam else { panic!("{cam:?}") };
        assert!(y0 >= 0.0 && (y1 - y0 - 400.0).abs() < 1e-9);
        assert!(y1 + 1080.0 <= band.height() as f64);
    }
}
