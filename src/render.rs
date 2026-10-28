//! Rendu de la timeline en vidéo : un MP4 par plan (mis en cache), puis concaténation.
//!
//! Deux styles. `scroll` (défaut) : la page en pleine largeur défile de case en case au fil
//! de la lecture, comme une vidéo de lecture YouTube. `panels` : la case recadrée, animée par
//! la caméra (zoom ou panoramique), posée sur un fond flou. Dans les deux cas un clip généré
//! remplace la case animée, avec sous-titres et voix.

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

/// À incrémenter quand le rendu d'un plan change, pour invalider le cache.
const RENDER_VERSION: u32 = 3;
const FIT: f64 = 0.92;
const PAN_ZOOM: f64 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Style {
    /// La page en pleine largeur défile de case en case pendant la lecture.
    Scroll,
    /// Une case à la fois sur fond flou, avec zoom ou panoramique.
    Panels,
}

pub struct RenderOptions {
    pub out: PathBuf,
    pub style: Style,
    /// Identifiants ou numéros (1 = premier plan) à rendre ; vide = tous.
    pub shots: Vec<String>,
    pub show_speaker: bool,
    pub captions: bool,
    pub jobs: usize,
}

/// Recadre la case avec une petite marge.
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

/// Comment la case occupe l'image : zoompan sur une image fixe, ou fenêtre glissante.
#[derive(Debug, PartialEq)]
pub enum Camera {
    /// Image entière visible (fw×fh), zoom de `from` à `to`.
    Zoom { fw: u32, fh: u32, from: f64, to: f64 },
    /// Image agrandie à sw×sh, fenêtre vw×vh qui glisse. `axis_x` : sens horizontal.
    Pan { sw: u32, sh: u32, vw: u32, vh: u32, axis_x: bool, reverse: bool },
    /// Clip vidéo mis à l'échelle fw×fh.
    Clip { fw: u32, fh: u32 },
    /// Bande de page déjà à la largeur du cadre, fenêtre plein cadre dont le haut va de y0 à y1.
    Scroll { y0: f64, y1: f64 },
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

/// Filtergraph ffmpeg d'un plan. Entrées : 0 = case (image ou clip), 1..=n = sous-titres,
/// puis les audios.
pub fn filter_graph(cam: &Camera, duration: f64, fps: u32, frame: (u32, u32), captions: &[CaptionPlacement], audio_starts: &[f64]) -> String {
    let (w, h) = frame;
    let frames = (duration * fps as f64).ceil() as u32;
    let ease = format!("(1-cos(PI*min(t/{duration:.3}\\,1)))/2");
    let mut g = String::new();
    match cam {
        Camera::Zoom { fw, fh, from, to } => {
            // Image source à 2× pour éviter le tremblement du zoompan sur les petits pas.
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
        // Vitesse constante sur le plan : le défilement suit la lecture, sans à-coups.
        Camera::Scroll { y0, y1 } => {
            write!(g, "[0:v]fps={fps},crop={w}:{h}:x=0:y='{y0:.2}+({:.2})*min(t/{duration:.3}\\,1)',setsar=1[v0];", y1 - y0).unwrap();
        }
    }
    if !matches!(cam, Camera::Scroll { .. }) {
        write!(
            g,
            "[fg]split[fga][fgb];\
             [fgb]scale=480:270:force_original_aspect_ratio=increase,crop=480:270,gblur=sigma=10,\
             colorchannelmixer=rr=0.45:gg=0.45:bb=0.45,scale={w}:{h},setsar=1[bg];\
             [bg][fga]overlay=x=(W-w)/2:y=(H-h)/2[v0];"
        )
        .unwrap();
    }
    let margin = (h as f64 * 0.045) as u32;
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
    if audio_starts.is_empty() {
        write!(g, ";anullsrc=r=48000:cl=stereo[aout]").unwrap();
    } else {
        for (i, start) in audio_starts.iter().enumerate() {
            let ms = (start * 1000.0).round() as u64;
            write!(g, ";[{}:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,adelay={ms}:all=1[a{i}]", first_audio + i).unwrap();
        }
        let inputs: String = (0..audio_starts.len()).map(|i| format!("[a{i}]")).collect();
        write!(g, ";{inputs}amix=inputs={}:normalize=0:duration=longest,volume=1.8,apad[aout]", audio_starts.len()).unwrap();
    }
    g
}

/// Trajet de la caméra en style scroll : pour chaque plan, le haut de la fenêtre au début et
/// à la fin, en pixels de la page mise à la largeur du cadre.
///
/// Chaque case passe au centre pendant sa lecture (une case plus haute que le cadre est
/// parcourue de haut en bas), la caméra glisse vers la suivante à mi-chemin entre deux plans
/// et ne remonte jamais. Le plan d'ensemble d'une page tient le haut de la page. Une page
/// trop basse pour défiler (planche double) n'a pas de trajet : rendu en style panels.
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

/// Bande de la page couvrant le trajet, mise à la largeur du cadre, et le trajet ramené à la bande.
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
            let m = fs::metadata(project.path(a)).with_context(|| format!("audio manquant : {a} (manga voice …)"))?;
            h.update(format!("{a}:{}:{:?}", m.len(), m.modified().ok()));
        }
    }
    if let Some(c) = clip {
        h.update(c.to_string_lossy().as_bytes());
    }
    Ok(hex::encode(h.finalize())[..12].to_string())
}

