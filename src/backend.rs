//! Where a generation step runs: local models (the default) or OpenRouter.

use anyhow::{Result, bail};
use clap::ValueEnum;

pub const KEY_ENV: &str = "OPENROUTER_API_KEY";

/// The value of `--backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    Local,
    #[value(name = "openrouter")]
    OpenRouter,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Local,
    OpenRouter {
        model: String,
    },
}

impl Backend {
    /// Combines `--backend` and `--model`. No model is ever assumed for OpenRouter: the choice
    /// sets what the user pays, and model ids change over time.
    pub fn parse(kind: BackendKind, model: Option<String>) -> Result<Self> {
        let model = model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
        match (kind, model) {
            (BackendKind::Local, None) => Ok(Backend::Local),
            (BackendKind::Local, Some(_)) => bail!("--model only applies to --backend openrouter"),
            (BackendKind::OpenRouter, Some(model)) => Ok(Backend::OpenRouter { model }),
            (BackendKind::OpenRouter, None) => bail!("--backend openrouter requires --model <id>: pick one at https://openrouter.ai/models"),
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
        }
    }

    /// Stops before any work when the key is missing. The key's value is never read further.
    pub fn check_key(&self) -> Result<()> {
        if self.is_local() { Ok(()) } else { key_status(std::env::var(KEY_ENV).ok().as_deref()) }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_is_the_default_and_takes_no_model() {
        assert_eq!(Backend::default(), Backend::Local);
        assert_eq!(Backend::parse(BackendKind::Local, None).unwrap(), Backend::Local);
        let err = Backend::parse(BackendKind::Local, Some("a/b".into())).unwrap_err().to_string();
        assert!(err.contains("--backend openrouter"), "{err}");
    }

    #[test]
    fn openrouter_requires_a_model() {
        assert_eq!(Backend::parse(BackendKind::OpenRouter, Some(" a/b ".into())).unwrap(), Backend::OpenRouter { model: "a/b".into() });
        for missing in [None, Some("   ".to_string())] {
            let err = Backend::parse(BackendKind::OpenRouter, missing).unwrap_err().to_string();
            assert!(err.contains("requires --model"), "{err}");
        }
    }

    #[test]
    fn cli_names_are_stable() {
        assert_eq!(BackendKind::from_str("local", false).unwrap(), BackendKind::Local);
        assert_eq!(BackendKind::from_str("openrouter", false).unwrap(), BackendKind::OpenRouter);
        assert!(BackendKind::from_str("open-router", false).is_err());
    }

    #[test]
    fn cache_suffix_distinguishes_backends_and_models() {
        assert_eq!(Backend::Local.cache_suffix(), "");
        let a = Backend::OpenRouter { model: "a/b".into() }.cache_suffix();
        let b = Backend::OpenRouter { model: "a/c".into() }.cache_suffix();
        assert!(!a.is_empty() && a != b);
    }

    #[test]
    fn missing_or_blank_key_is_refused() {
        assert!(key_status(None).is_err());
        assert!(key_status(Some("  ")).is_err());
        assert!(key_status(Some("sk-or-test")).is_ok());
        assert!(key_status(None).unwrap_err().to_string().contains(KEY_ENV));
    }
}
