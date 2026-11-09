//! `manga`: read and animate manga locally.
//!
//! Pipeline: import/source → project new → analyze → (read) → voice → timeline → render.
//! Spin-off: characters → story new → story build → voice → timeline → render.

mod analysis;
mod animate;
mod backend;
mod captions;
mod characters;
mod doctor;
mod home;
mod library;
mod project;
mod reader;
mod render;
mod sources;
mod story;
mod timeline;
mod uninstall;
mod voice;
mod worker;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::backend::{Backend, BackendKind};
use crate::project::{Direction, PagePos, Project};
use crate::timeline::{Mode, Timeline};

#[derive(Parser)]
#[command(name = "manga", version, about = "Read and animate manga locally: bubble OCR, voices, video editing, AI animation")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check dependencies and models.
    Doctor,
    /// Search and download from an online source (MangaDex).
    Source {
        #[command(subcommand)]
        cmd: SourceCmd,
    },
    /// Import local files: CBZ, EPUB, PDF, CBR, or a folder of images or chapters.
    Import {
        path: PathBuf,
        /// Title in the library (default: file or folder name).
        #[arg(long)]
        title: Option<String>,
        /// Chapter number (default: guessed from the name).
        #[arg(long)]
        chapter: Option<f64>,
    },
    /// List the local library.
    Library,
    /// Create, list or show projects.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Detect panels and bubbles, read the text (OCR), attribute lines to speakers.
    Analyze {
        project: String,
        /// magi (local, characters), vision (local, fast) or api (vision + proofreading by the Claude API).
        #[arg(long, default_value = "magi")]
        detector: String,
        /// Re-analyze even pages already analyzed (overwrites corrections).
        #[arg(long)]
        force: bool,
    },
    /// Open the web reader to check and correct the analysis.
    Read {
        project: String,
        #[arg(long, default_value_t = 8642)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// Generate the voice of each speech bubble according to cast.toml.
    Voice {
        project: String,
        #[arg(long)]
        force: bool,
        /// local (Kokoro) or openrouter (billed, sends the text to third parties; voices in cast.toml are the model's own).
        #[arg(long, value_enum, default_value = "local")]
        backend: BackendKind,
        /// OpenRouter model id, required with --backend openrouter.
        #[arg(long)]
        model: Option<String>,
    },
    /// Build timeline.json (the edit), keeping manual adjustments.
    Timeline {
        project: String,
        /// Seconds of full-page shot on each page (0 for none).
        #[arg(long, default_value_t = 1.5)]
        page_intro: f64,
        /// Discard manual adjustments from the existing timeline.
        #[arg(long)]
        reset: bool,
    },
    /// Mark shots for AI animation (or switch them back to motion mode).
    Animate {
        project: String,
        /// Shot ids (p0003_k1) or numbers (1 = first).
        shots: Vec<String>,
        /// What should move, preferably in English ("wind blowing through his hair").
        #[arg(long)]
        prompt: Option<String>,
        /// Switch these shots back to motion mode.
        #[arg(long)]
        off: bool,
    },
    /// Render the video.
    Render {
        project: String,
        /// Output file (default: projects/<name>/out/<name>.mp4).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Render only these shots (ids or numbers), for previewing.
        #[arg(long, value_delimiter = ',')]
        shots: Vec<String>,
        /// scroll: the page scrolls from panel to panel; panels: one panel at a time on a blurred background.
        #[arg(long, value_enum, default_value = "scroll")]
        style: render::Style,
        /// Treat "animate" shots as motion shots (fast).
        #[arg(long)]
        no_animate: bool,
        /// Show the speaker's name above the captions.
        #[arg(long)]
        speaker: bool,
        /// No captions (the bubble text stays visible in the image).
        #[arg(long)]
        no_captions: bool,
        /// Captions in scroll style too, where the full-width page makes them redundant.
        #[arg(long, conflicts_with = "no_captions")]
        captions: bool,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Frames per animated clip (4n+1, 24 fps).
        #[arg(long, default_value_t = 49)]
        frames: u32,
        #[arg(long)]
        steps: Option<u32>,
        /// Maximum area of an animated clip, in pixels.
        #[arg(long, default_value_t = 640 * 352)]
        max_area: u32,
        /// Where animated clips are generated: local (Wan2.2) or openrouter (billed, sends panels to third parties).
        #[arg(long, value_enum, default_value = "local")]
        backend: BackendKind,
        /// OpenRouter model id, required with --backend openrouter.
        #[arg(long)]
        model: Option<String>,
    },
    /// Run analyze → voice → timeline → render in sequence.
    Run {
        project: String,
        #[arg(long, default_value = "magi")]
        detector: String,
        #[arg(long)]
        no_animate: bool,
    },
    /// Crop the characters detected by Magi into characters/found/, to name and reuse them.
    Characters {
        project: String,
        /// Crops kept per character (the largest appearances).
        #[arg(long, default_value_t = 8)]
        per_character: usize,
    },
    /// Spin-off: a new manga drawn with a project's characters (story.json).
    Story {
        #[command(subcommand)]
        cmd: StoryCmd,
    },
    /// Install the heavy models.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Remove everything: the app folder (data, models and caches included) and its links.
    Uninstall {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SourceCmd {
    /// Search for a series.
    Search {
        query: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long)]
        lang: Option<String>,
    },
    /// List a series' chapters in a language.
    Chapters {
        id: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Download chapters into the library.
    Fetch {
        id: String,
        /// 101-110, 1,3,5-7 or all.
        #[arg(long)]
        chapters: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long, default_value = "en")]
        lang: String,
        /// Title in the library (default: the series title).
        #[arg(long)]
        title: Option<String>,
        /// Prefer this group when several versions of a chapter exist.
        #[arg(long)]
        group: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Create a project from a page range: --from ch101:p1 --to ch120:p20.
    New {
        name: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        from: PagePos,
        #[arg(long)]
        to: PagePos,
        /// Language of the text on the pages (fr, en, ja, es, zh, ko…).
        #[arg(long)]
        lang: String,
        /// rtl for manga, ltr for comics/webtoons.
        #[arg(long, value_enum, default_value = "rtl")]
        direction: Direction,
    },
    List,
    Show {
        name: String,
    },
}

