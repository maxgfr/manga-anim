//! The edit: `timeline.json`, an ordered list of shots (one panel per shot).
//!
//! `manga timeline` generates it from the analysis and the audio durations. Manual settings
//! (motion, mode, prompt, skip, extra) are preserved on regeneration,
//! shot by shot, as long as the shot's id still exists.

use std::collections::HashMap;
use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::analysis::{BBox, PageAnalysis, TextBox, area};
use crate::project::{Direction, Project, stem};
use crate::voice::{AudioIndex, Cast};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    Static,
    ZoomIn,
    ZoomOut,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Still panel animated by the camera (fast).
    Motion,
    /// Panel animated by an image-to-video model (slow).
    Animate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub text_id: String,
    pub text: String,
    pub speaker: Option<String>,
    /// Start within the shot, in seconds.
    pub start: f64,
    pub duration: f64,
    /// WAV relative to the project, absent if there is no voice.
    pub audio: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    /// `p0003_k1`: page 0003, panel 1. `p0003_page`: the whole page.
    pub id: String,
    pub page: String,
    pub bbox: BBox,
    pub duration: f64,
    pub motion: Motion,
    pub mode: Mode,
    #[serde(default)]
    pub prompt: Option<String>,
    /// Excludes the shot from the render without deleting it (it would come back on regeneration).
    #[serde(default)]
    pub skip: bool,
    /// Seconds added to (or removed from) the computed duration.
    #[serde(default)]
    pub extra: f64,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub shots: Vec<Shot>,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Duration of the overview shot on each new page (0 = no overview shot).
    pub page_intro: f64,
    /// Duration of a panel with no text.
    pub silent: f64,
    /// Reading without voice: characters per second.
    pub read_cps: f64,
    pub lead: f64,
    pub gap: f64,
    pub tail: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self { page_intro: 1.5, silent: 1.6, read_cps: 14.0, lead: 0.4, gap: 0.3, tail: 0.6 }
    }
}

