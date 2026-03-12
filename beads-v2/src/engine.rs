//! Engine — shared context for all command handlers.

use crate::config::Config;
use crate::index::Index;
use crate::log::EventLog;
use std::io;
use std::path::{Path, PathBuf};

pub struct Engine {
    pub log: EventLog,
    pub index: Index,
    pub config: Config,
    pub beads_dir: PathBuf,
}

impl Engine {
    /// Open the engine at the given beads directory.
    /// Creates the directory and files if needed. Rebuilds index if stale.
    pub fn open(beads_dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(beads_dir)?;

        let log_path = beads_dir.join("events.jsonl");
        let db_path = beads_dir.join("index.db");
        let config_path = beads_dir.join("config.yaml");

        let log = EventLog::open(&log_path)?;
        let index = Index::open_or_rebuild(&db_path, &log)?;
        let config = Config::load(&config_path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        Ok(Self {
            log,
            index,
            config,
            beads_dir: beads_dir.to_path_buf(),
        })
    }

    /// Generate a unique bead ID: {prefix}-{4 random hex chars}.
    /// Retries if the generated ID already exists in the index.
    pub fn generate_id(&self) -> String {
        let prefix = &self.config.issue_prefix;
        for _ in 0..100 {
            let bytes = random_bytes();
            let id = format!("{}-{:02x}{:02x}", prefix, bytes[0], bytes[1]);
            if self.index.query_show(&id).is_none() {
                return id;
            }
        }
        // Fallback: use more bytes for uniqueness
        let b1 = random_bytes();
        let b2 = random_bytes();
        format!("{}-{:02x}{:02x}{:02x}{:02x}", prefix, b1[0], b1[1], b2[0], b2[1])
    }

    /// Path to the snapshot directory.
    pub fn snapshot_dir(&self) -> PathBuf {
        self.beads_dir.join("snapshots")
    }
}

/// Read 2 random bytes from /dev/urandom (no extra dependency).
fn random_bytes() -> [u8; 2] {
    use std::fs::File;
    use std::io::Read;
    let mut buf = [0u8; 2];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    } else {
        // Fallback: time-based
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        buf[0] = (t & 0xFF) as u8;
        buf[1] = ((t >> 8) & 0xFF) as u8;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn engine_opens_fresh_dir() {
        let tmp = TempDir::new().unwrap();
        let beads_dir = tmp.path().join(".beads");
        let engine = Engine::open(&beads_dir).unwrap();
        assert!(beads_dir.join("events.jsonl").exists());
        assert_eq!(engine.config.issue_prefix, "pol");
    }

    #[test]
    fn engine_generates_valid_ids() {
        let tmp = TempDir::new().unwrap();
        let beads_dir = tmp.path().join(".beads");
        let engine = Engine::open(&beads_dir).unwrap();
        let id = engine.generate_id();
        assert!(id.starts_with("pol-"));
        assert_eq!(id.len(), 8); // "pol-" + 4 hex chars
    }

    #[test]
    fn engine_opens_existing_with_events() {
        let tmp = TempDir::new().unwrap();
        let beads_dir = tmp.path().join(".beads");
        std::fs::create_dir_all(&beads_dir).unwrap();

        // Write a create event
        let event_json = r#"{"op":"create","ts":"2026-03-12T10:00:00Z","actor":"test","bead":{"id":"pol-0001","title":"Test bead","status":"open","priority":2,"type":"task","project":"test","dependencies":[],"labels":[],"created_at":"2026-03-12T10:00:00Z","updated_at":"2026-03-12T10:00:00Z"}}"#;
        std::fs::write(beads_dir.join("events.jsonl"), format!("{}\n", event_json)).unwrap();

        let engine = Engine::open(&beads_dir).unwrap();
        let bead = engine.index.query_show("pol-0001");
        assert!(bead.is_some());
        assert_eq!(bead.unwrap().title, "Test bead");
    }
}
