//! Voices: `cast.toml` maps each character to a voice, then the `tts` worker generates one
//! WAV per bubble, with Kokoro or through OpenRouter. The `audio/index.json` cache avoids
//! regenerating a bubble whose text, voice and backend haven't changed.

use std::collections::{BTreeMap, HashMap};
use std::fs;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::PageAnalysis;
use crate::backend::Backend;
use crate::project::Project;
use crate::worker;

pub const TTS_MODEL: &str = "mlx-community/Kokoro-82M-bf16";
const KOKORO_LANGS: [&str; 8] = ["en", "fr", "ja", "zh", "es", "it", "pt", "hi"];

fn is_kokoro_voice(voice: &str) -> bool {
    KOKORO_LANGS.iter().filter_map(|l| voices_for(l)).flatten().any(|v| v.split('@').next() == Some(voice))
}

/// Kokoro reads the language from the first letter of the voice and aborts on any other.
fn has_kokoro_prefix(voice: &str) -> bool {
    voice.starts_with(['a', 'b', 'e', 'f', 'h', 'i', 'j', 'p', 'z'])
}

/// Cache key of a line. Local keys are unchanged, so existing audio stays valid.
fn line_hash(text: &str, voice: &str, speed: f64, backend: &Backend) -> String {
    hex::encode(Sha256::digest(format!("{text}\n{voice}\n{speed}\n{TTS_MODEL}{}", backend.cache_suffix())))
}

/// Kokoro voices per language: the first is for the narrator, the others for characters.
fn voices_for(lang: &str) -> Option<&'static [&'static str]> {
    Some(match lang {
        "en" => &["af_heart", "am_adam", "af_bella", "am_michael", "bf_emma", "bm_george", "af_nicole", "am_fenrir", "bf_alice", "bm_daniel"],
        "fr" => &["ff_siwis", "ff_siwis@0.9", "ff_siwis@1.1"],
        "ja" => &["jf_alpha", "jm_kumo", "jf_gongitsune", "jf_nezumi", "jf_tebukuro"],
        "zh" => &["zf_xiaobei", "zm_yunxi", "zf_xiaoni", "zm_yunjian", "zf_xiaoxiao", "zm_yunyang"],
        "es" => &["ef_dora", "em_alex", "em_santa"],
        "it" => &["if_sara", "im_nicola"],
        "pt" => &["pf_dora", "pm_alex", "pm_santa"],
        "hi" => &["hf_alpha", "hm_omega", "hf_beta", "hm_psi"],
        _ => return None,
    })
}

pub fn default_cast(lang: &str) -> String {
    let Some(voices) = voices_for(lang) else {
        return format!(
            "# Kokoro has no voice for \"{lang}\": the video will be captioned without voices.\n\
             # Available voices: https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md\n\
             enabled = false\nspeed = 1.0\nnarrator = \"af_heart\"\npool = [\"af_heart\"]\n\n[voices]\n"
        );
    };
    let pool: Vec<String> = voices.iter().skip(1).map(|v| format!("\"{v}\"")).collect();
    format!(
        "# Kokoro voices: https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md\n\
         # \"voice@0.9\" = same voice at speed 0.9. The first letter of the voice sets the language.\n\
         # With `manga voice --backend openrouter`, use the voice ids of that model instead.\n\
         enabled = true\n\
         speed = 1.0\n\
         # Caption boxes and bubbles with no identified speaker.\n\
         narrator = \"{}\"\n\
         # Assigned in order to characters not listed in [voices].\n\
         pool = [{}]\n\n\
         [voices]\n\
         # Thorfinn = \"am_adam\"\n",
        voices[0],
        if pool.is_empty() { format!("\"{}\"", voices[0]) } else { pool.join(", ") }
    )
}

#[derive(Debug, Deserialize)]
pub struct Cast {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub speed: f64,
    pub narrator: String,
    pub pool: Vec<String>,
    #[serde(default)]
    pub voices: BTreeMap<String, String>,
}

fn enabled() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

impl Cast {
    pub fn load(project: &Project) -> Result<Self> {
        let path = project.path("cast.toml");
        if !path.exists() {
            fs::write(&path, default_cast(&project.cfg.lang))?;
        }
        Ok(toml::from_str(&fs::read_to_string(path)?)?)
    }
}

/// Assigns a voice to each speaker, stably: in order of first appearance.
pub struct Casting<'a> {
    cast: &'a Cast,
    assigned: HashMap<String, String>,
}

