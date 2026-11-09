//! `manga anime`: the last step. Each panel of an analyzed and voiced project is repainted as a
//! clean key image (no bubbles, no lettering), animated with its sound effects for as long as its
//! shot lasts, and rendered full-frame with the voices and subtitles. `timeline.json` only gains
//! what the direction writes.
//!
//! `anime.toml` is the project's style bible: the art style, the appearance notes and, in `anime/`,
//! a reference image and one remembered face per character. Every run reuses them, so a project
//! keeps the same look and the same faces, and `--style-from` passes them on to another project.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::animate::{self, Settings};
use crate::backend::{Backend, Review};
use crate::characters;
use crate::project::Project;
use crate::render::{self, Format, RenderOptions, Style, crop_panel};
use crate::story::{self, DrawItem};
use crate::timeline::{Mode, Shot, Timeline};
use crate::worker;

const KEY_LONG: u32 = 1280;
const KEY_SHORT: u32 = 720;
const FACE_SIZE: u32 = 1024;
const KEY_SEED: u64 = 42;
/// What `manga story build` uses with the local model.
const KEY_STEPS: u32 = 4;
/// Faces per key image, on top of the panel and the style reference.
const MAX_FACES: usize = 3;
const LOCAL_MAX_AREA: u32 = 640 * 352;
const REMOTE_MAX_AREA: u32 = KEY_LONG * KEY_SHORT;

const COLOR_ART: &str = "Art style, identical in every shot: Japanese TV anime from the early 2020s, digital cel shading with one hard shadow tone, \
crisp thin even line art, muted natural palette. No film grain, no retro 1980s or 1990s look, no painterly texture, no 3D.";
const BW_ART: &str = "Art style, identical in every shot: clean black ink line art and screentone shading, high contrast, no gray wash, no color at all.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimeStyle {
    /// Full-color anime stills.
    Color,
    /// Clean black-and-white stills.
    Bw,
}

impl AnimeStyle {
    fn name(self) -> &'static str {
        match self {
            AnimeStyle::Color => "color",
            AnimeStyle::Bw => "bw",
        }
    }

    fn art(self) -> &'static str {
        match self {
            AnimeStyle::Color => COLOR_ART,
            AnimeStyle::Bw => BW_ART,
        }
    }

    fn opening(self) -> &'static str {
        match self {
            AnimeStyle::Color => "Repaint this manga panel as a full-color anime still.",
            AnimeStyle::Bw => "Redraw this manga panel as a clean black-and-white anime still.",
        }
    }
}

pub struct Options {
    pub image: Backend,
    pub video: Backend,
    pub style: Option<AnimeStyle>,
    pub art: Option<String>,
    pub look: Option<String>,
    pub style_from: Option<String>,
    pub director: Option<Review>,
    pub shots: Vec<String>,
    pub key_shots_only: bool,
    pub keyframes_only: bool,
    pub sfx: bool,
    pub format: Format,
    pub max_duration: Option<f64>,
    pub split: bool,
    pub max_seconds: Option<f64>,
    pub dry_run: bool,
    pub max_area: Option<u32>,
    pub out: Option<PathBuf>,
    pub jobs: usize,
}

/// `anime.toml`: what stays the same from one shot, run or episode to the next.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Bible {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    style: Option<AnimeStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    look: Option<String>,
    /// Style name → the shot the reference image was drawn from (it does not reference itself).
    #[serde(default)]
    reference_shots: BTreeMap<String, String>,
}

impl Bible {
    fn style(&self) -> AnimeStyle {
        self.style.unwrap_or(AnimeStyle::Color)
    }

    fn art(&self) -> String {
        self.art.clone().unwrap_or_else(|| self.style().art().to_string())
    }
}

fn read_bible(project: &Project) -> Result<Bible> {
    let path = project.path("anime.toml");
    if !path.exists() {
        return Ok(Bible::default());
    }
    let text = fs::read_to_string(&path)?;
    toml::from_str(&text).with_context(|| format!("invalid style bible: {}", path.display()))
}

fn save_bible(project: &Project, bible: &Bible) -> Result<()> {
    fs::write(project.path("anime.toml"), toml::to_string_pretty(bible)?)?;
    Ok(())
}

