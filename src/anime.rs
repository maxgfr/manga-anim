//! `manga anime`: the last step. Each panel of an analyzed and voiced project is repainted as a
//! clean 16:9 key image (no bubbles, no lettering), animated for as long as its shot lasts, and
//! rendered full-frame with the voices and subtitles. `timeline.json` only gains motion prompts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::animate::{self, Settings};
use crate::backend::{Backend, Review};
use crate::characters;
use crate::project::Project;
use crate::render::{self, RenderOptions, Style, crop_panel};
use crate::story::{self, DrawItem};
use crate::timeline::{Mode, Shot, Timeline};
use crate::worker;

const KEY_WIDTH: u32 = 1280;
const KEY_HEIGHT: u32 = 720;
const KEY_SEED: u64 = 42;
/// What `manga story build` uses with the local model.
const KEY_STEPS: u32 = 4;
/// Character references per key image, on top of the panel itself.
const MAX_CHARACTER_REFS: usize = 3;
const LOCAL_MAX_AREA: u32 = 640 * 352;
const REMOTE_MAX_AREA: u32 = KEY_WIDTH * KEY_HEIGHT;

const COLOR: &str = "Repaint this manga panel as a full-color still from a modern Japanese TV anime: clean line art, cel shading, natural colors.";
const BW: &str = "Redraw this manga panel as a clean black-and-white widescreen anime still: ink line art and screentone shading, no color at all.";
const CLEANUP: &str = "Remove every speech bubble, caption, sound effect and letter, and paint what is behind them. \
Extend the scene to fill a 16:9 widescreen frame. Keep the same characters, faces, clothing, poses and composition. No text anywhere.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AnimeStyle {
    /// Full-color anime stills.
    Color,
    /// Clean black-and-white stills.
    Bw,
}

pub struct Options {
    pub image: Backend,
    pub video: Backend,
    pub style: AnimeStyle,
    pub look: Option<String>,
    pub director: Option<Review>,
    pub shots: Vec<String>,
    pub key_shots_only: bool,
    pub keyframes_only: bool,
    pub max_seconds: Option<f64>,
    pub dry_run: bool,
    pub max_area: Option<u32>,
    pub out: Option<PathBuf>,
    pub jobs: usize,
}

/// `--direct-provider` and `--direct-model`. Claude writes prompts through the skill, for free.
pub fn parse_director(provider: Option<String>, model: Option<String>) -> Result<Option<Review>> {
    if provider.as_deref() == Some("anthropic") {
        bail!("--direct-provider takes openrouter, openai or google; with Claude, let the manga skill write the prompts");
    }
    Review::parse("api", provider, model)
}

fn targets<'a>(tl: &'a Timeline, wanted: &[String]) -> Result<Vec<&'a Shot>> {
    let shots: Vec<&Shot> = render::select(tl, wanted)?.into_iter().filter(|s| !s.id.ends_with("_page")).collect();
    if shots.is_empty() {
        bail!("no panel shot to turn into anime");
    }
    Ok(shots)
}

fn keyframe_prompt(style: AnimeStyle, look: Option<&str>, characters: &[&str]) -> String {
    let mut parts = vec![match style {
        AnimeStyle::Color => COLOR.to_string(),
        AnimeStyle::Bw => BW.to_string(),
    }];
    parts.push(CLEANUP.to_string());
    if !characters.is_empty() {
        parts.push(format!("The first reference image is the panel to repaint; the others show these characters, in order: {}.", characters.join(", ")));
    }
    if let Some(look) = look.map(str::trim).filter(|l| !l.is_empty()) {
        parts.push(format!("Appearance: {look}."));
    }
    parts.join(" ")
}

/// The bank portraits of the shot's speakers, in order of first line.
fn speaker_refs<'a>(shot: &Shot, bank: &'a [(String, PathBuf)]) -> Vec<&'a (String, PathBuf)> {
    let mut refs: Vec<&(String, PathBuf)> = Vec::new();
    for speaker in shot.lines.iter().filter_map(|l| l.speaker.as_deref()) {
        if let Some(entry) = bank.iter().find(|(name, _)| name.eq_ignore_ascii_case(speaker))
            && refs.len() < MAX_CHARACTER_REFS
            && !refs.iter().any(|r| r.0 == entry.0)
        {
            refs.push(entry);
        }
    }
    refs
}