impl<'a> Casting<'a> {
    pub fn new(cast: &'a Cast) -> Self {
        Self { cast, assigned: HashMap::new() }
    }

    /// (Kokoro voice, speed)
    pub fn voice(&mut self, speaker: Option<&str>) -> (String, f64) {
        let spec = match speaker {
            None => self.cast.narrator.clone(),
            Some(s) => match self.cast.voices.get(s) {
                Some(v) => v.clone(),
                None => {
                    let n = self.assigned.len();
                    let pool = &self.cast.pool;
                    self.assigned
                        .entry(s.to_string())
                        .or_insert_with(|| if pool.is_empty() { self.cast.narrator.clone() } else { pool[n % pool.len()].clone() })
                        .clone()
                }
            },
        };
        match spec.split_once('@') {
            Some((v, s)) => (v.to_string(), s.parse::<f64>().unwrap_or(1.0) * self.cast.speed),
            None => (spec, self.cast.speed),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AudioIndex {
    /// bubble id → (text+voice hash, duration in s)
    pub clips: BTreeMap<String, (String, f64)>,
}

impl AudioIndex {
    pub fn load(project: &Project) -> Self {
        fs::read_to_string(project.path("audio/index.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn duration(&self, id: &str) -> Option<f64> {
        self.clips.get(id).map(|c| c.1)
    }
}

#[derive(Serialize)]
struct TtsItem {
    id: String,
    text: String,
    voice: String,
    speed: f64,
    out: String,
}

#[derive(Serialize)]
struct TtsJob {
    /// Absent for the local path. Never holds the API key: the worker reads it from the environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<&'static str>,
    model: String,
    items: Vec<TtsItem>,
    results: String,
}

/// Makes text speakable: `...` creates odd silences, and all-caps text
/// (manga lettering) risks being spelled out like acronyms.
pub fn speakable(text: &str) -> String {
    let t = text.replace('…', "...").replace("...", ", ");
    let t = t.trim_start_matches([',', ' ']).trim();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    let upper = letters.iter().filter(|c| c.is_uppercase()).count();
    if letters.len() < 4 || upper * 10 < letters.len() * 7 {
        return t;
    }
    let mut out = String::with_capacity(t.len());
    let mut start = true;
    for c in t.chars() {
        if start && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            start = false;
        } else {
            out.extend(c.to_lowercase());
        }
        if matches!(c, '.' | '!' | '?') {
            start = true;
        }
    }
    // The English pronoun "I" stays capitalized (I, I'm, I'll…).
    out.split(' ')
        .map(|w| if w == "i" || w.starts_with("i'") { format!("I{}", &w[1..]) } else { w.to_string() })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn run(project: &Project, force: bool, backend: &Backend) -> Result<()> {
    let cast = Cast::load(project)?;
    if !cast.enabled {
        println!("voices disabled in cast.toml (enabled = false): nothing to do");
        return Ok(());
    }
    let mut casting = Casting::new(&cast);
    let mut index = AudioIndex::load(project);
    let mut items = Vec::new();
    let mut hashes = HashMap::new();
    let mut wanted = Vec::new();
    for page in &project.cfg.pages {
        let analysis = PageAnalysis::load(&project.analysis_path(page))?;
        for t in analysis.texts.iter().filter(|t| t.essential) {
            let text = speakable(&t.text);
            if text.chars().all(|c| !c.is_alphanumeric()) {
                continue;
            }
            let (voice, speed) = casting.voice(t.speaker.as_deref());
            let hash = line_hash(&text, &voice, speed, backend);
            let out = project.path(format!("audio/{}.wav", t.id));
            wanted.push(t.id.clone());
            if !force && out.exists() && index.clips.get(&t.id).is_some_and(|c| c.0 == hash) {
                continue;
            }
            hashes.insert(t.id.clone(), hash);
            items.push(TtsItem { id: t.id.clone(), text, voice, speed, out: out.to_string_lossy().into() });
        }
    }
    index.clips.retain(|id, _| wanted.contains(id));
    if items.is_empty() {
        println!("voices up to date ({} lines)", wanted.len());
    } else {
        println!("{} lines to generate ({} already up to date)", items.len(), wanted.len() - items.len());
        let results = project.path("work/tts_durations.json");
        let (name, model) = match backend {
            Backend::Local => {
                if let Some(item) = items.iter().find(|i| !has_kokoro_prefix(&i.voice)) {
                    bail!("cast.toml uses \"{}\", which is not a Kokoro voice: see https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md", item.voice);
                }
                (None, TTS_MODEL.to_string())
            }
            Backend::OpenRouter { model } => {
                if let Some(item) = items.iter().find(|i| is_kokoro_voice(&i.voice)) {
                    bail!(
                        "cast.toml uses the Kokoro voice \"{}\", which {model} does not know: set narrator, pool and [voices] to voice ids of that model",
                        item.voice
                    );
                }
                backend.check_key()?;
                println!("using {model} on OpenRouter (billed per use; the text is sent to third parties)");
                (Some("openrouter"), model.clone())
            }
        };
        // A leftover file from an earlier run must not pass for this run's results.
        let _ = fs::remove_file(&results);
        let job = TtsJob { backend: name, model, items, results: results.to_string_lossy().into() };
        let outcome = worker::run("tts", &job, &project.path("work/tts_job.json"));
        if let Err(e) = outcome {
            // An interrupted OpenRouter run leaves partial results: keep the lines already paid for.
            if let Ok(text) = fs::read_to_string(&results) {
                let durations: HashMap<String, f64> = serde_json::from_str(&text)?;
                for (id, hash) in hashes {
                    if let Some(d) = durations.get(&id) {
                        index.clips.insert(id, (hash, *d));
                    }
                }
                fs::create_dir_all(project.path("audio"))?;
                fs::write(project.path("audio/index.json"), serde_json::to_string_pretty(&index)?)?;
            }
            return Err(e);
        }
        let durations: HashMap<String, f64> = serde_json::from_str(&fs::read_to_string(&results)?)?;
        for (id, hash) in hashes {
            let Some(d) = durations.get(&id) else { bail!("missing duration for {id}") };
            index.clips.insert(id, (hash, *d));
        }
    }
    fs::create_dir_all(project.path("audio"))?;
    fs::write(project.path("audio/index.json"), serde_json::to_string_pretty(&index)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openrouter(model: &str) -> Backend {
        Backend::OpenRouter { model: model.into() }
    }

    #[test]
    fn local_line_hash_is_unchanged() {
        let expected = hex::encode(Sha256::digest(format!("Hi\nam_adam\n1\n{TTS_MODEL}")));
        assert_eq!(line_hash("Hi", "am_adam", 1.0, &Backend::Local), expected);
    }

    #[test]
    fn line_hash_depends_on_backend_and_model() {
        let key = |b: &Backend| line_hash("Hi", "alloy", 1.0, b);
        assert_ne!(key(&Backend::Local), key(&openrouter("a/b")));
        assert_ne!(key(&openrouter("a/b")), key(&openrouter("a/c")));
    }

    #[test]
    fn kokoro_voices_are_recognized() {
        assert!(is_kokoro_voice("af_heart"));
        assert!(is_kokoro_voice("ff_siwis"));
        assert!(!is_kokoro_voice("alloy"));
        assert!(!is_kokoro_voice("en-US-Harper:MAI-Voice-2.1"));
    }

    #[test]
    fn kokoro_prefix_covers_every_listed_voice() {
        assert!(KOKORO_LANGS.iter().filter_map(|l| voices_for(l)).flatten().all(|v| has_kokoro_prefix(v)));
        assert!(!has_kokoro_prefix("does_not_exist"));
        assert!(!has_kokoro_prefix("xf_nope"));
    }

    #[test]
    fn casting_is_stable() {
        let cast: Cast = toml::from_str(&default_cast("en")).unwrap();
        let mut c = Casting::new(&cast);
        let a = c.voice(Some("character-1"));
        let b = c.voice(Some("character-2"));
        assert_ne!(a, b);
        assert_eq!(c.voice(Some("character-1")), a);
        assert_eq!(c.voice(None).0, "af_heart");
    }

    #[test]
    fn speed_suffix() {
        let cast: Cast = toml::from_str(&default_cast("fr")).unwrap();
        let mut c = Casting::new(&cast);
        assert_eq!(c.voice(Some("x")), ("ff_siwis".into(), 0.9));
    }

    #[test]
    fn unsupported_lang_disables_voice() {
        let cast: Cast = toml::from_str(&default_cast("ko")).unwrap();
        assert!(!cast.enabled);
    }

    #[test]
    fn speakable_text() {
        assert_eq!(speakable("... and the final touch."), "and the final touch.");
        assert_eq!(speakable("What...  really"), "What, really");
        assert_eq!(speakable("IF YOU DON'T LEAVE, I'LL CUT YOU. NOW!"), "If you don't leave, I'll cut you. Now!");
        assert_eq!(speakable("NOO"), "NOO");
    }
}