impl Timeline {
    pub fn load(project: &Project) -> Result<Self> {
        let path = project.path("timeline.json");
        let text = fs::read_to_string(&path).with_context(|| format!("timeline not found: {} (manga timeline …)", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("invalid timeline: {}", path.display()))
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        fs::write(project.path("timeline.json"), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn total(&self) -> f64 {
        self.shots.iter().filter(|s| !s.skip).map(|s| s.duration).sum()
    }
}

fn choose_motion(bbox: &BBox, index: usize, direction: Direction) -> Motion {
    let ar = (bbox[2] - bbox[0]) / (bbox[3] - bbox[1]).max(1.0);
    if ar < 0.75 {
        Motion::PanDown
    } else if ar > 2.6 {
        match direction {
            Direction::Rtl => Motion::PanLeft,
            Direction::Ltr => Motion::PanRight,
        }
    } else if index.is_multiple_of(2) {
        Motion::ZoomIn
    } else {
        Motion::ZoomOut
    }
}

fn nearest_panel(t: &TextBox, panels: &[BBox]) -> Option<usize> {
    if let Some(p) = t.panel.filter(|&p| p < panels.len()) {
        return Some(p);
    }
    let (cx, cy) = ((t.bbox[0] + t.bbox[2]) / 2.0, (t.bbox[1] + t.bbox[3]) / 2.0);
    (0..panels.len()).min_by(|&a, &b| {
        let d = |p: &BBox| {
            let dx = (p[0] - cx).max(cx - p[2]).max(0.0);
            let dy = (p[1] - cy).max(cy - p[3]).max(0.0);
            dx * dx + dy * dy
        };
        d(&panels[a]).total_cmp(&d(&panels[b]))
    })
}

pub fn build(project: &Project, analyses: &[PageAnalysis], audio: &AudioIndex, opts: &Options, previous: Option<&Timeline>) -> Timeline {
    let mut shots = Vec::new();
    for (page, analysis) in project.cfg.pages.iter().zip(analyses) {
        let page_id = stem(&page.file);
        let full: BBox = [0.0, 0.0, analysis.width as f64, analysis.height as f64];
        // A "panel" covering nearly the whole page adds nothing as an overview shot.
        let panels: Vec<BBox> = if analysis.panels.is_empty() {
            vec![full]
        } else {
            analysis.panels.iter().map(|p| p.bbox).collect()
        };
        if opts.page_intro > 0.0 && !(panels.len() == 1 && area(&panels[0]) > 0.8 * area(&full)) {
            shots.push(Shot {
                id: format!("p{page_id}_page"),
                page: page.file.clone(),
                bbox: full,
                duration: opts.page_intro,
                motion: Motion::Static,
                mode: Mode::Motion,
                prompt: None,
                skip: false,
                extra: 0.0,
                lines: Vec::new(),
            });
        }
        let mut per_panel: Vec<Vec<&TextBox>> = vec![Vec::new(); panels.len()];
        for t in analysis.texts.iter().filter(|t| t.essential && t.text.chars().any(char::is_alphanumeric)) {
            if let Some(p) = nearest_panel(t, &panels) {
                per_panel[p].push(t);
            }
        }
        for (k, (bbox, texts)) in panels.iter().zip(per_panel).enumerate() {
            let mut lines = Vec::new();
            let mut cursor = opts.lead;
            for t in texts {
                let (duration, audio_path) = match audio.duration(&t.id) {
                    Some(d) => (d, Some(format!("audio/{}.wav", t.id))),
                    None => (0.8 + t.text.chars().count() as f64 / opts.read_cps, None),
                };
                lines.push(Line {
                    text_id: t.id.clone(),
                    text: t.text.clone(),
                    speaker: t.speaker.clone(),
                    start: cursor,
                    duration,
                    audio: audio_path,
                });
                cursor += duration + opts.gap;
            }
            let duration = if lines.is_empty() { opts.silent } else { cursor - opts.gap + opts.tail };
            shots.push(Shot {
                id: format!("p{page_id}_k{k}"),
                page: page.file.clone(),
                bbox: *bbox,
                duration,
                motion: choose_motion(bbox, shots.len(), project.cfg.direction),
                mode: Mode::Motion,
                prompt: None,
                skip: false,
                extra: 0.0,
                lines,
            });
        }
    }

    if let Some(prev) = previous {
        let old: HashMap<&str, &Shot> = prev.shots.iter().map(|s| (s.id.as_str(), s)).collect();
        for shot in &mut shots {
            if let Some(o) = old.get(shot.id.as_str()) {
                shot.motion = o.motion;
                shot.mode = o.mode;
                shot.prompt = o.prompt.clone();
                shot.skip = o.skip;
                shot.extra = o.extra;
            }
        }
    }
    for shot in &mut shots {
        shot.duration = (shot.duration + shot.extra).max(0.5);
    }
    Timeline { width: 1920, height: 1080, fps: 30, shots }
}

pub fn run(project: &Project, opts: &Options, reset: bool) -> Result<Timeline> {
    let analyses = project
        .cfg
        .pages
        .iter()
        .map(|p| PageAnalysis::load(&project.analysis_path(p)))
        .collect::<Result<Vec<_>>>()?;
    // Clips stay on disk for when voices are turned back on, but `enabled = false` means no voices.
    let audio = if Cast::load(project)?.enabled { AudioIndex::load(project) } else { AudioIndex::default() };
    let previous = if reset { None } else { Timeline::load(project).ok() };
    let timeline = build(project, &analyses, &audio, opts, previous.as_ref());
    timeline.save(project)?;
    Ok(timeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Panel;
    use crate::project::{PageRef, ProjectConfig};

    fn project() -> Project {
        Project {
            root: "/tmp/x".into(),
            cfg: ProjectConfig {
                name: "t".into(),
                title: "t".into(),
                lang: "fr".into(),
                direction: Direction::Rtl,
                from: "ch1".into(),
                to: "ch1".into(),
                pages: vec![PageRef { file: "0001.png".into(), chapter: 1.0, page: 1 }],
            },
        }
    }

    fn text(id: &str, panel: Option<usize>, bbox: BBox, essential: bool) -> TextBox {
        TextBox { id: id.into(), bbox, panel, text: format!("text {id}"), speaker: None, essential }
    }

    fn analysis() -> PageAnalysis {
        PageAnalysis {
            page: "0001.png".into(),
            width: 1000,
            height: 1500,
            panels: vec![Panel { bbox: [0.0, 0.0, 1000.0, 350.0] }, Panel { bbox: [0.0, 370.0, 400.0, 1500.0] }],
            texts: vec![
                text("0001_t0", Some(0), [10.0, 10.0, 100.0, 100.0], true),
                text("0001_t1", Some(0), [10.0, 110.0, 100.0, 200.0], true),
                text("0001_t2", Some(0), [500.0, 110.0, 600.0, 200.0], false),
                text("0001_t3", None, [420.0, 900.0, 500.0, 1000.0], true),
            ],
            characters: Vec::new(),
            detector: "test".into(),
        }
    }

    #[test]
    fn shots_follow_panels_and_audio() {
        let mut audio = AudioIndex::default();
        audio.clips.insert("0001_t0".into(), ("h".into(), 2.0));
        audio.clips.insert("0001_t1".into(), ("h".into(), 1.0));
        let opts = Options::default();
        let tl = build(&project(), &[analysis()], &audio, &opts, None);
        let ids: Vec<&str> = tl.shots.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["p0001_page", "p0001_k0", "p0001_k1"]);
        let k0 = &tl.shots[1];
        assert_eq!(k0.lines.len(), 2, "the non-essential bubble is ignored");
        assert_eq!(k0.lines[1].start, opts.lead + 2.0 + opts.gap);
        assert!((k0.duration - (opts.lead + 2.0 + opts.gap + 1.0 + opts.tail)).abs() < 1e-9);
        // Bubble outside any panel: attached to the nearest panel, reading duration without voice.
        let k1 = &tl.shots[2];
        assert_eq!(k1.lines[0].text_id, "0001_t3");
        assert!(k1.lines[0].audio.is_none());
        assert_eq!(k1.motion, Motion::PanDown, "tall panel → vertical pan");
        assert_eq!(k0.motion, Motion::PanLeft, "very wide panel in rtl → pan to the left");
    }

    #[test]
    fn manual_edits_survive_rebuild() {
        let audio = AudioIndex::default();
        let opts = Options::default();
        let mut first = build(&project(), &[analysis()], &audio, &opts, None);
        first.shots[1].mode = Mode::Animate;
        first.shots[1].prompt = Some("wind in the hair".into());
        first.shots[2].skip = true;
        first.shots[2].extra = 1.0;
        let base = first.shots[2].duration;
        let second = build(&project(), &[analysis()], &audio, &opts, Some(&first));
        assert_eq!(second.shots[1].mode, Mode::Animate);
        assert_eq!(second.shots[1].prompt.as_deref(), Some("wind in the hair"));
        assert!(second.shots[2].skip);
        assert!((second.shots[2].duration - (base + 1.0)).abs() < 1e-9);
    }
}