struct KeyPlan {
    paths: HashMap<String, PathBuf>,
    todo: Vec<DrawItem>,
}

/// Saves each panel for the workers and finds which key images are missing.
fn plan_keyframes(project: &Project, shots: &[&Shot], opts: &Options) -> Result<KeyPlan> {
    let bank = characters::bank(project);
    let work = project.path("work/anime");
    fs::create_dir_all(&work)?;
    fs::create_dir_all(project.path("keyframes"))?;
    let mut pages = HashMap::new();
    let mut plan = KeyPlan { paths: HashMap::new(), todo: Vec::new() };
    for shot in shots {
        if !pages.contains_key(&shot.page) {
            let img = image::open(project.path("pages").join(&shot.page)).with_context(|| format!("unreadable page: {}", shot.page))?;
            pages.insert(shot.page.clone(), img);
        }
        let panel = crop_panel(&pages[&shot.page], &shot.bbox);
        let source = work.join(format!("{}.png", shot.id));
        panel.save(&source)?;
        let refs = speaker_refs(shot, &bank);
        let names: Vec<&str> = refs.iter().map(|r| r.0.as_str()).collect();
        let prompt = keyframe_prompt(opts.style, opts.look.as_deref(), &names);
        let mut h = Sha256::new();
        h.update(panel.as_bytes());
        h.update(format!("{prompt}|{KEY_WIDTH}x{KEY_HEIGHT}|{KEY_SEED}|{KEY_STEPS}{}", opts.image.cache_suffix()));
        for (_, path) in &refs {
            h.update(fs::read(path).with_context(|| format!("unreadable portrait: {}", path.display()))?);
        }
        let out = project.path(format!("keyframes/{}.png", &hex::encode(h.finalize())[..16]));
        if !out.exists() {
            let mut sources = vec![source.to_string_lossy().to_string()];
            sources.extend(refs.iter().map(|r| r.1.to_string_lossy().to_string()));
            plan.todo.push(DrawItem {
                id: shot.id.clone(),
                prompt,
                refs: sources,
                width: KEY_WIDTH,
                height: KEY_HEIGHT,
                seed: KEY_SEED,
                steps: KEY_STEPS,
                out: out.to_string_lossy().into(),
            });
        }
        plan.paths.insert(shot.id.clone(), out);
    }
    Ok(plan)
}

/// Models bill whole seconds: this is the most a shot can cost, short of a model's minimum length.
fn billed_seconds(duration: f64) -> f64 {
    duration.ceil()
}

fn marked(tl: &Timeline, ids: &[String], key_shots_only: bool) -> Timeline {
    let mut out = tl.clone();
    if !key_shots_only {
        out.shots.iter_mut().filter(|s| ids.contains(&s.id)).for_each(|s| s.mode = Mode::Animate);
    }
    out
}

fn clip_settings(opts: &Options, keyframes: HashMap<String, PathBuf>) -> Settings {
    let default_area = if opts.video.is_local() { LOCAL_MAX_AREA } else { REMOTE_MAX_AREA };
    Settings { max_area: opts.max_area.unwrap_or(default_area), backend: opts.video.clone(), keyframes, anime: true, ..Default::default() }
}

/// The ids of the shots that would pay for a clip: no key image yet, no prompt yet, or no clip yet.
fn clips_to_generate(project: &Project, tl: &Timeline, ids: &[String], opts: &Options, plan: &KeyPlan) -> Result<Vec<String>> {
    let shots: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id) && s.mode == Mode::Animate).collect();
    let undecided = |s: &Shot| !plan.paths[&s.id].exists() || (opts.director.is_some() && s.prompt.is_none());
    let (open, known): (Vec<&Shot>, Vec<&Shot>) = shots.iter().copied().partition(|s| undecided(s));
    let mut pending: Vec<String> = open.iter().map(|s| s.id.clone()).collect();
    pending.extend(animate::pending(project, &known, &clip_settings(opts, plan.paths.clone()))?);
    Ok(pending)
}

#[derive(Serialize)]
struct DirectItem {
    id: String,
    image: String,
    lines: Vec<String>,
}

/// Never holds an API key: the worker reads it from the environment.
#[derive(Serialize)]
struct DirectJob<'a> {
    provider: &'a str,
    model: &'a str,
    lang: &'a str,
    items: Vec<DirectItem>,
    out: String,
}