/// What the flags change is kept for the next runs; what they leave out comes from the bible.
fn resolve_bible(project: &Project, source: Option<&Project>, opts: &Options) -> Result<Bible> {
    let mut bible = read_bible(source.unwrap_or(project))?;
    if source.is_some() {
        // Those shot ids belong to the other project: here, every shot follows the reference.
        bible.reference_shots.values_mut().for_each(String::clear);
    }
    if opts.style.is_some() {
        bible.style = opts.style;
    }
    if let Some(art) = clean(&opts.art) {
        bible.art = Some(art);
    }
    if let Some(look) = clean(&opts.look) {
        bible.look = Some(look);
    }
    Ok(bible)
}

fn clean(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn style_dir(project: &Project, style: AnimeStyle) -> PathBuf {
    project.path(format!("anime/{}", style.name()))
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

fn key_size(format: Format) -> (u32, u32) {
    match format {
        Format::Landscape => (KEY_LONG, KEY_SHORT),
        Format::Vertical => (KEY_SHORT, KEY_LONG),
    }
}

fn keyframe_prompt(bible: &Bible, format: Format, faces: &[&str], has_reference: bool) -> String {
    let frame = match format {
        Format::Landscape => "a 16:9 widescreen frame",
        Format::Vertical => "a 9:16 vertical frame, with the characters recomposed to fit it",
    };
    let mut parts = vec![
        bible.style().opening().to_string(),
        bible.art(),
        format!(
            "The first reference image is the manga panel: keep its exact composition, camera angle, poses, gestures, facial expressions and objects. \
             Remove every speech bubble, caption, sound effect and letter, and paint what is behind them. Fill {frame}. No text anywhere."
        ),
    ];
    if !faces.is_empty() {
        parts.push(format!(
            "The next reference images only give the identity of {}, in that order: use them for facial features and hair, never for the pose, the expression or the framing.",
            faces.join(", ")
        ));
    }
    if has_reference {
        parts.push("The last reference image is only an art style sample: match its line art, coloring, palette and shading, and copy nothing from its scene, poses or layout.".to_string());
    }
    if let Some(look) = &bible.look {
        parts.push(format!("Appearance: {look}."));
    }
    parts.join(" ")
}

fn face_prompt(bible: &Bible, name: &str) -> String {
    let mut parts = vec![
        format!("Draw {name} from this manga portrait as an anime character sheet: one front-facing head-and-shoulders portrait on a plain light background."),
        bible.art(),
        "Keep the exact face, hair and age of the reference. No text.".to_string(),
    ];
    if let Some(look) = &bible.look {
        parts.push(format!("Appearance: {look}."));
    }
    parts.join(" ")
}

/// The characters of a shot who have a portrait: those named on the shot, then its speakers.
fn shot_faces<'a>(shot: &Shot, bank: &'a [(String, PathBuf)]) -> Vec<&'a (String, PathBuf)> {
    let wanted = shot.characters.iter().map(String::as_str).chain(shot.lines.iter().filter_map(|l| l.speaker.as_deref()));
    let mut found: Vec<&(String, PathBuf)> = Vec::new();
    for name in wanted {
        if let Some(entry) = bank.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))
            && found.len() < MAX_FACES
            && !found.iter().any(|f| f.0 == entry.0)
        {
            found.push(entry);
        }
    }
    found
}

/// One remembered face per character, drawn once in the project's style.
fn plan_faces(project: &Project, shots: &[&Shot], bible: &Bible) -> Vec<DrawItem> {
    let bank = characters::bank(project);
    let dir = style_dir(project, bible.style()).join("faces");
    let mut seen: Vec<&str> = Vec::new();
    let mut todo = Vec::new();
    for (name, portrait) in shots.iter().flat_map(|s| shot_faces(s, &bank)) {
        let out = dir.join(format!("{name}.png"));
        if !seen.contains(&name.as_str()) && !out.exists() {
            todo.push(DrawItem {
                id: format!("face_{name}"),
                prompt: face_prompt(bible, name),
                refs: vec![portrait.to_string_lossy().into()],
                width: FACE_SIZE,
                height: FACE_SIZE,
                seed: KEY_SEED,
                steps: KEY_STEPS,
                out: out.to_string_lossy().into(),
            });
        }
        seen.push(name);
    }
    todo
}

