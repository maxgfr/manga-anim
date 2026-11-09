//! The optional Node worker behind `--backend connect`: `workers/connect/run.mjs` calls
//! llm-connect. Python workers drive it one item at a time, and still do the file conversions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::home::{connect_dir, is_installed, workers_dir};

const FILES: [&str; 3] = ["run.mjs", "package.json", "package-lock.json"];
const MIN_NODE: (u32, u32) = (22, 12);
const STAMP: &str = "node_modules/.manga-lock-sha256";

/// The job fields telling a Python worker to go through llm-connect. Never holds an API key.
#[derive(Serialize)]
pub struct Fields<'a> {
    pub backend: &'static str,
    pub provider: &'a str,
    pub model: &'a str,
    pub script: String,
}

impl<'a> Fields<'a> {
    pub fn new(provider: &'a str, model: &'a str, script: &Path) -> Self {
        Self { backend: "connect", provider, model, script: script.to_string_lossy().into() }
    }
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `node --version`, or `None` when Node is not on the PATH.
pub fn node_version() -> Option<String> {
    let out = Command::new("node").arg("--version").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether the installed Node can run the worker.
pub fn node_is_recent(version: &str) -> bool {
    parse_version(version).is_some_and(|v| v >= MIN_NODE)
}

fn lock_digest(dir: &Path) -> Result<String> {
    Ok(hex::encode(Sha256::digest(fs::read(dir.join("package-lock.json"))?)))
}

/// Whether the locked dependencies of the worker are in place.
pub fn dependencies_ready() -> bool {
    let dir = connect_dir();
    let source = workers_dir().join("connect");
    let Ok(wanted) = lock_digest(if is_installed() { &source } else { &dir }) else { return false };
    fs::read_to_string(dir.join(STAMP)).is_ok_and(|s| s.trim() == wanted)
}

/// Copies the worker next to its `node_modules` when the code folder is an install prefix,
/// which is replaced on every update and is not ours to write to.
fn sync_files(from: &Path, to: &Path) -> Result<()> {
    if from == to {
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for name in FILES {
        let bytes = fs::read(from.join(name)).with_context(|| format!("{name} is missing from {}", from.display()))?;
        if fs::read(to.join(name)).ok().as_deref() != Some(&bytes[..]) {
            fs::write(to.join(name), bytes)?;
        }
    }
    Ok(())
}

/// Checks Node, installs the locked dependencies if needed and returns the path of `run.mjs`.
pub fn ensure_worker() -> Result<PathBuf> {
    let min = format!("{}.{}", MIN_NODE.0, MIN_NODE.1);
    let Some(version) = node_version() else {
        bail!("Node.js not found: --backend connect needs Node {min} or later (brew install node)");
    };
    if !node_is_recent(&version) {
        bail!("Node.js {version} is too old: --backend connect needs Node {min} or later");
    }
    let dir = connect_dir();
    sync_files(&workers_dir().join("connect"), &dir)?;
    let wanted = lock_digest(&dir)?;
    let stamp = dir.join(STAMP);
    if fs::read_to_string(&stamp).map_or(true, |s| s.trim() != wanted) {
        eprintln!("installing the llm-connect worker (npm ci)");
        // Scripts stay off: nothing in these packages needs to run at install time.
        let status = Command::new("npm")
            .args(["ci", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund", "--loglevel=error"])
            .current_dir(&dir)
            .status()
            .context("npm not found: install Node.js (brew install node)")?;
        if !status.success() {
            bail!("npm ci failed in {} ({status})", dir.display());
        }
        fs::write(&stamp, &wanted)?;
    }
    Ok(dir.join("run.mjs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_versions() {
        assert_eq!(parse_version("v24.22.2\n"), Some((24, 22)));
        assert!(node_is_recent("v24.22.2"));
        assert!(node_is_recent("v22.12.0"));
        assert!(!node_is_recent("v22.11.9"));
        assert!(!node_is_recent("v18.20.0"));
        assert!(!node_is_recent("garbage"));
    }

    #[test]
    fn job_fields_hold_no_key() {
        let fields = Fields::new("fal", "fal-ai/x", Path::new("/w/run.mjs"));
        let text = serde_json::to_string(&fields).unwrap();
        assert_eq!(text, r#"{"backend":"connect","provider":"fal","model":"fal-ai/x","script":"/w/run.mjs"}"#);
    }

    #[test]
    fn files_are_copied_once_when_installed() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b"));
        fs::create_dir_all(&from).unwrap();
        for name in FILES {
            fs::write(from.join(name), name).unwrap();
        }
        sync_files(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("run.mjs")).unwrap(), "run.mjs");
        fs::write(from.join("run.mjs"), "new").unwrap();
        sync_files(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("run.mjs")).unwrap(), "new");
    }

    #[test]
    fn sync_is_a_no_op_in_the_repo() {
        sync_files(Path::new("/nonexistent"), Path::new("/nonexistent")).unwrap();
    }

    /// Installs the locked packages from npm: free, but it needs the network and Node.
    #[test]
    #[ignore]
    fn worker_installs_from_the_lockfile() {
        let script = ensure_worker().unwrap();
        assert!(script.ends_with("run.mjs") && script.exists());
        assert!(dependencies_ready());
    }
}