/// Fills the empty motion prompts of `ids`. Manual prompts are never replaced.
fn direct(project: &Project, tl: &mut Timeline, ids: &[String], director: &Review) -> Result<()> {
    let work = project.path("work/anime");
    let items: Vec<DirectItem> = tl
        .shots
        .iter()
        .filter(|s| ids.contains(&s.id) && s.prompt.is_none())
        .map(|s| DirectItem { id: s.id.clone(), image: work.join(format!("{}.png", s.id)).to_string_lossy().into(), lines: s.lines.iter().map(|l| l.text.clone()).collect() })
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    println!("{} shots to direct with {} on {} (billed per use; panels are sent to this provider)", items.len(), director.model, director.provider);
    let out = work.join("direction.json");
    let job = DirectJob { provider: &director.provider, model: &director.model, lang: &project.cfg.lang, items, out: out.to_string_lossy().into() };
    worker::run("direct", &job, &project.path("work/direct_job.json"))?;
    apply_direction(tl, &read_direction(&out)?);
    tl.save(project)
}

fn read_direction(path: &Path) -> Result<HashMap<String, String>> {
    let text = fs::read_to_string(path).with_context(|| format!("direction not written: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid direction: {}", path.display()))
}

fn apply_direction(tl: &mut Timeline, prompts: &HashMap<String, String>) {
    for shot in tl.shots.iter_mut().filter(|s| s.prompt.is_none()) {
        shot.prompt = prompts.get(&shot.id).cloned();
    }
}