struct KeyPlan {
    paths: HashMap<String, PathBuf>,
    todo: Vec<DrawItem>,
}

/// Saves each panel for the workers and finds which key images are missing. A face or reference
/// not drawn yet cannot be hashed: those shots simply count as still to draw.
fn plan_keyframes(project: &Project, shots: &[&Shot], opts: &Options, bible: &Bible, reference: Option<&(String, PathBuf)>) -> Result<KeyPlan> {
    let bank = characters::bank(project);
    let faces_dir = style_dir(project, bible.style()).join("faces");
    let work = project.path("work/anime");
    fs::create_dir_all(&work)?;
    fs::create_dir_all(project.path("keyframes"))?;
    let (width, height) = key_size(opts.format);
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
        let faces = shot_faces(shot, &bank);
        let names: Vec<&str> = faces.iter().map(|f| f.0.as_str()).collect();
        let style_image = reference.filter(|(id, _)| *id != shot.id).map(|(_, path)| path);
        let prompt = keyframe_prompt(bible, opts.format, &names, style_image.is_some());
        let mut refs: Vec<PathBuf> = vec![source.clone()];
        refs.extend(names.iter().map(|n| faces_dir.join(format!("{n}.png"))));
        refs.extend(style_image.cloned());
        let mut h = Sha256::new();
        h.update(panel.as_bytes());
        h.update(format!("{prompt}|{width}x{height}|{KEY_SEED}|{KEY_STEPS}{}", opts.image.cache_suffix()));
        for path in &refs[1..] {
            if let Ok(bytes) = fs::read(path) {
                h.update(bytes);
            }
        }
        let out = project.path(format!("keyframes/{}.png", &hex::encode(h.finalize())[..16]));
        if !out.exists() {
            plan.todo.push(DrawItem {
                id: shot.id.clone(),
                prompt,
                refs: refs.iter().map(|r| r.to_string_lossy().to_string()).collect(),
                width,
                height,
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

fn clip_settings(opts: &Options, bible: &Bible, keyframes: HashMap<String, PathBuf>) -> Settings {
    let default_area = if opts.video.is_local() { LOCAL_MAX_AREA } else { REMOTE_MAX_AREA };
    Settings {
        max_area: opts.max_area.unwrap_or(default_area),
        backend: opts.video.clone(),
        keyframes,
        anime: true,
        art: bible.art(),
        // The local model is silent.
        sound: opts.sfx && !opts.video.is_local(),
        ..Default::default()
    }
}

/// The ids of the shots that would pay for a clip: no key image yet, no direction yet, or no clip yet.
fn clips_to_generate(project: &Project, tl: &Timeline, ids: &[String], opts: &Options, bible: &Bible, plan: &KeyPlan) -> Result<Vec<String>> {
    let shots: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id) && s.mode == Mode::Animate).collect();
    let undecided = |s: &Shot| !plan.paths[&s.id].exists() || (opts.director.is_some() && needs_direction(s));
    let (open, known): (Vec<&Shot>, Vec<&Shot>) = shots.iter().copied().partition(|s| undecided(s));
    let mut pending: Vec<String> = open.iter().map(|s| s.id.clone()).collect();
    pending.extend(animate::pending(project, &known, &clip_settings(opts, bible, plan.paths.clone()))?);
    Ok(pending)
}

fn needs_direction(shot: &Shot) -> bool {
    shot.prompt.is_none() || shot.sound.is_none()
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
    characters: Vec<String>,
    items: Vec<DirectItem>,
    out: String,
}

#[derive(Debug, Default, Deserialize)]
struct Direction {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    sound: Option<String>,
    #[serde(default)]
    characters: Vec<String>,
}

/// Fills what is missing on the shots of `ids`. What was written by hand is never replaced.
fn direct(project: &Project, tl: &mut Timeline, ids: &[String], director: &Review) -> Result<()> {
    let work = project.path("work/anime");
    let items: Vec<DirectItem> = tl
        .shots
        .iter()
        .filter(|s| ids.contains(&s.id) && needs_direction(s))
        .map(|s| DirectItem { id: s.id.clone(), image: work.join(format!("{}.png", s.id)).to_string_lossy().into(), lines: s.lines.iter().map(|l| l.text.clone()).collect() })
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    println!("{} shots to direct with {} on {} (billed per use; panels are sent to this provider)", items.len(), director.model, director.provider);
    let out = work.join("direction.json");
    let known: Vec<String> = characters::bank(project).into_iter().map(|(name, _)| name).collect();
    let job = DirectJob { provider: &director.provider, model: &director.model, lang: &project.cfg.lang, characters: known.clone(), items, out: out.to_string_lossy().into() };
    worker::run("direct", &job, &project.path("work/direct_job.json"))?;
    apply_direction(tl, &read_direction(&out)?, &known);
    tl.save(project)
}

fn read_direction(path: &Path) -> Result<HashMap<String, Direction>> {
    let text = fs::read_to_string(path).with_context(|| format!("direction not written: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid direction: {}", path.display()))
}

fn apply_direction(tl: &mut Timeline, direction: &HashMap<String, Direction>, known: &[String]) {
    for shot in &mut tl.shots {
        let Some(d) = direction.get(&shot.id) else { continue };
        if shot.prompt.is_none() {
            shot.prompt = d.prompt.clone();
        }
        if shot.sound.is_none() {
            shot.sound = d.sound.clone();
        }
        if shot.characters.is_empty() {
            shot.characters = d.characters.iter().filter(|c| known.contains(c)).cloned().collect();
        }
    }
}

/// The shots with no sound effects, and why. `audible` is `None` for a shot without a clip.
fn silent_shots<'a>(shots: &[&'a Shot], audible: impl Fn(&str) -> Option<bool>) -> Vec<(&'a str, &'static str)> {
    shots
        .iter()
        .filter_map(|s| match audible(&s.id) {
            None => Some((s.id.as_str(), "still image, no clip")),
            Some(false) => Some((s.id.as_str(), "the model returned a silent clip")),
            Some(true) => None,
        })
        .collect()
}

