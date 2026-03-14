//! Configuration — read config.yaml and resolve paths.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_prefix")]
    pub issue_prefix: String,
    #[serde(default)]
    pub projects: HashMap<String, PathBuf>,
}

fn default_prefix() -> String {
    "pol".into()
}

impl Config {
    /// Load config from a YAML file. Returns defaults if missing.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self {
                issue_prefix: default_prefix(),
                projects: HashMap::new(),
            });
        }
        let data = std::fs::read_to_string(path).map_err(|e| format!("read config: {e}"))?;
        serde_yaml::from_str(&data).map_err(|e| format!("parse config.yaml: {e}"))
    }
}

/// Read POLIS_ACTOR env var. Fails if unset — agents cannot self-report identity.
pub fn resolve_actor() -> Result<String, String> {
    std::env::var("POLIS_ACTOR")
        .map_err(|_| "POLIS_ACTOR not set — set it in your environment".into())
}

/// Find the beads directory: BEADS_DIR env, then walk up from cwd for .beads/, then default.
pub fn resolve_beads_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BEADS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = cwd.as_path();
        loop {
            let candidate = dir.join(".beads");
            if candidate.is_dir() {
                return candidate;
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
    }
    // Default canonical location
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/polis".into());
    PathBuf::from(home).join(".polis/beads")
}