pub fn run(project: &Project, opts: &Options) -> Result<()> {
    let mut tl = Timeline::load(project)?;
    let ids: Vec<String> = targets(&tl, &opts.shots)?.iter().map(|s| s.id.clone()).collect();
    let selected: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id)).collect();
    let plan = plan_keyframes(project, &selected, opts)?;
    let planned = marked(&tl, &ids, opts.key_shots_only);
    let clips = if opts.keyframes_only { Vec::new() } else { clips_to_generate(project, &planned, &ids, opts, &plan)? };
    // Not sum(): an empty float sum is -0.0, printed as "-0".
    let seconds = planned.shots.iter().filter(|s| clips.contains(&s.id)).fold(0.0, |total, s| total + billed_seconds(s.duration));
    println!("{} shots: {} key images to draw, {} clips to generate (about {seconds:.0} s of video)", ids.len(), plan.todo.len(), clips.len());
    if opts.dry_run {
        return Ok(());
    }
    if let Some(max) = opts.max_seconds.filter(|max| seconds > *max) {
        bail!("{seconds:.0} s of video to generate is over --max-seconds {max:.0}: use --shots, --key-shots-only or a higher limit");
    }
    // Every key is checked first: a missing video key must not surface after paying for the images.
    if !plan.todo.is_empty() {
        opts.image.check_key()?;
    }
    if !clips.is_empty() {
        opts.video.check_key()?;
    }
    if let Some(director) = opts.director.as_ref().filter(|_| !opts.keyframes_only) {
        director.check_key()?;
        // A still shot has no use for a motion prompt: do not pay for one.
        let animated: Vec<String> = planned.shots.iter().filter(|s| ids.contains(&s.id) && s.mode == Mode::Animate).map(|s| s.id.clone()).collect();
        direct(project, &mut tl, &animated, director)?;
    }

    let keyframes = plan.paths;
    if !plan.todo.is_empty() {
        story::draw_items(&project.root, &opts.image, plan.todo)?;
    }
    for (id, path) in &keyframes {
        if !path.exists() {
            bail!("key image not generated for shot {id}");
        }
    }
    if opts.keyframes_only {
        for id in &ids {
            println!("{id}: {}", keyframes[id].display());
        }
        return Ok(());
    }

    let tl = marked(&tl, &ids, opts.key_shots_only);
    let chosen: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id)).collect();
    let clips = animate::ensure_clips(project, &chosen, &clip_settings(opts, keyframes.clone()))?;
    let out = opts.out.clone().unwrap_or_else(|| project.path(format!("out/{}-anime.mp4", project.cfg.name)));
    let render_opts = RenderOptions { out, style: Style::Anime, shots: ids, show_speaker: false, captions: true, jobs: opts.jobs, keyframes };
    let path = render::run(project, &tl, &clips, &render_opts)?;
    println!("anime: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Line, Motion};

    fn shot(id: &str, speakers: &[&str]) -> Shot {
        Shot {
            id: id.into(),
            page: "0001.png".into(),
            bbox: [0.0, 0.0, 10.0, 10.0],
            duration: 2.2,
            motion: Motion::Static,
            mode: Mode::Motion,
            prompt: None,
            skip: false,
            extra: 0.0,
            lines: speakers.iter().map(|s| Line { text_id: "t".into(), text: "hi".into(), speaker: Some(s.to_string()), start: 0.0, duration: 1.0, audio: None }).collect(),
        }
    }

    fn timeline(shots: Vec<Shot>) -> Timeline {
        Timeline { width: 1920, height: 1080, fps: 30, shots }
    }

    #[test]
    fn page_overviews_never_become_anime_shots() {
        let tl = timeline(vec![shot("p0001_page", &[]), shot("p0001_k0", &[])]);
        let ids: Vec<&str> = targets(&tl, &[]).unwrap().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["p0001_k0"]);
        assert!(targets(&tl, &["1".to_string()]).is_err(), "only an overview selected");
    }

    #[test]
    fn key_shots_only_keeps_the_marks() {
        let mut key = shot("p0001_k1", &[]);
        key.mode = Mode::Animate;
        let tl = timeline(vec![shot("p0001_k0", &[]), key]);
        let ids = vec!["p0001_k0".to_string(), "p0001_k1".to_string()];
        let only = marked(&tl, &ids, true);
        assert_eq!(only.shots[0].mode, Mode::Motion);
        assert_eq!(only.shots[1].mode, Mode::Animate);
        assert!(marked(&tl, &ids, false).shots.iter().all(|s| s.mode == Mode::Animate));
    }

    #[test]
    fn styles_ask_for_different_stills() {
        let color = keyframe_prompt(AnimeStyle::Color, None, &[]);
        let bw = keyframe_prompt(AnimeStyle::Bw, None, &[]);
        assert!(color.contains("full-color") && bw.contains("no color"));
        assert!(color.contains("speech bubble") && bw.contains("speech bubble"));
    }

    #[test]
    fn look_and_characters_reach_the_prompt() {
        let prompt = keyframe_prompt(AnimeStyle::Color, Some(" Kit: red hair "), &["Kit", "Ann"]);
        assert!(prompt.contains("in order: Kit, Ann."));
        assert!(prompt.contains("Appearance: Kit: red hair."));
    }

    #[test]
    fn speakers_are_found_once_and_capped() {
        let bank: Vec<(String, PathBuf)> = ["Ann", "Bob", "Cid", "Dan"].iter().map(|n| (n.to_string(), PathBuf::from(format!("{n}.png")))).collect();
        let s = shot("k", &["ann", "ann", "Unknown", "Bob", "Cid", "Dan"]);
        let names: Vec<&str> = speaker_refs(&s, &bank).iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["Ann", "Bob", "Cid"]);
    }

    #[test]
    fn direction_never_replaces_a_manual_prompt() {
        let mut manual = shot("a", &[]);
        manual.prompt = Some("mine".into());
        let mut tl = timeline(vec![manual, shot("b", &[]), shot("c", &[])]);
        let prompts = HashMap::from([("a".to_string(), "theirs".to_string()), ("b".to_string(), "new".to_string())]);
        apply_direction(&mut tl, &prompts);
        assert_eq!(tl.shots[0].prompt.as_deref(), Some("mine"));
        assert_eq!(tl.shots[1].prompt.as_deref(), Some("new"));
        assert_eq!(tl.shots[2].prompt, None);
    }

    #[test]
    fn anthropic_director_is_refused_with_the_way_out() {
        let err = parse_director(Some("anthropic".into()), Some("m".into())).unwrap_err().to_string();
        assert!(err.contains("manga skill"), "{err}");
        assert_eq!(parse_director(None, None).unwrap(), None);
    }

    #[test]
    fn seconds_are_rounded_up_like_the_bill() {
        assert_eq!(billed_seconds(2.2), 3.0);
        assert_eq!(billed_seconds(3.0), 3.0);
    }
}