fn print_silent(project: &str, silent: &[(&str, &str)]) {
    if silent.is_empty() {
        println!("every shot has sound effects");
        return;
    }
    println!("{} shots without sound effects:", silent.len());
    for (id, why) in silent {
        println!("  {id}: {why}. To fix: manga animate {project} {id} --sound \"what is heard\", then run manga anime again");
    }
}

pub fn run(project: &Project, opts: &Options) -> Result<()> {
    let mut tl = Timeline::load(project)?;
    let source = opts.style_from.as_deref().map(Project::open).transpose()?;
    let mut bible = resolve_bible(project, source.as_ref(), opts)?;
    let parts = render::parts(&targets(&tl, &opts.shots)?, opts.max_duration, opts.split);
    let ids: Vec<String> = parts.concat();

    let style = bible.style();
    let reference_path = style_dir(project, style).join("reference.png");
    let inherited = source.as_ref().is_some_and(|s| style_dir(s, style).join("reference.png").exists());
    let known_reference = (reference_path.exists() || inherited).then(|| (bible.reference_shots.get(style.name()).cloned().unwrap_or_default(), reference_path.clone()));

    let selected: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id)).collect();
    let faces = plan_faces(project, &selected, &bible);
    let plan = plan_keyframes(project, &selected, opts, &bible, known_reference.as_ref())?;
    let planned = marked(&tl, &ids, opts.key_shots_only);
    let clips = if opts.keyframes_only { Vec::new() } else { clips_to_generate(project, &planned, &ids, opts, &bible, &plan)? };
    // Not sum(): an empty float sum is -0.0, printed as "-0".
    let seconds = planned.shots.iter().filter(|s| clips.contains(&s.id)).fold(0.0, |total, s| total + billed_seconds(s.duration));
    println!("{} shots: {} key images to draw, {} clips to generate (about {seconds:.0} s of video)", ids.len(), plan.todo.len(), clips.len());
    if !faces.is_empty() {
        println!("{} character faces to draw and remember", faces.len());
    }
    if opts.dry_run {
        return Ok(());
    }
    if let Some(max) = opts.max_seconds.filter(|max| seconds > *max) {
        bail!("{seconds:.0} s of video to generate is over --max-seconds {max:.0}: use --shots, --key-shots-only or a higher limit");
    }
    // Every key is checked first: a missing video key must not surface after paying for the images.
    if !plan.todo.is_empty() || !faces.is_empty() {
        opts.image.check_key()?;
    }
    if !clips.is_empty() {
        opts.video.check_key()?;
    }
    if let Some(src) = &source
        && src.path("anime").exists()
    {
        copy_tree(&src.path("anime"), &project.path("anime"))?;
    }
    save_bible(project, &bible)?;
    if let Some(director) = opts.director.as_ref().filter(|_| !opts.keyframes_only) {
        director.check_key()?;
        // A still shot has no use for a motion prompt: do not pay for one.
        let animated: Vec<String> = planned.shots.iter().filter(|s| ids.contains(&s.id) && s.mode == Mode::Animate).map(|s| s.id.clone()).collect();
        direct(project, &mut tl, &animated, director)?;
    }

    let selected: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id)).collect();
    let faces = plan_faces(project, &selected, &bible);
    if !faces.is_empty() {
        fs::create_dir_all(style_dir(project, style).join("faces"))?;
        story::draw_items(&project.root, &opts.image, faces)?;
    }
    let reference = match known_reference {
        Some(reference) => reference,
        None => {
            // The first image sets the look: every other one is drawn to match it.
            let first = selected[0];
            let plan = plan_keyframes(project, &[first], opts, &bible, None)?;
            if !plan.todo.is_empty() {
                story::draw_items(&project.root, &opts.image, plan.todo)?;
            }
            fs::copy(&plan.paths[&first.id], &reference_path).with_context(|| format!("reference image not generated for shot {}", first.id))?;
            bible.reference_shots.insert(style.name().to_string(), first.id.clone());
            save_bible(project, &bible)?;
            (first.id.clone(), reference_path.clone())
        }
    };
    let plan = plan_keyframes(project, &selected, opts, &bible, Some(&reference))?;
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

    let mut tl = marked(&tl, &ids, opts.key_shots_only);
    if opts.format == Format::Vertical {
        (tl.width, tl.height) = opts.format.frame();
    }
    let chosen: Vec<&Shot> = tl.shots.iter().filter(|s| ids.contains(&s.id)).collect();
    let clips = animate::ensure_clips(project, &chosen, &clip_settings(opts, &bible, keyframes.clone()))?;
    if clip_settings(opts, &bible, HashMap::new()).sound {
        let silent = silent_shots(&chosen, |id| clips.get(id).map(|c| render::is_audible(c)));
        print_silent(&project.cfg.name, &silent);
    }
    let suffix = if opts.format == Format::Vertical { "-vertical" } else { "" };
    let out = opts.out.clone().unwrap_or_else(|| project.path(format!("out/{}-anime{suffix}.mp4", project.cfg.name)));
    let render_opts = RenderOptions { out, style: Style::Anime, shots: Vec::new(), show_speaker: false, captions: true, jobs: opts.jobs, keyframes };
    for path in render::run_parts(project, &tl, &clips, render_opts, parts)? {
        println!("anime: {}", path.display());
    }
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
            sound: None,
            characters: Vec::new(),
            lines: speakers.iter().map(|s| Line { text_id: "t".into(), text: "hi".into(), speaker: Some(s.to_string()), start: 0.0, duration: 1.0, audio: None }).collect(),
        }
    }

    fn timeline(shots: Vec<Shot>) -> Timeline {
        Timeline { width: 1920, height: 1080, fps: 30, shots }
    }

    fn bible(style: AnimeStyle) -> Bible {
        Bible { style: Some(style), ..Default::default() }
    }

    fn bank(names: &[&str]) -> Vec<(String, PathBuf)> {
        names.iter().map(|n| (n.to_string(), PathBuf::from(format!("{n}.png")))).collect()
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
    fn every_prompt_pins_the_era_and_the_cleanup() {
        let color = keyframe_prompt(&bible(AnimeStyle::Color), Format::Landscape, &[], false);
        let bw = keyframe_prompt(&bible(AnimeStyle::Bw), Format::Landscape, &[], false);
        assert!(color.contains("full-color") && color.contains("early 2020s") && color.contains("no retro 1980s"));
        assert!(bw.contains("no color"));
        assert!(color.contains("speech bubble") && bw.contains("speech bubble"));
    }

    #[test]
    fn a_custom_art_style_replaces_the_default() {
        let b = Bible { art: Some("1990s cel animation".into()), ..Default::default() };
        let prompt = keyframe_prompt(&b, Format::Landscape, &[], false);
        assert!(prompt.contains("1990s cel animation") && !prompt.contains("early 2020s"));
    }

    #[test]
    fn faces_look_and_reference_reach_the_prompt() {
        let b = Bible { look: Some("Kit: red hair".into()), ..Default::default() };
        let prompt = keyframe_prompt(&b, Format::Landscape, &["Kit", "Ann"], true);
        assert!(prompt.contains("only give the identity of Kit, Ann, in that order"));
        assert!(prompt.contains("never for the pose, the expression or the framing"));
        assert!(prompt.contains("only an art style sample"));
        assert!(prompt.contains("copy nothing from its scene"));
        assert!(prompt.contains("Appearance: Kit: red hair."));
    }

    #[test]
    fn vertical_asks_for_a_recomposed_frame() {
        assert_eq!(key_size(Format::Vertical), (720, 1280));
        assert!(keyframe_prompt(&bible(AnimeStyle::Color), Format::Vertical, &[], false).contains("9:16 vertical"));
    }

    #[test]
    fn faces_are_found_once_named_first_and_capped() {
        let bank = bank(&["Ann", "Bob", "Cid", "Dan"]);
        let mut s = shot("k", &["ann", "ann", "Unknown", "Cid", "Dan"]);
        s.characters = vec!["Bob".into()];
        let names: Vec<&str> = shot_faces(&s, &bank).iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["Bob", "Ann", "Cid"]);
    }

    #[test]
    fn the_bible_round_trips_through_toml() {
        let mut b = Bible { style: Some(AnimeStyle::Bw), look: Some("Kit: red hair".into()), ..Default::default() };
        b.reference_shots.insert("bw".into(), "p0001_k0".into());
        let back: Bible = toml::from_str(&toml::to_string_pretty(&b).unwrap()).unwrap();
        assert_eq!(back.style(), AnimeStyle::Bw);
        assert_eq!(back.look.as_deref(), Some("Kit: red hair"));
        assert_eq!(back.reference_shots["bw"], "p0001_k0");
    }

    #[test]
    fn direction_never_replaces_what_was_written_by_hand() {
        let mut manual = shot("a", &[]);
        manual.prompt = Some("mine".into());
        manual.sound = Some("my sound".into());
        let mut tl = timeline(vec![manual, shot("b", &[]), shot("c", &[])]);
        let written = |p: &str| Direction { prompt: Some(p.into()), sound: Some(format!("{p} sound")), characters: vec!["Kit".into(), "Ghost".into()] };
        let direction = HashMap::from([("a".to_string(), written("theirs")), ("b".to_string(), written("new"))]);
        apply_direction(&mut tl, &direction, &["Kit".to_string()]);
        assert_eq!(tl.shots[0].prompt.as_deref(), Some("mine"));
        assert_eq!(tl.shots[0].sound.as_deref(), Some("my sound"));
        assert_eq!(tl.shots[1].prompt.as_deref(), Some("new"));
        assert_eq!(tl.shots[1].sound.as_deref(), Some("new sound"));
        assert_eq!(tl.shots[1].characters, ["Kit"], "an unknown character is ignored");
        assert_eq!(tl.shots[2].prompt, None);
    }

    #[test]
    fn silent_shots_say_why() {
        let (a, b, c) = (shot("a", &[]), shot("b", &[]), shot("c", &[]));
        let silent = silent_shots(&[&a, &b, &c], |id| match id {
            "a" => None,
            "b" => Some(false),
            _ => Some(true),
        });
        assert_eq!(silent, [("a", "still image, no clip"), ("b", "the model returned a silent clip")]);
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
