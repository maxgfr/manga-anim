//! Where a generation step runs: local models (the default), OpenRouter, or a provider reached
//! through llm-connect (a small Node worker, see `connect.rs`). Also how `--detector api` reviews.

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::Serialize;

pub const KEY_ENV: &str = "OPENROUTER_API_KEY";

/// The value of `--backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    Local,
    #[value(name = "openrouter")]
    OpenRouter,
    Connect,
}

/// What a backend generates, which decides the providers `--backend connect` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Speech,
    Video,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Image => "image generation",
            Kind::Speech => "speech",
            Kind::Video => "video generation",
        }
    }
}

struct ConnectProvider {
    id: &'static str,
    /// The environment variable llm-connect documents for this provider.
    env: &'static str,
    key_url: &'static str,
    kinds: &'static [Kind],
}

/// What llm-connect actually supports per kind (`IMAGE_PROVIDERS`, `SPEECH_PROVIDERS`,
/// `VIDEO_PROVIDERS`), without the aggregators: OpenRouter has its own backend here.
const CONNECT_PROVIDERS: [ConnectProvider; 6] = [
    ConnectProvider { id: "fal", env: "FAL_KEY", key_url: "https://fal.ai/dashboard/keys", kinds: &[Kind::Image, Kind::Speech, Kind::Video] },
    ConnectProvider { id: "openai", env: "OPENAI_API_KEY", key_url: "https://platform.openai.com/api-keys", kinds: &[Kind::Image, Kind::Speech] },
    ConnectProvider { id: "google", env: "GEMINI_API_KEY", key_url: "https://aistudio.google.com/apikey", kinds: &[Kind::Image, Kind::Speech, Kind::Video] },
    ConnectProvider { id: "xai", env: "XAI_API_KEY", key_url: "https://console.x.ai", kinds: &[Kind::Image, Kind::Video] },
    ConnectProvider { id: "replicate", env: "REPLICATE_API_TOKEN", key_url: "https://replicate.com/account/api-tokens", kinds: &[Kind::Image, Kind::Video] },
    ConnectProvider { id: "elevenlabs", env: "ELEVENLABS_API_KEY", key_url: "https://elevenlabs.io/app/settings/api-keys", kinds: &[Kind::Speech] },
];

fn connect_provider(id: &str) -> Option<&'static ConnectProvider> {
    CONNECT_PROVIDERS.iter().find(|p| p.id == id)
}

fn supported(kind: Kind) -> String {
    CONNECT_PROVIDERS.iter().filter(|p| p.kinds.contains(&kind)).map(|p| p.id).collect::<Vec<_>>().join(", ")
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Local,
    OpenRouter {
        model: String,
    },
    Connect {
        provider: String,
        model: String,
    },
}

fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Backend {
    /// Combines `--backend`, `--provider` and `--model`. No model is ever assumed: the choice
    /// sets what the user pays, and model ids change over time.
    pub fn parse(backend: BackendKind, provider: Option<String>, model: Option<String>, kind: Kind) -> Result<Self> {
        let (provider, model) = (clean(provider), clean(model));
        match backend {
            BackendKind::Local => {
                if provider.is_some() || model.is_some() {
                    bail!("--model only applies to --backend openrouter, --provider and --model to --backend connect");
                }
                Ok(Backend::Local)
            }
            BackendKind::OpenRouter => {
                if provider.is_some() {
                    bail!("--provider only applies to --backend connect");
                }
                let Some(model) = model else { bail!("--backend openrouter requires --model <id>: pick one at https://openrouter.ai/models") };
                Ok(Backend::OpenRouter { model })
            }
            BackendKind::Connect => {
                let Some(provider) = provider else { bail!("--backend connect requires --provider ({})", supported(kind)) };
                if !connect_provider(&provider).is_some_and(|p| p.kinds.contains(&kind)) {
                    bail!("{} is not available through {provider} with --backend connect: use one of {}", kind.name(), supported(kind));
                }
                let Some(model) = model else { bail!("--backend connect requires --model <id>: search with `manga models search`") };
                Ok(Backend::Connect { provider, model })
            }
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Backend::Local)
    }

    /// Appended to cache keys. Empty for local, so existing local caches stay valid.
    pub fn cache_suffix(&self) -> String {
        match self {
            Backend::Local => String::new(),
            Backend::OpenRouter { model } => format!("|openrouter|{model}"),
            Backend::Connect { provider, model } => format!("|connect|{provider}|{model}"),
        }
    }

    /// Stops before any work when the key is missing. The key's value is never read further.
    pub fn check_key(&self) -> Result<()> {
        match self {
            Backend::Local => Ok(()),
            Backend::OpenRouter { .. } => key_status(std::env::var(KEY_ENV).ok().as_deref()),
            Backend::Connect { provider, .. } => {
                let Some(info) = connect_provider(provider) else { bail!("unknown provider: {provider}") };
                require_env(info.env, info.key_url)
            }
        }
    }
}

