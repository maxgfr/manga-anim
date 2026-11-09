//! Command-level regressions: real binary, throwaway MANGA_HOME, no worker and no model.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Env {
    dir: TempDir,
    home: PathBuf,
    /// Put first on PATH: fake external tools.
    bin: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        Self { dir, home, bin }
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        Command::new(env!("CARGO_BIN_EXE_manga")).args(args).current_dir(cwd).env("MANGA_HOME", &self.home).env("PATH", path).output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(self.dir.path(), args)
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(out.status.success(), "manga {args:?} failed: {}", text(&out));
        text(&out)
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(out.status.code(), Some(1), "manga {args:?} should fail cleanly: {}", text(&out));
        text(&out)
    }

    fn fake_tool(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A folder of `n` valid one-chapter pages, under the test directory.
    fn pages(&self, rel: &str, n: usize) -> PathBuf {
        let dir = self.dir.path().join(rel);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            image::RgbImage::new(16, 24).save(dir.join(format!("{i}.png"))).unwrap();
        }
        dir
    }

    fn library(&self, rel: &str) -> PathBuf {
        self.home.join("library").join(rel)
    }

    fn project(&self, rel: &str) -> PathBuf {
        self.home.join("projects").join(rel)
    }

    /// Library title `t` with chapter 1 of two pages, and a project over it.
    fn with_project(&self, name: &str) {
        let src = self.pages("src-pages", 2);
        self.ok(&["import", src.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
        self.ok(&["project", "new", name, "--title", "t", "--from", "ch1", "--to", "ch1", "--lang", "en"]);
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

#[test]
fn import_of_the_current_folder_uses_its_name() {
    let env = Env::new();
    let dir = env.pages("My Book ch3", 1);
    let out = env.run_in(&dir, &["import", "."]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(names(&env.library("my-book-ch3/3")), ["001.png"]);
}

#[test]
fn folder_title_keeps_its_dots() {
    let env = Env::new();
    let dir = env.pages("Dr. Stone", 1);
    env.ok(&["import", dir.to_str().unwrap()]);
    assert!(env.library("dr-stone").is_dir());
}

#[test]
fn series_folders_keep_decimal_chapters() {
    let env = Env::new();
    for ch in ["series/Chapter 1", "series/Chapter 1.5"] {
        env.pages(ch, 1);
    }
    let series = env.dir.path().join("series");
    env.ok(&["import", series.to_str().unwrap(), "--title", "s"]);
    assert_eq!(names(&env.library("s")), ["1", "1.5"]);
}

#[test]
fn series_folder_ignores_macos_junk() {
    let env = Env::new();
    env.pages("series/ch 1", 1);
    let junk = env.dir.path().join("series/__MACOSX");
    fs::create_dir_all(&junk).unwrap();
    fs::write(junk.join("._x"), b"x").unwrap();
    let series = env.dir.path().join("series");
    env.ok(&["import", series.to_str().unwrap(), "--title", "s"]);
    assert_eq!(names(&env.library("s")), ["1"]);
}

#[test]
fn series_entries_with_the_same_chapter_are_refused() {
    let env = Env::new();
    env.pages("series/Vol 1 Ch 1", 1);
    env.pages("series/Vol 2 Ch 1", 2);
    let series = env.dir.path().join("series");
    let err = env.fails(&["import", series.to_str().unwrap(), "--title", "s"]);
    assert!(err.contains("several entries are chapter 1"), "{err}");
    assert_eq!(names(&env.library("s/1")), ["001.png"], "the first entry was overwritten");
}

#[test]
fn analyze_names_an_unreadable_page_before_starting() {
    let env = Env::new();
    let dir = env.pages("pages", 2);
    fs::write(dir.join("1.png"), b"not a png").unwrap();
    env.ok(&["import", dir.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
    env.ok(&["project", "new", "p", "--title", "t", "--from", "ch1", "--to", "ch1", "--lang", "en"]);
    let err = env.fails(&["analyze", "p", "--detector", "vision"]);
    assert!(err.contains("unreadable page") && err.contains("0002.png"), "{err}");
    assert!(!env.project("p/analysis").exists());
}

#[test]
fn chapter_flag_is_refused_on_a_series() {
    let env = Env::new();
    env.pages("series/ch 1", 1);
    let series = env.dir.path().join("series");
    let err = env.fails(&["import", series.to_str().unwrap(), "--title", "s", "--chapter", "9"]);
    assert!(err.contains("--chapter"), "{err}");
    assert!(!env.library("s").exists());
}

#[test]
fn title_without_letters_is_refused() {
    let env = Env::new();
    let dir = env.pages("pages", 1);
    env.fails(&["import", dir.to_str().unwrap(), "--title", "???", "--chapter", "1"]);
    assert!(!env.library("1").exists(), "chapter written at the library root");
}

#[test]
fn missing_input_is_named_in_the_error() {
    let env = Env::new();
    let err = env.fails(&["import", "nowhere.cbz", "--title", "x"]);
    assert!(err.contains("nowhere.cbz"), "{err}");
}

#[test]
fn failed_pdf_keeps_the_existing_chapter() {
    let env = Env::new();
    let dir = env.pages("pages", 2);
    env.ok(&["import", dir.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
    env.fake_tool("pdftoppm", "exit 1");
    let pdf = env.dir.path().join("bad.pdf");
    fs::write(&pdf, b"nope").unwrap();
    env.fails(&["import", pdf.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
    assert_eq!(names(&env.library("t/1")), ["001.png", "002.png"]);
    env.fails(&["import", pdf.to_str().unwrap(), "--title", "new", "--chapter", "1"]);
    assert!(!env.library("new/1").exists(), "empty chapter left behind");
}

#[test]
fn archive_without_images_creates_no_chapter() {
    let env = Env::new();
    env.fake_tool("unar", "exit 0");
    let archive = env.dir.path().join("empty.7z");
    fs::write(&archive, b"x").unwrap();
    let err = env.fails(&["import", archive.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
    assert!(err.contains("no images"), "{err}");
    assert!(!env.library("t/1").exists());
}

#[test]
fn existing_project_is_not_overwritten() {
    let env = Env::new();
    env.with_project("p");
    fs::write(env.project("p/marker"), b"x").unwrap();
    let err = env.fails(&["project", "new", "p", "--title", "t", "--from", "ch1:p2", "--to", "ch1:p2", "--lang", "en"]);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(names(&env.project("p/pages")).len(), 2);
}

#[test]
fn project_name_without_letters_is_refused() {
    let env = Env::new();
    let dir = env.pages("pages", 1);
    env.ok(&["import", dir.to_str().unwrap(), "--title", "t", "--chapter", "1"]);
    env.fails(&["project", "new", "???", "--title", "t", "--from", "ch1", "--to", "ch1", "--lang", "en"]);
    assert!(!env.home.join("projects/project.toml").exists(), "project written at the projects root");
}

#[test]
fn animate_without_shots_changes_nothing() {
    let env = Env::new();
    env.with_project("p");
    let timeline = r#"{"width":1920,"height":1080,"fps":30,"shots":[
        {"id":"p0001_k0","page":"0001.png","bbox":[0,0,16,24],"duration":2.0,"motion":"static","mode":"motion","lines":[]}]}"#;
    fs::write(env.project("p/timeline.json"), timeline).unwrap();
    let err = env.fails(&["animate", "p"]);
    assert!(err.contains("at least one shot"), "{err}");
    assert_eq!(fs::read_to_string(env.project("p/timeline.json")).unwrap(), timeline);
}

const ANIME_TIMELINE: &str = r#"{"width":1920,"height":1080,"fps":30,"shots":[
    {"id":"p0001_page","page":"0001.png","bbox":[0,0,16,24],"duration":1.5,"motion":"static","mode":"motion","lines":[]},
    {"id":"p0001_k0","page":"0001.png","bbox":[0,0,16,12],"duration":2.2,"motion":"zoom_in","mode":"motion","lines":[]},
    {"id":"p0001_k1","page":"0001.png","bbox":[0,12,16,24],"duration":3.0,"motion":"zoom_out","mode":"animate","lines":[]}]}"#;

fn anime_project() -> Env {
    let env = Env::new();
    env.with_project("p");
    fs::write(env.project("p/timeline.json"), ANIME_TIMELINE).unwrap();
    env
}

const ANIME_API: [&str; 7] = ["--backend", "openrouter", "--image-model", "i/m", "--video-model", "v/m", "--dry-run"];

fn anime_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["anime", "p"];
    args.extend(ANIME_API);
    args.extend(extra);
    args
}

#[test]
fn anime_dry_run_counts_without_a_key_or_a_model() {
    let env = anime_project();
    let out = env.ok(&anime_args(&[]));
    assert!(out.contains("2 shots: 2 key images to draw, 2 clips to generate (about 6 s of video)"), "{out}");
    assert_eq!(fs::read_to_string(env.project("p/timeline.json")).unwrap(), ANIME_TIMELINE, "marks stay out of the timeline");
}

#[test]
fn anime_key_shots_only_pays_for_the_marked_shots() {
    let env = anime_project();
    let out = env.ok(&anime_args(&["--key-shots-only"]));
    assert!(out.contains("2 key images to draw, 1 clips to generate (about 3 s of video)"), "{out}");
}

#[test]
fn anime_refuses_to_start_over_the_limit() {
    let env = anime_project();
    let mut args = anime_args(&["--max-seconds", "5"]);
    args.retain(|a| *a != "--dry-run");
    let err = env.fails(&args);
    assert!(err.contains("over --max-seconds 5"), "{err}");
}

#[test]
fn anime_needs_the_key_before_any_work() {
    let env = anime_project();
    let mut args = anime_args(&[]);
    args.retain(|a| *a != "--dry-run");
    let out = {
        let path = format!("{}:{}", env.bin.display(), std::env::var("PATH").unwrap());
        Command::new(env!("CARGO_BIN_EXE_manga")).args(&args).current_dir(env.dir.path()).env("MANGA_HOME", &env.home).env("PATH", path).env_remove("OPENROUTER_API_KEY").output().unwrap()
    };
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("OPENROUTER_API_KEY is not set"), "{}", text(&out));
}

#[test]
fn anime_max_duration_only_counts_what_is_kept() {
    let env = anime_project();
    let out = env.ok(&anime_args(&["--max-duration", "3"]));
    assert!(out.contains("1 shots: 1 key images to draw, 1 clips to generate (about 3 s of video)"), "{out}");
}

#[test]
fn anime_split_counts_every_part() {
    let env = anime_project();
    let out = env.ok(&anime_args(&["--max-duration", "3", "--split"]));
    assert!(out.contains("2 shots: 2 key images to draw, 2 clips"), "{out}");
}

#[test]
fn split_needs_a_duration() {
    let env = anime_project();
    let out = env.run(&anime_args(&["--split"]));
    assert!(!out.status.success());
    assert!(text(&out).contains("--max-duration"), "{}", text(&out));
}

#[test]
fn anime_counts_the_faces_to_remember_and_writes_nothing_on_a_dry_run() {
    let env = anime_project();
    fs::create_dir_all(env.project("p/characters")).unwrap();
    image::RgbImage::new(8, 8).save(env.project("p/characters/Kit.png")).unwrap();
    let timeline = ANIME_TIMELINE.replace(r#""mode":"motion","lines":[]},
    {"id":"p0001_k1""#, r#""mode":"motion","characters":["Kit"],"lines":[]},
    {"id":"p0001_k1""#);
    fs::write(env.project("p/timeline.json"), timeline).unwrap();
    let out = env.ok(&anime_args(&[]));
    assert!(out.contains("1 character faces to draw and remember"), "{out}");
    assert!(!env.project("p/anime.toml").exists());
}

#[test]
fn animate_stores_sound_and_characters() {
    let env = anime_project();
    env.ok(&["animate", "p", "p0001_k0", "--sound", "door knock", "--characters", "Kit,Ann"]);
    let saved = fs::read_to_string(env.project("p/timeline.json")).unwrap();
    assert!(saved.contains(r#""sound": "door knock""#), "{saved}");
    assert!(saved.contains(r#""Ann""#), "{saved}");
}

#[test]
fn anime_style_is_not_a_render_option() {
    let env = anime_project();
    let err = env.run(&["render", "p", "--style", "anime"]);
    assert!(!err.status.success());
    assert!(text(&err).contains("invalid value"), "{}", text(&err));
}

#[test]
fn disabled_voices_leave_the_timeline_silent() {
    let env = Env::new();
    env.with_project("p");
    fs::create_dir_all(env.project("p/analysis")).unwrap();
    fs::create_dir_all(env.project("p/audio")).unwrap();
    for page in ["0001", "0002"] {
        let analysis = format!(
            r#"{{"page":"{page}.png","width":16,"height":24,"panels":[{{"bbox":[0,0,16,24]}}],
            "texts":[{{"id":"{page}_t0","bbox":[1,1,9,9],"panel":0,"text":"Hello","speaker":null,"essential":true}}]}}"#
        );
        fs::write(env.project(&format!("p/analysis/{page}.json")), analysis).unwrap();
    }
    fs::write(env.project("p/audio/index.json"), r#"{"clips":{"0001_t0":["h",2.0]}}"#).unwrap();
    env.ok(&["timeline", "p"]);
    assert!(fs::read_to_string(env.project("p/timeline.json")).unwrap().contains("audio/0001_t0.wav"), "voice used while enabled");
    let cast = fs::read_to_string(env.project("p/cast.toml")).unwrap();
    fs::write(env.project("p/cast.toml"), cast.replace("enabled = true", "enabled = false")).unwrap();
    env.ok(&["timeline", "p"]);
    assert!(!fs::read_to_string(env.project("p/timeline.json")).unwrap().contains("audio/0001_t0.wav"), "voice used while disabled");
}

#[test]
fn story_new_does_not_take_over_a_regular_project() {
    let env = Env::new();
    env.with_project("src");
    env.ok(&["project", "new", "other", "--title", "t", "--from", "ch1", "--to", "ch1", "--lang", "en"]);
    let err = env.fails(&["story", "new", "other", "--from", "src"]);
    assert!(err.contains("already exists"), "{err}");
    assert!(!env.project("other/story.json").exists());
}

#[test]
fn story_name_without_letters_is_refused() {
    let env = Env::new();
    env.with_project("src");
    env.fails(&["story", "new", "???", "--from", "src"]);
    assert!(!env.home.join("projects/story.json").exists());
}

#[test]
fn story_build_refuses_unknown_pages() {
    let env = Env::new();
    env.with_project("src");
    env.ok(&["story", "new", "spin", "--from", "src"]);
    for page in ["0", "99"] {
        let err = env.fails(&["story", "build", "spin", "--pages", page]);
        assert!(err.contains("the story has 1 pages"), "{err}");
    }
}
