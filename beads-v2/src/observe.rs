//! Observability — structured logging and thrash detection.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Self-healing events emitted by the maintenance subsystem.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ObserveEvent {
    IndexRebuild { ts: DateTime<Utc>, message: String },
    TruncatedLineDiscarded { ts: DateTime<Utc>, message: String },
    StaleClaimReleased { ts: DateTime<Utc>, bead_id: String, previous_holder: String },
    CompactionRun { ts: DateTime<Utc>, beads: usize, old_lines: usize },
    IntegrityCheckFailed { ts: DateTime<Utc>, message: String },
}

/// Simple file + stderr logger for self-healing events.
pub struct ObserveLog {
    log_path: PathBuf,
}

impl ObserveLog {
    pub fn new(log_path: PathBuf) -> Self {
        Self { log_path }
    }

    /// Default log location: ~/.polis/beads/beads.log
    pub fn default_location() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/polis".into());
        Self::new(PathBuf::from(home).join(".polis/beads/beads.log"))
    }

    /// Emit an event to stderr and append to the log file.
    pub fn emit(&self, event: &ObserveEvent) {
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(_) => return,
        };
        eprintln!("[beads] {line}");
        self.append_to_file(&line);
    }

    fn append_to_file(&self, line: &str) {
        if let Some(parent) = self.log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Rotate log if it exceeds max_bytes. Keeps one backup.
    pub fn rotate_if_needed(&self, max_bytes: u64) {
        let size = fs::metadata(&self.log_path).map(|m| m.len()).unwrap_or(0);
        if size > max_bytes {
            let backup = self.log_path.with_extension("log.1");
            let _ = fs::rename(&self.log_path, backup);
        }
    }
}

/// Detects index rebuild thrashing: >5 rebuilds in 60 seconds.
pub struct ThrashDetector {
    timestamps: VecDeque<DateTime<Utc>>,
    threshold: usize,
    window_secs: i64,
}

impl Default for ThrashDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ThrashDetector {
    pub fn new() -> Self {
        Self {
            timestamps: VecDeque::new(),
            threshold: 5,
            window_secs: 60,
        }
    }

    /// Record a rebuild. Returns true if thrashing detected.
    pub fn record_rebuild(&mut self) -> bool {
        let now = Utc::now();
        self.timestamps.push_back(now);
        self.prune(now);
        self.timestamps.len() > self.threshold
    }

    /// If thrashing, emit a JSON alert to stdout.
    pub fn check_and_alert(&mut self) {
        if self.record_rebuild() {
            let alert = serde_json::json!({
                "alert": "index_thrashing",
                "rebuilds": self.timestamps.len(),
                "window": "60s"
            });
            println!("{alert}");
        }
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::seconds(self.window_secs);
        while self.timestamps.front().is_some_and(|t| *t < cutoff) {
            self.timestamps.pop_front();
        }
    }

    /// Load timestamps from a state file (one ISO timestamp per line).
    pub fn load(path: &Path) -> Self {
        let mut det = Self::new();
        if let Ok(data) = fs::read_to_string(path) {
            for line in data.lines() {
                if let Ok(ts) = line.parse::<DateTime<Utc>>() {
                    det.timestamps.push_back(ts);
                }
            }
            det.prune(Utc::now());
        }
        det
    }

    /// Persist timestamps to a state file.
    pub fn save(&self, path: &Path) {
        let lines: Vec<String> = self.timestamps.iter().map(|t| t.to_rfc3339()).collect();
        let _ = fs::write(path, lines.join("\n"));
    }
}