/// `{env} is not set` unless the variable holds something. The value is never read further.
fn require_env(env: &str, key_url: &str) -> Result<()> {
    if std::env::var(env).ok().is_none_or(|k| k.trim().is_empty()) {
        bail!("{env} is not set: create a key at {key_url} and export it");
    }
    Ok(())
}

fn key_status(key: Option<&str>) -> Result<()> {
    if key.is_none_or(|k| k.trim().is_empty()) {
        bail!("{KEY_ENV} is not set: create a key at https://openrouter.ai/settings/keys and export it");
    }
    Ok(())
}

/// Whether a key is configured, for `manga doctor`.
pub fn key_configured() -> bool {
    key_status(std::env::var(KEY_ENV).ok().as_deref()).is_ok()
}

/// The names (never the values) of the `--backend connect` keys that are set, for `manga doctor`.
pub fn connect_keys_set() -> Vec<&'static str> {
    CONNECT_PROVIDERS.iter().filter(|p| require_env(p.env, p.key_url).is_ok()).map(|p| p.env).collect()
}

/// Providers of `--detector api` besides the default Anthropic path: (id, key variable, key page).
/// Each serves an OpenAI-compatible chat API with vision.
const REVIEW_PROVIDERS: [(&str, &str, &str); 3] = [
    ("openrouter", KEY_ENV, "https://openrouter.ai/settings/keys"),
    ("openai", "OPENAI_API_KEY", "https://platform.openai.com/api-keys"),
    ("google", "GEMINI_API_KEY", "https://aistudio.google.com/apikey"),
];

/// The reviewer of `--detector api`, absent from the job for the default Anthropic path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Review {
    pub provider: String,
    pub model: String,
}

impl Review {
    /// `None` keeps the default Anthropic reviewer. Flags outside `--detector api` are refused.
    pub fn parse(detector: &str, provider: Option<String>, model: Option<String>) -> Result<Option<Self>> {
        let (provider, model) = (clean(provider), clean(model));
        if detector != "api" {
            if provider.is_some() || model.is_some() {
                bail!("--provider and --model only apply to --detector api");
            }
            return Ok(None);
        }
        let names = || REVIEW_PROVIDERS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ");
        match provider.as_deref() {
            None | Some("anthropic") => {
                if model.is_some() {
                    bail!("--model needs --provider ({}): the default Anthropic reviewer uses a fixed model", names());
                }
                Ok(None)
            }
            Some(id) => {
                if !REVIEW_PROVIDERS.iter().any(|p| p.0 == id) {
                    bail!("unknown review provider: {id} (anthropic, {})", names());
                }
                let Some(model) = model else { bail!("--provider {id} requires --model <id>: search with `manga models search vision`") };
                Ok(Some(Review { provider: id.to_string(), model }))
            }
        }
    }

