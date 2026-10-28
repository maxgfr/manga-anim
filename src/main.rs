//! `manga` : lire et animer des mangas en local.
//!
//! Chaîne : import/source → project new → analyze → (read) → voice → timeline → render.

mod analysis;
mod animate;
mod captions;
mod doctor;
mod home;
mod library;
mod project;
mod reader;
mod render;
mod sources;
mod timeline;
mod uninstall;
mod voice;
mod worker;

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::project::{Direction, PagePos, Project};
use crate::timeline::{Mode, Timeline};

#[derive(Parser)]
#[command(name = "manga", version, about = "Lire et animer des mangas en local : OCR des bulles, voix, montage vidéo, animation IA")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Vérifie les dépendances et les modèles.
    Doctor,
    /// Cherche et télécharge depuis une source en ligne (MangaDex).
    Source {
        #[command(subcommand)]
        cmd: SourceCmd,
    },
    /// Importe des fichiers locaux : CBZ, EPUB, PDF, CBR, dossier d'images ou de chapitres.
    Import {
        path: PathBuf,
        /// Titre dans la bibliothèque (défaut : nom du fichier ou dossier).
        #[arg(long)]
        title: Option<String>,
        /// Numéro de chapitre (défaut : deviné depuis le nom).
        #[arg(long)]
        chapter: Option<f64>,
    },
    /// Liste la bibliothèque locale.
    Library,
    /// Crée, liste ou affiche les projets.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// Détecte cases et bulles, lit le texte (OCR), attribue les répliques.
    Analyze {
        project: String,
        /// magi (précis, persos) ou vision (rapide, sans modèle à télécharger).
        #[arg(long, default_value = "magi")]
        detector: String,
        /// Réanalyse même les pages déjà analysées (écrase les corrections).
        #[arg(long)]
        force: bool,
    },
    /// Ouvre la liseuse web pour vérifier et corriger l'analyse.
    Read {
        project: String,
        #[arg(long, default_value_t = 8642)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// Génère la voix de chaque bulle selon cast.toml.
    Voice {
        project: String,
        #[arg(long)]
        force: bool,
    },
    /// Construit timeline.json (le montage), en gardant les réglages manuels.
    Timeline {
        project: String,
        /// Secondes de plan d'ensemble sur chaque page (0 pour aucun).
        #[arg(long, default_value_t = 1.5)]
        page_intro: f64,
        /// Ignore les réglages manuels de la timeline existante.
        #[arg(long)]
        reset: bool,
    },
    /// Marque des plans à animer par l'IA (ou les repasse en mode motion).
    Animate {
        project: String,
        /// Identifiants (p0003_k1) ou numéros de plans (1 = premier).
        shots: Vec<String>,
        /// Ce qui doit bouger, en anglais de préférence (« wind blowing through his hair »).
        #[arg(long)]
        prompt: Option<String>,
        /// Repasse ces plans en mode motion.
        #[arg(long)]
        off: bool,
    },
    /// Rend la vidéo.
    Render {
        project: String,
        /// Fichier de sortie (défaut : projects/<nom>/out/<nom>.mp4).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Ne rend que ces plans (identifiants ou numéros), pour prévisualiser.
        #[arg(long, value_delimiter = ',')]
        shots: Vec<String>,
        /// scroll : la page défile de case en case ; panels : une case à la fois sur fond flou.
        #[arg(long, value_enum, default_value = "scroll")]
        style: render::Style,
        /// Traite les plans « animate » comme des plans motion (rapide).
        #[arg(long)]
        no_animate: bool,
        /// Affiche le nom du locuteur au-dessus des sous-titres.
        #[arg(long)]
        speaker: bool,
        /// Pas de sous-titres (le texte des bulles reste visible dans l'image).
        #[arg(long)]
        no_captions: bool,
        /// Sous-titres aussi en style scroll, où la page en pleine largeur les rend superflus.
        #[arg(long, conflicts_with = "no_captions")]
        captions: bool,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Images par clip animé (4n+1, 24 i/s).
        #[arg(long, default_value_t = 49)]
        frames: u32,
        #[arg(long)]
        steps: Option<u32>,
        /// Surface max d'un clip animé, en pixels.
        #[arg(long, default_value_t = 640 * 352)]
        max_area: u32,
    },
    /// Enchaîne analyze → voice → timeline → render.
    Run {
        project: String,
        #[arg(long, default_value = "magi")]
        detector: String,
        #[arg(long)]
        no_animate: bool,
    },
    /// Installe les modèles lourds.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Supprime tout : le dossier de l'app (données, modèles, caches compris) et ses liens.
    Uninstall {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SourceCmd {
    /// Cherche une série.
    Search {
        query: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long)]
        lang: Option<String>,
    },
    /// Liste les chapitres d'une série dans une langue.
    Chapters {
        id: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Télécharge des chapitres dans la bibliothèque.
    Fetch {
        id: String,
        /// 101-110, 1,3,5-7 ou all.
        #[arg(long)]
        chapters: String,
        #[arg(long, default_value = "mangadex")]
        source: String,
        #[arg(long, default_value = "en")]
        lang: String,
        /// Titre dans la bibliothèque (défaut : titre de la série).
        #[arg(long)]
        title: Option<String>,
        /// Préfère ce groupe quand plusieurs versions d'un chapitre existent.
        #[arg(long)]
        group: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Crée un projet sur une plage de pages : --from ch101:p1 --to ch120:p20.
    New {
        name: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        from: PagePos,
        #[arg(long)]
        to: PagePos,
        /// Langue du texte des planches (fr, en, ja, es, zh, ko…).
        #[arg(long)]
        lang: String,
        /// rtl pour le manga, ltr pour comics/webtoons/BD.
        #[arg(long, value_enum, default_value = "rtl")]
        direction: Direction,
    },
    List,
    Show {
        name: String,
    },
}

#[derive(Subcommand)]
enum ModelsCmd {
    /// Télécharge et convertit un modèle : wan (animation, ~34 Go).
    Install {
        model: String,
        /// Quantification du transformer : 4 (adapté à 16 Go) ou 8 (qualité, 24 Go et plus).
        #[arg(long, default_value_t = 4)]
        bits: u32,
    },
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
    if !matches!(detector, "magi" | "vision") {
        bail!("détecteur inconnu : {detector} (magi ou vision)");
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
        println!("toutes les pages sont déjà analysées (--force pour recommencer)");
        return Ok(());
    }
    // Banque de personnages : characters/<Nom>.png, pour nommer les locuteurs.
    let mut character_bank = Vec::new();
    if let Ok(entries) = std::fs::read_dir(project.path("characters")) {
        for p in entries.flatten().map(|e| e.path()).filter(|p| home::is_image(p)) {
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            character_bank.push(BankEntry { name, image: p.to_string_lossy().into() });
        }
    }
    println!("analyse de {} pages ({detector}, {} persos connus)", pages.len(), character_bank.len());
    let job = AnalyzeJob {
        lang: project.cfg.lang.clone(),
        direction: project.cfg.direction,
        detector: detector.into(),
        character_bank,
        pages,
    };
    worker::run("analyze", &job, &project.path("work/analyze_job.json"))?;
    // Relit tout : valide le JSON produit par le worker.
    let mut lines = 0;
    for p in &project.cfg.pages {
        lines += analysis::PageAnalysis::load(&project.analysis_path(p))?.texts.iter().filter(|t| t.essential).count();
    }
    println!("{lines} répliques détectées. Vérifie avec : manga read {}", project.cfg.name);
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
    println!("vidéo : {}", path.display());
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Doctor => doctor::run(),
        Cmd::Source { cmd } => match cmd {
            SourceCmd::Search { query, source, lang } => {
                for hit in sources::by_name(&source)?.search(&query, lang.as_deref())? {
                    println!(
                        "{}  {}{}  [{}]  langues : {}",
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
                    println!("aucun chapitre téléchargeable en « {lang} » : titre sous licence retiré de MangaDex, ou pas de traduction. Utilise tes propres fichiers avec `manga import`.");
                }
                for c in &chapters {
                    let num = c.chapter.map(library::chapter_dir_name).unwrap_or_else(|| "?".into());
                    let state = match &c.external_url {
                        Some(url) => format!("externe (éditeur) : {url}"),
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
                // Une version par numéro : téléchargeable d'abord, puis le groupe demandé.
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
                    bail!("aucun chapitre « {chapters} » en « {lang} » pour cette série");
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
                            println!("ch {} : {pages} pages ({})", library::chapter_dir_name(n), c.group);
                        }
                        Err(e) => eprintln!("ch {} ignoré : {e:#}", library::chapter_dir_name(n)),
                    }
                }
                println!("{fetched}/{} chapitres dans {}", best.len(), library::title_dir(&title).display());
            }
        },
        Cmd::Import { path, title, chapter } => {
            let title = title.unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "sans-titre".into()));
            for (n, pages) in library::import(&path, &title, chapter)? {
                println!("{} ch {} : {pages} pages", home::slugify(&title), library::chapter_dir_name(n));
            }
        }
        Cmd::Library => {
            let titles = library::list_titles()?;
            if titles.is_empty() {
                println!("bibliothèque vide : manga import <fichier> ou manga source fetch …");
            }
            for (title, n) in titles {
                let chapters = library::chapters(&title)?;
                let range = match (chapters.first(), chapters.last()) {
                    (Some(a), Some(b)) => format!("ch {} → {}", library::chapter_dir_name(a.0), library::chapter_dir_name(b.0)),
                    _ => String::new(),
                };
                println!("{title:<40} {n:>4} chapitres  {range}");
            }
        }
        Cmd::Project { cmd } => match cmd {
            ProjectCmd::New { name, title, from, to, lang, direction } => {
                let p = Project::create(&name, &title, from, to, &lang, direction)?;
                println!("projet {} : {} pages ({} → {})\n{}", p.cfg.name, p.cfg.pages.len(), p.cfg.from, p.cfg.to, p.root.display());
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
                println!("dossier   : {}", p.root.display());
                println!("pages     : {} ({analyzed} analysées)", p.cfg.pages.len());
                println!("voix      : {audio} répliques");
                match Timeline::load(&p) {
                    Ok(tl) => {
                        let animated = tl.shots.iter().filter(|s| s.mode == Mode::Animate && !s.skip).count();
                        println!("timeline  : {} plans, {:.0}s, {animated} animés", tl.shots.len(), tl.total());
                    }
                    Err(_) => println!("timeline  : absente"),
                }
            }
        },
        Cmd::Analyze { project, detector, force } => analyze(&Project::open(&project)?, &detector, force)?,
        Cmd::Read { project, port, no_open } => reader::serve(Project::open(&project)?, port, !no_open)?,
        Cmd::Voice { project, force } => voice::run(&Project::open(&project)?, force)?,
        Cmd::Timeline { project, page_intro, reset } => {
            let p = Project::open(&project)?;
            let opts = timeline::Options { page_intro, ..Default::default() };
            let tl = timeline::run(&p, &opts, reset)?;
            println!("{} plans, {:.0}s → {}", tl.shots.len(), tl.total(), p.path("timeline.json").display());
        }
        Cmd::Animate { project, shots, prompt, off } => {
            let p = Project::open(&project)?;
            let mut tl = Timeline::load(&p)?;
            let ids: Vec<String> = render::select(&tl, &shots)?.iter().map(|s| s.id.clone()).collect();
            if ids.is_empty() {
                bail!("indique au moins un plan (identifiant ou numéro)");
            }
            for shot in tl.shots.iter_mut().filter(|s| ids.contains(&s.id)) {
                shot.mode = if off { Mode::Motion } else { Mode::Animate };
                if prompt.is_some() {
                    shot.prompt = prompt.clone();
                }
                println!("{} → {:?}", shot.id, shot.mode);
            }
            tl.save(&p)?;
        }
        Cmd::Render { project, out, shots, style, no_animate, speaker, no_captions, captions, jobs, frames, steps, max_area } => {
            let settings = animate::Settings { frames, steps, max_area, ..Default::default() };
            let captions = captions || (!no_captions && style == render::Style::Panels);
            let opts = render::RenderOptions { out: PathBuf::new(), style, shots, show_speaker: speaker, captions, jobs };
            render(&Project::open(&project)?, out, opts, no_animate, settings)?;
        }
        Cmd::Run { project, detector, no_animate } => {
            let p = Project::open(&project)?;
            analyze(&p, &detector, false)?;
            voice::run(&p, false)?;
            timeline::run(&p, &timeline::Options::default(), false)?;
            let opts = render::RenderOptions { out: PathBuf::new(), style: render::Style::Scroll, shots: Vec::new(), show_speaker: false, captions: false, jobs: 4 };
            render(&p, None, opts, no_animate, animate::Settings::default())?;
        }
        Cmd::Uninstall { yes } => uninstall::run(yes)?,
        Cmd::Models { cmd } => match cmd {
            ModelsCmd::Install { model, bits } => {
                if model != "wan" {
                    bail!("modèle inconnu : {model} (disponible : wan)");
                }
                let out = animate::model_dir();
                let job = serde_json::json!({
                    "model": "wan",
                    "src_dir": home::data_dir().join("models/wan2.2-ti2v-5b-src"),
                    "out_dir": out,
                    "bits": bits,
                });
                worker::run("models", &job, &home::data_dir().join("models/install_job.json"))?;
            }
        },
    }
    Ok(())
}
