//! Le montage : `timeline.json`, une liste ordonnée de plans (une case par plan).
//!
//! `manga timeline` la génère depuis l'analyse et les durées audio. Les réglages faits à
//! la main (mouvement, mode, prompt, skip, extra) sont conservés à la régénération,
//! plan par plan, tant que l'identifiant du plan existe encore.

use std::collections::HashMap;
use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::analysis::{BBox, PageAnalysis, TextBox, area};
use crate::project::{Direction, Project, stem};
use crate::voice::AudioIndex;

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
    /// Case fixe animée par la caméra (rapide).
    Motion,
    /// Case animée par un modèle image→vidéo (lent).
    Animate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub text_id: String,
    pub text: String,
    pub speaker: Option<String>,
    /// Début dans le plan, en secondes.
    pub start: f64,
    pub duration: f64,
    /// WAV relatif au projet, absent si pas de voix.
    pub audio: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    /// `p0003_k1` : page 0003, case 1. `p0003_page` : la page entière.
    pub id: String,
    pub page: String,
    pub bbox: BBox,
    pub duration: f64,
    pub motion: Motion,
    pub mode: Mode,
    #[serde(default)]
    pub prompt: Option<String>,
    /// Exclut le plan du rendu sans le supprimer (il reviendrait à la régénération).
    #[serde(default)]
    pub skip: bool,
    /// Secondes ajoutées (ou retirées) à la durée calculée.
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
    /// Durée du plan d'ensemble sur chaque nouvelle page (0 = pas de plan d'ensemble).
    pub page_intro: f64,
    /// Durée d'une case sans texte.
    pub silent: f64,
    /// Lecture sans voix : caractères par seconde.
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
        let text = fs::read_to_string(&path).with_context(|| format!("timeline absente : {} (manga timeline …)", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("timeline invalide : {}", path.display()))
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
        // Une « case » couvrant presque toute la page n'apporte rien comme plan d'ensemble.
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
    let audio = AudioIndex::load(project);
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
        TextBox { id: id.into(), bbox, panel, text: format!("texte {id}"), speaker: None, essential }
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
        assert_eq!(k0.lines.len(), 2, "la bulle non essentielle est ignorée");
        assert_eq!(k0.lines[1].start, opts.lead + 2.0 + opts.gap);
        assert!((k0.duration - (opts.lead + 2.0 + opts.gap + 1.0 + opts.tail)).abs() < 1e-9);
        // Bulle hors case : rattachée à la case la plus proche, durée de lecture sans voix.
        let k1 = &tl.shots[2];
        assert_eq!(k1.lines[0].text_id, "0001_t3");
        assert!(k1.lines[0].audio.is_none());
        assert_eq!(k1.motion, Motion::PanDown, "case haute → panoramique vertical");
        assert_eq!(k0.motion, Motion::PanLeft, "case très large en rtl → panoramique vers la gauche");
    }

    #[test]
    fn manual_edits_survive_rebuild() {
        let audio = AudioIndex::default();
        let opts = Options::default();
        let mut first = build(&project(), &[analysis()], &audio, &opts, None);
        first.shots[1].mode = Mode::Animate;
        first.shots[1].prompt = Some("vent dans les cheveux".into());
        first.shots[2].skip = true;
        first.shots[2].extra = 1.0;
        let base = first.shots[2].duration;
        let second = build(&project(), &[analysis()], &audio, &opts, Some(&first));
        assert_eq!(second.shots[1].mode, Mode::Animate);
        assert_eq!(second.shots[1].prompt.as_deref(), Some("vent dans les cheveux"));
        assert!(second.shots[2].skip);
        assert!((second.shots[2].duration - (base + 1.0)).abs() < 1e-9);
    }
}