#[allow(clippy::too_many_arguments)]
fn render_shot(project: &Project, tl: &Timeline, shot: &Shot, clip: Option<&Path>, scroll: Option<(f64, f64)>, style: &CaptionStyle, opts: &RenderOptions, out: &Path) -> Result<()> {
    let work = out.with_extension("parts");
    fs::create_dir_all(&work)?;
    let page = image::open(project.path("pages").join(&shot.page)).with_context(|| format!("page illisible : {}", shot.page))?;
    let frame = (tl.width, tl.height);
    let panel_path = work.join("panel.png");
    let cam = match scroll.filter(|_| clip.is_none()) {
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
    let graph = filter_graph(&cam, shot.duration, tl.fps, frame, &placements, &starts);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    let dur = format!("{:.3}", shot.duration);
    match (&cam, clip) {
        (Camera::Clip { .. }, Some(c)) => cmd.args(["-stream_loop", "-1", "-t", &dur, "-i"]).arg(c),
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
    let output = cmd.output().context("ffmpeg introuvable (brew install ffmpeg)")?;
    if !output.status.success() {
        fs::write(work.join("filter.txt"), &graph).ok();
        bail!("ffmpeg a échoué sur le plan {} : {}", shot.id, String::from_utf8_lossy(&output.stderr).trim());
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
            _ => tl.shots.iter().find(|s| &s.id == w).with_context(|| format!("plan inconnu : {w}")),
        })
        .collect()
}

pub fn run(project: &Project, tl: &Timeline, clips: &HashMap<String, PathBuf>, opts: &RenderOptions) -> Result<PathBuf> {
    let shots = select(tl, &opts.shots)?;
    if shots.is_empty() {
        bail!("aucun plan à rendre");
    }
    let style = CaptionStyle::load(&caption_font(), tl.height, tl.width, opts.show_speaker)?;
    let cache = project.path("work/render");
    fs::create_dir_all(&cache)?;
    // Le trajet dépend des plans voisins : calculé sur toute la timeline, même pour un aperçu.
    let scroll = match opts.style {
        Style::Scroll => {
            let active: Vec<&Shot> = tl.shots.iter().filter(|s| !s.skip).collect();
            scroll_path(&active, |page| image::image_dimensions(project.path("pages").join(page)).ok(), (tl.width, tl.height))
        }
        Style::Panels => HashMap::new(),
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
    println!("{} plans, {} à rendre, durée {:.0}s", shots.len(), todo.len(), shots.iter().map(|s| s.duration).sum::<f64>());

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
        bail!("concaténation ffmpeg échouée");
    }
    // Les plans qui ne sont plus dans la timeline encombrent le cache.
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
        // Case carrée, zoom : tient dans 92 % de la hauteur.
        assert_eq!(camera(1000, 1000, frame, Motion::ZoomIn, false), Camera::Zoom { fw: 994, fh: 994, from: 1.0, to: 1.1 });
        // Case très haute : panoramique vertical sur une version agrandie.
        match camera(400, 1600, frame, Motion::PanDown, false) {
            Camera::Pan { sw, sh, vh, axis_x: false, reverse: false, .. } => {
                assert!(sh > vh && sw == 398);
            }
            other => panic!("{other:?}"),
        }
        // Panoramique demandé sans marge pour glisser : retombe sur un zoom.
        assert!(matches!(camera(1000, 1000, frame, Motion::PanLeft, false), Camera::Zoom { .. }));
    }

    #[test]
    fn graph_wires_captions_and_audio() {
        let cam = Camera::Zoom { fw: 994, fh: 994, from: 1.0, to: 1.1 };
        let caps = [CaptionPlacement { w: 800, h: 100, start: 0.4, end: 2.0 }];
        let g = filter_graph(&cam, 3.0, 30, (1920, 1080), &caps, &[0.4]);
        assert!(g.contains("zoompan=z='1+(1.1-1)*on/90'"));
        assert!(g.contains("[v0][1:v]overlay=x=560:y=932:enable='between(t\\,0.400\\,2.000)'[v1]"));
        assert!(g.contains("[2:a]aresample=48000"));
        assert!(g.contains("adelay=400:all=1"));
        assert!(g.contains("[v1]format=yuv420p[vout]"));
        let silent = filter_graph(&cam, 3.0, 30, (1920, 1080), &[], &[]);
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
            lines: Vec::new(),
        }
    }

    #[test]
    fn scroll_follows_panels_down_the_page() {
        // Page 1000×1500 → 1920×2880 dans le cadre : 1800 px de défilement possible.
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
        assert_eq!(path["p1_page"], (0.0, 0.0), "plan d'ensemble : haut de la page");
        // Rangée des cases 1 et 2 (806–1536 px), centrée dans le cadre.
        let row = (806.4 + 1536.0) / 2.0 - 540.0;
        // Case 0 (0–768 px) déjà visible en haut : pas de remontée sous 0, puis mi-chemin.
        assert_eq!(path["p1_k0"], (0.0, row / 2.0));
        // Cases 1 et 2 sur la même rangée : la caméra y arrive pendant la 1, repart pendant la 2.
        assert_eq!(path["p1_k1"], (row / 2.0, row));
        assert_eq!(path["p1_k2"].0, row);
        // Dernière case plus haute que le cadre : parcourue jusqu'au bas de la page.
        assert_eq!(path["p1_k3"].1, 1800.0);
        assert!(!path.contains_key("p2_k0"), "planche double : pas de défilement");
    }

    #[test]
    fn scroll_graph_crops_without_background() {
        let g = filter_graph(&Camera::Scroll { y0: 10.0, y1: 250.0 }, 4.0, 30, (1920, 1080), &[], &[]);
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