    pub fn check_key(&self) -> Result<()> {
        match REVIEW_PROVIDERS.iter().find(|p| p.0 == self.provider) {
            Some((_, env, url)) => require_env(env, url),
            None => bail!("unknown review provider: {}", self.provider),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(backend: BackendKind, provider: Option<&str>, model: Option<&str>, kind: Kind) -> Result<Backend> {
        Backend::parse(backend, provider.map(str::to_string), model.map(str::to_string), kind)
    }

    #[test]
    fn local_is_the_default_and_takes_no_model() {
        assert_eq!(Backend::default(), Backend::Local);
        assert_eq!(parse(BackendKind::Local, None, None, Kind::Speech).unwrap(), Backend::Local);
        let err = parse(BackendKind::Local, None, Some("a/b"), Kind::Speech).unwrap_err().to_string();
        assert!(err.contains("--backend openrouter"), "{err}");
        assert!(parse(BackendKind::Local, Some("fal"), None, Kind::Speech).is_err());
    }

    #[test]
    fn openrouter_requires_a_model() {
        assert_eq!(parse(BackendKind::OpenRouter, None, Some(" a/b "), Kind::Video).unwrap(), Backend::OpenRouter { model: "a/b".into() });
        for missing in [None, Some("   ")] {
            let err = parse(BackendKind::OpenRouter, None, missing, Kind::Video).unwrap_err().to_string();
            assert!(err.contains("requires --model"), "{err}");
        }
        assert!(parse(BackendKind::OpenRouter, Some("fal"), Some("a/b"), Kind::Video).is_err());
    }

    #[test]
    fn connect_requires_a_provider_and_model() {
        assert_eq!(
            parse(BackendKind::Connect, Some("fal"), Some("fal-ai/x"), Kind::Video).unwrap(),
            Backend::Connect { provider: "fal".into(), model: "fal-ai/x".into() }
        );
        let err = parse(BackendKind::Connect, None, Some("a"), Kind::Video).unwrap_err().to_string();
        assert!(err.contains("--provider") && err.contains("fal, google, xai, replicate"), "{err}");
        let err = parse(BackendKind::Connect, Some("fal"), None, Kind::Video).unwrap_err().to_string();
        assert!(err.contains("requires --model"), "{err}");
    }

    #[test]
    fn connect_refuses_providers_the_library_does_not_support_for_a_kind() {
        let cases = [
            (Kind::Video, "openai", "fal, google, xai, replicate"),
            (Kind::Video, "elevenlabs", "fal, google, xai, replicate"),
            (Kind::Speech, "xai", "fal, openai, google, elevenlabs"),
            (Kind::Speech, "replicate", "fal, openai, google, elevenlabs"),
            (Kind::Image, "elevenlabs", "fal, openai, google, xai, replicate"),
            (Kind::Image, "nope", "fal, openai, google, xai, replicate"),
        ];
        for (kind, provider, names) in cases {
            let err = parse(BackendKind::Connect, Some(provider), Some("m"), kind).unwrap_err().to_string();
            assert!(err.contains(provider) && err.contains(names) && !err.contains('\n'), "{err}");
        }
    }

    #[test]
    fn cli_names_are_stable() {
        assert_eq!(BackendKind::from_str("local", false).unwrap(), BackendKind::Local);
        assert_eq!(BackendKind::from_str("openrouter", false).unwrap(), BackendKind::OpenRouter);
        assert_eq!(BackendKind::from_str("connect", false).unwrap(), BackendKind::Connect);
        assert!(BackendKind::from_str("open-router", false).is_err());
    }

    #[test]
    fn cache_suffix_distinguishes_backend_provider_and_model() {
        assert_eq!(Backend::Local.cache_suffix(), "");
        let connect = |provider: &str, model: &str| Backend::Connect { provider: provider.into(), model: model.into() }.cache_suffix();
        assert_eq!(connect("fal", "m"), "|connect|fal|m");
        let a = Backend::OpenRouter { model: "a/b".into() }.cache_suffix();
        let b = Backend::OpenRouter { model: "a/c".into() }.cache_suffix();
        assert!(!a.is_empty() && a != b);
        assert_ne!(connect("fal", "m"), connect("google", "m"));
        assert_ne!(connect("fal", "m"), connect("fal", "n"));
        assert_ne!(connect("fal", "a/b"), a);
    }

    #[test]
    fn missing_or_blank_key_is_refused() {
        assert!(key_status(None).is_err());
        assert!(key_status(Some("  ")).is_err());
        assert!(key_status(Some("sk-or-test")).is_ok());
        assert!(key_status(None).unwrap_err().to_string().contains(KEY_ENV));
    }

    #[test]
    fn review_defaults_to_anthropic() {
        assert_eq!(Review::parse("api", None, None).unwrap(), None);
        assert_eq!(Review::parse("api", Some("anthropic".into()), None).unwrap(), None);
        assert!(Review::parse("api", Some("anthropic".into()), Some("m".into())).is_err());
        assert!(Review::parse("api", None, Some("m".into())).is_err());
    }

    #[test]
    fn review_through_a_provider_needs_a_model() {
        assert_eq!(Review::parse("api", Some("openrouter".into()), Some(" a/b ".into())).unwrap(), Some(Review { provider: "openrouter".into(), model: "a/b".into() }));
        let err = Review::parse("api", Some("openrouter".into()), None).unwrap_err().to_string();
        assert!(err.contains("requires --model"), "{err}");
        let err = Review::parse("api", Some("nope".into()), Some("m".into())).unwrap_err().to_string();
        assert!(err.contains("anthropic, openrouter, openai, google"), "{err}");
    }

    #[test]
    fn review_flags_only_apply_to_the_api_detector() {
        assert_eq!(Review::parse("magi", None, None).unwrap(), None);
        assert!(Review::parse("vision", Some("openrouter".into()), None).is_err());
        assert!(Review::parse("magi", None, Some("m".into())).is_err());
    }
}