#[derive(Subcommand)]
enum StoryCmd {
    /// Create the project with a sample story.json and the characters from --from copied into refs/.
    New {
        name: String,
        /// Source project, whose characters/ bank (or characters/found/) provides the characters.
        #[arg(long)]
        from: String,
        /// Dialogue language (default: the source project's).
        #[arg(long)]
        lang: Option<String>,
        #[arg(long, value_enum)]
        direction: Option<Direction>,
    },
    /// Draw the panels that changed, then compose the pages and their analysis.
    Build {
        name: String,
        /// Draw only these pages (1,3); the others stay as they are.
        #[arg(long, value_delimiter = ',')]
        pages: Vec<usize>,
        /// Compose without drawing: panels not drawn yet stay gray.
        #[arg(long)]
        no_draw: bool,
        #[arg(long, default_value_t = 4)]
        steps: u32,
        /// Where panels are drawn: local (FLUX.2 klein) or openrouter (billed, sends prompts and character references to third parties).
        #[arg(long, value_enum, default_value = "local")]
        backend: BackendKind,
        /// OpenRouter model id, required with --backend openrouter.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Subcommand)]
enum ModelsCmd {
    /// Download and convert a model: wan (animation, ~34 GB) or flux2 (drawing, ~15 GB).
    Install {
        model: String,
        /// Transformer quantization: 4 (suits 16 GB) or 8 (quality, 24 GB and up).
        #[arg(long, default_value_t = 4, value_parser = quantization_bits)]
        bits: u32,
    },
}

/// Refused here: otherwise a bad value only fails after a download of tens of GB.
fn quantization_bits(s: &str) -> Result<u32, String> {
    match s {
        "4" => Ok(4),
        "8" => Ok(8),
        _ => Err("expected 4 or 8".into()),
    }
}

#[derive(Serialize)]
struct AnalyzeJob {
    lang: String,
    direction: Direction,
    detector: String,
    character_bank: Vec<BankEntry>,
    pages: Vec<AnalyzePage>,
}

#[derive(Serialize)]
struct BankEntry {
    name: String,
    image: String,
}

#[derive(Serialize)]
struct AnalyzePage {
    image: String,
    out: String,
    prefix: String,
}

fn analyze(project: &Project, detector: &str, force: bool) -> Result<()> {
    if !matches!(detector, "magi" | "vision" | "api") {
        bail!("unknown detector: {detector} (magi, vision or api)");
    }
    let pages: Vec<AnalyzePage> = project
        .cfg
        .pages
        .iter()
        .filter(|p| force || !project.analysis_path(p).exists())
        .map(|p| AnalyzePage {
            image: project.page_path(p).to_string_lossy().into(),
            out: project.analysis_path(p).to_string_lossy().into(),
            prefix: project::stem(&p.file).to_string(),
        })
        .collect();
    if pages.is_empty() {
        println!("all pages are already analyzed (--force to start over)");
        return Ok(());
    }
    // A worker stopping on page 40 of 200 is a Python traceback and a long wait for nothing.
    for page in &pages {
        image::image_dimensions(&page.image).with_context(|| format!("unreadable page: {} (replace it, or leave it out of the project's range)", page.image))?;
    }
    // Character bank: characters/<Name>.png, used to name the speakers.
    let character_bank: Vec<BankEntry> =
        characters::bank(project).into_iter().map(|(name, p)| BankEntry { name, image: p.to_string_lossy().into() }).collect();
    println!("analyzing {} pages ({detector}, {} known characters)", pages.len(), character_bank.len());
    let job = AnalyzeJob {
        lang: project.cfg.lang.clone(),
        direction: project.cfg.direction,
        detector: detector.into(),
        character_bank,
        pages,
    };
    worker::run("analyze", &job, &project.path("work/analyze_job.json"))?;
    // Reload everything: this validates the JSON produced by the worker.
    let mut lines = 0;
    for p in &project.cfg.pages {
        lines += analysis::PageAnalysis::load(&project.analysis_path(p))?.texts.iter().filter(|t| t.essential).count();
    }
    println!("{lines} lines detected. Check them with: manga read {}", project.cfg.name);
    Ok(())
}

fn render(project: &Project, out: Option<PathBuf>, mut opts: render::RenderOptions, no_animate: bool, settings: animate::Settings) -> Result<()> {
    let mut tl = Timeline::load(project)?;
    if no_animate {
        tl.shots.iter_mut().for_each(|s| s.mode = Mode::Motion);
    }
    let selected = render::select(&tl, &opts.shots)?;
    let clips = animate::ensure_clips(project, &selected, &settings)?;
    opts.out = out.unwrap_or_else(|| project.path(format!("out/{}.mp4", project.cfg.name)));
    let path = render::run(project, &tl, &clips, &opts)?;
    println!("video: {}", path.display());
    Ok(())
}

fn main() -> Result<()> {
    match dispatch() {
        // The worker already printed a one-line error.
        Err(e) if e.is::<worker::Reported>() => std::process::exit(1),
        other => other,
    }
}

fn dispatch() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Doctor => doctor::run(),
        Cmd::Source { cmd } => match cmd {
            SourceCmd::Search { query, source, lang } => {
                for hit in sources::by_name(&source)?.search(&query, lang.as_deref())? {
                    println!(
                        "{}  {}{}  [{}]  languages: {}",
                        hit.id,
                        hit.title,
                        hit.year.map(|y| format!(" ({y})")).unwrap_or_default(),
                        hit.status,
                        hit.languages.join(" ")
                    );
                }
            }
            SourceCmd::Chapters { id, source, lang } => {
                let chapters = sources::by_name(&source)?.chapters(&id, &lang)?;
                if chapters.is_empty() {
                    println!("no downloadable chapter in \"{lang}\": licensed title removed from MangaDex, or no translation. Use your own files with `manga import`.");
                }
                for c in &chapters {
                    let num = c.chapter.map(library::chapter_dir_name).unwrap_or_else(|| "?".into());
                    let state = match &c.external_url {
                        Some(url) => format!("external (publisher): {url}"),
                        None => format!("{} pages", c.pages),
                    };
                    println!("ch {num:>6}  vol {:>3}  {:<40}  {:<25}  {state}", c.volume.as_deref().unwrap_or("-"), c.title, c.group);
                }
            }
            SourceCmd::Fetch { id, chapters, source, lang, title, group } => {
                let src = sources::by_name(&source)?;
                let spec = sources::parse_chapter_spec(&chapters)?;
                let title = match title {
                    Some(t) => t,
                    None => src.series_title(&id)?,
                };
                let all = src.chapters(&id, &lang)?;
                // One version per number: downloadable first, then the requested group.
                let mut best: std::collections::BTreeMap<String, sources::ChapterInfo> = Default::default();
                for c in all.into_iter().filter(|c| c.chapter.is_some_and(|n| sources::in_spec(&spec, n))) {
                    let key = library::chapter_dir_name(c.chapter.unwrap());
                    let score = |c: &sources::ChapterInfo| {
                        (c.external_url.is_none() && c.pages > 0, group.as_ref().is_some_and(|g| c.group.eq_ignore_ascii_case(g)))
                    };
                    if best.get(&key).is_none_or(|b| score(&c) > score(b)) {
                        best.insert(key, c);
                    }
                }
                if best.is_empty() {
                    bail!("no chapter \"{chapters}\" in \"{lang}\" for this series");
                }
                let mut fetched = 0;
                for c in best.values() {
                    let n = c.chapter.unwrap();
                    let dest = library::title_dir(&title).join(library::chapter_dir_name(n));
                    if dest.exists() {
                        std::fs::remove_dir_all(&dest)?;
                    }
                    match src.fetch(c, &dest) {
                        Ok(pages) => {
                            fetched += 1;
                            println!("ch {}: {pages} pages ({})", library::chapter_dir_name(n), c.group);
                        }
                        Err(e) => eprintln!("ch {} skipped: {e:#}", library::chapter_dir_name(n)),
                    }
                }
                println!("{fetched}/{} chapters in {}", best.len(), library::title_dir(&title).display());
            }
        },
        Cmd::Import { path, title, chapter } => {
            let title = title.unwrap_or_else(|| library::default_title(&path));
            for (n, pages) in library::import(&path, &title, chapter)? {
                println!("{} ch {}: {pages} pages", home::slugify(&title), library::chapter_dir_name(n));
            }
        }
        Cmd::Library => {
            let titles = library::list_titles()?;
            if titles.is_empty() {
                println!("library is empty: manga import <file> or manga source fetch …");
            }
            for (title, n) in titles {
                let chapters = library::chapters(&title)?;
                let range = match (chapters.first(), chapters.last()) {
                    (Some(a), Some(b)) => format!("ch {} → {}", library::chapter_dir_name(a.0), library::chapter_dir_name(b.0)),
                    _ => String::new(),
                };
                println!("{title:<40} {n:>4} chapters  {range}");
            }
        }
        Cmd::Project { cmd } => match cmd {
            ProjectCmd::New { name, title, from, to, lang, direction } => {
                let p = Project::create(&name, &title, from, to, &lang, direction)?;
                println!("project {}: {} pages ({} → {})\n{}", p.cfg.name, p.cfg.pages.len(), p.cfg.from, p.cfg.to, p.root.display());
            }
            ProjectCmd::List => {
                if let Ok(entries) = std::fs::read_dir(home::projects_dir()) {
                    for e in entries.flatten() {
                        if let Ok(p) = Project::open(&e.file_name().to_string_lossy()) {
                            println!("{:<24} {:<30} {} → {}  {} pages  [{}]", p.cfg.name, p.cfg.title, p.cfg.from, p.cfg.to, p.cfg.pages.len(), p.cfg.lang);
                        }
                    }
                }
            }
            ProjectCmd::Show { name } => {
                let p = Project::open(&name)?;
                let analyzed = p.cfg.pages.iter().filter(|pg| p.analysis_path(pg).exists()).count();
                let audio = voice::AudioIndex::load(&p).clips.len();
                println!("{} — {} {} → {} [{} {:?}]", p.cfg.name, p.cfg.title, p.cfg.from, p.cfg.to, p.cfg.lang, p.cfg.direction);
                println!("folder:   {}", p.root.display());
                println!("pages:    {} ({analyzed} analyzed)", p.cfg.pages.len());
                println!("voice:    {audio} lines");
                match Timeline::load(&p) {
                    Ok(tl) => {
                        let animated = tl.shots.iter().filter(|s| s.mode == Mode::Animate && !s.skip).count();
                        println!("timeline: {} shots, {:.0}s, {animated} animated", tl.shots.len(), tl.total());
                    }
                    Err(_) => println!("timeline: missing"),
                }
            }
        },
        Cmd::Analyze { project, detector, force } => analyze(&Project::open(&project)?, &detector, force)?,
        Cmd::Read { project, port, no_open } => reader::serve(Project::open(&project)?, port, !no_open)?,
        Cmd::Voice { project, force, backend, model } => {
            let backend = Backend::parse(backend, model)?;
            voice::run(&Project::open(&project)?, force, &backend)?;
        }
        Cmd::Timeline { project, page_intro, reset } => {
            let p = Project::open(&project)?;
            let opts = timeline::Options { page_intro, ..Default::default() };
            let tl = timeline::run(&p, &opts, reset)?;
            println!("{} shots, {:.0}s → {}", tl.shots.len(), tl.total(), p.path("timeline.json").display());
        }
        Cmd::Animate { project, shots, prompt, off } => {
            // select() reads an empty list as "every shot".
            if shots.is_empty() {
                bail!("specify at least one shot (id or number)");
            }
            let p = Project::open(&project)?;
            let mut tl = Timeline::load(&p)?;
            let ids: Vec<String> = render::select(&tl, &shots)?.iter().map(|s| s.id.clone()).collect();
            for shot in tl.shots.iter_mut().filter(|s| ids.contains(&s.id)) {
                shot.mode = if off { Mode::Motion } else { Mode::Animate };
                if prompt.is_some() {
                    shot.prompt = prompt.clone();
                }
                println!("{} → {:?}", shot.id, shot.mode);
            }
            tl.save(&p)?;
        }
        Cmd::Render { project, out, shots, style, no_animate, speaker, no_captions, captions, jobs, frames, steps, max_area, backend, model } => {
            let backend = Backend::parse(backend, model)?;
            let settings = animate::Settings { frames, steps, max_area, backend, ..Default::default() };
            let captions = captions || (!no_captions && style == render::Style::Panels);
            let opts = render::RenderOptions { out: PathBuf::new(), style, shots, show_speaker: speaker, captions, jobs };
            render(&Project::open(&project)?, out, opts, no_animate, settings)?;
        }
        Cmd::Run { project, detector, no_animate } => {
            let p = Project::open(&project)?;
            analyze(&p, &detector, false)?;
            voice::run(&p, false, &Backend::Local)?;
            timeline::run(&p, &timeline::Options::default(), false)?;
            let opts = render::RenderOptions { out: PathBuf::new(), style: render::Style::Scroll, shots: Vec::new(), show_speaker: false, captions: false, jobs: 4 };
            render(&p, None, opts, no_animate, animate::Settings::default())?;
        }
        Cmd::Characters { project, per_character } => {
            let p = Project::open(&project)?;
            let found = characters::run(&p, per_character)?;
            for f in &found {
                println!("{:<20} {:>4} appearances  {}", f.label, f.appearances, f.crops.first().map(String::as_str).unwrap_or(""));
            }
            println!(
                "{} characters → {}\nTo name one, copy its best crop to characters/<Name>.png",
                found.len(),
                p.path("characters/found/index.json").display()
            );
        }
        Cmd::Story { cmd } => match cmd {
            StoryCmd::New { name, from, lang, direction } => {
                let path = story::new(&name, &Project::open(&from)?, lang, direction)?;
                println!("script: {}\nWrite the story, then run: manga story build {}", path.display(), home::slugify(&name));
            }
            StoryCmd::Build { name, pages, no_draw, steps, backend, model } => {
                let backend = Backend::parse(backend, model)?;
                let (p, pending) = story::build(&name, &story::BuildOptions { pages, draw: !no_draw, steps, backend })?;
                println!("{} pages → {}", p.cfg.pages.len(), p.path("pages").display());
                if pending > 0 {
                    println!("{pending} panels not drawn yet (shown in gray)");
                }
                println!("Next: manga voice {0} && manga timeline {0} && manga render {0}", p.cfg.name);
            }
        },
        Cmd::Uninstall { yes } => uninstall::run(yes)?,
        Cmd::Models { cmd } => match cmd {
            ModelsCmd::Install { model, bits } => {
                let job_path = home::data_dir().join("models/install_job.json");
                match model.as_str() {
                    "wan" => {
                        let job = serde_json::json!({
                            "model": "wan",
                            "src_dir": home::data_dir().join("models/wan2.2-ti2v-5b-src"),
                            "out_dir": animate::model_dir(),
                            "bits": bits,
                        });
                        worker::run("models", &job, &job_path)?;
                    }
                    "flux2" => {
                        let job = serde_json::json!({ "model": "flux2", "out_dir": story::model_dir(), "bits": bits });
                        worker::run_in(worker::Env::Draw, "models", &job, &job_path)?;
                    }
                    _ => bail!("unknown model: {model} (available: wan, flux2)"),
                }
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice_args(args: &[&str]) -> (BackendKind, Option<String>) {
        match Cli::try_parse_from([&["manga", "voice", "p"], args].concat()).unwrap().cmd {
            Cmd::Voice { backend, model, .. } => (backend, model),
            _ => unreachable!(),
        }
    }

    #[test]
    fn model_bits_are_checked_before_any_download() {
        let parse = |bits| Cli::try_parse_from(["manga", "models", "install", "wan", "--bits", bits]).is_ok();
        assert!(parse("4") && parse("8"));
        assert!(!parse("3") && !parse("16"));
    }

    #[test]
    fn backend_defaults_to_local() {
        assert_eq!(voice_args(&[]), (BackendKind::Local, None));
        for args in [vec!["manga", "render", "p"], vec!["manga", "story", "build", "p"]] {
            let cmd = Cli::try_parse_from(args).unwrap().cmd;
            let backend = match cmd {
                Cmd::Render { backend, .. } => backend,
                Cmd::Story { cmd: StoryCmd::Build { backend, .. } } => backend,
                _ => unreachable!(),
            };
            assert_eq!(backend, BackendKind::Local);
        }
    }

    #[test]
    fn backend_flags_parse_on_every_step() {
        assert_eq!(voice_args(&["--backend", "openrouter", "--model", "a/b"]), (BackendKind::OpenRouter, Some("a/b".into())));
        for sub in [vec!["manga", "render", "p"], vec!["manga", "story", "build", "p"]] {
            Cli::try_parse_from([sub, vec!["--backend", "openrouter", "--model", "a/b"]].concat()).unwrap();
        }
        assert!(Cli::try_parse_from(["manga", "voice", "p", "--backend", "nope"]).is_err());
    }
}
