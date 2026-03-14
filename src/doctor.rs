//! Doctor — health check and self-healing.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::bead::{Bead, Status};
use crate::event::Event;
use crate::observe::{ObserveEvent, ObserveLog};

#[derive(Debug, Default, Serialize)]
pub struct Diagnosis {
    pub jsonl_lines: usize,
    pub jsonl_valid_lines: usize,
    pub jsonl_invalid_lines: usize,
    pub truncated_last_line: bool,
    pub index_watermark: Option<usize>,
    pub index_watermark_stale: bool,
    #[serde(default)]
    pub sqlite_integrity: String,
    pub stale_claims: Vec<StaleClaim>,
    pub last_rebuild_ts: Option<String>,
    pub snapshot_count: usize,
    pub oldest_snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StaleClaim {
    pub bead_id: String,
    pub assignee: String,
    pub deadline: DateTime<Utc>,
}

/// Run diagnostics on the beads store.
pub fn diagnose(log_path: &Path, db_path: &Path) -> Diagnosis {
    let mut diag = Diagnosis { sqlite_integrity: "no_db".into(), ..Default::default() };

    // JSONL line count and validity
    let mut last_line_bad = false;
    let mut beads: HashMap<String, Bead> = HashMap::new();
    if let Ok(file) = File::open(log_path) {
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            diag.jsonl_lines += 1;
            last_line_bad = false;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Event>(&line) {
                Ok(event) => {
                    diag.jsonl_valid_lines += 1;
                    let id = event.id().to_string();
                    if let Some(bead) = beads.get_mut(&id) {
                        bead.apply_event(&event);
                    } else if let Some(bead) = Bead::from_events(&[event]) {
                        beads.insert(id, bead);
                    }
                }
                Err(_) => {
                    diag.jsonl_invalid_lines += 1;
                    last_line_bad = true;
                }
            }
        }
    }
    diag.truncated_last_line = last_line_bad;

    // Index watermark vs JSONL line count
    let wm_path = db_path.with_file_name("index.watermark");
    if let Ok(data) = fs::read_to_string(&wm_path) {
        if let Ok(wm) = data.trim().parse::<usize>() {
            diag.index_watermark = Some(wm);
            diag.index_watermark_stale = wm < diag.jsonl_lines;
        }
    }

    // SQLite integrity check
    if db_path.exists() {
        diag.sqlite_integrity = match rusqlite::Connection::open(db_path) {
            Ok(conn) => conn
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap_or_else(|e| format!("query_failed: {e}")),
            Err(e) => format!("open_failed: {e}"),
        };
    }

    // Stale claims
    let now = Utc::now();
    for bead in beads.values() {
        if bead.status != Status::InProgress {
            continue;
        }
        if let Some(deadline) = bead.claim_deadline {
            if deadline < now {
                // Check heartbeat — if last_heartbeat is also stale, it's truly stale
                let heartbeat_stale = bead
                    .last_heartbeat
                    .is_none_or(|hb| hb < deadline);
                if heartbeat_stale {
                    diag.stale_claims.push(StaleClaim {
                        bead_id: bead.id.clone(),
                        assignee: bead.assignee.clone().unwrap_or_default(),
                        deadline,
                    });
                }
            }
        }
    }

    // Snapshot count and age
    let snap_dir = log_path.parent().unwrap_or(Path::new(".")).join("snapshots");
    if let Ok(entries) = fs::read_dir(&snap_dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        diag.snapshot_count = names.len();
        diag.oldest_snapshot = names.first().cloned();
    }

    diag
}

/// Apply automatic fixes based on diagnosis.
/// Acquires flock before any JSONL modifications to prevent data-loss races.
pub fn auto_fix(diagnosis: &Diagnosis, log_path: &Path, db_path: &Path) {
    let logger = ObserveLog::default_location();
    let lock_path = log_path.with_extension("jsonl.lock");

    // Fix 1: Truncated last line — discard it (requires flock)
    if diagnosis.truncated_last_line {
        if let Ok(lock_file) = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
        {
            if fs2::FileExt::lock_exclusive(&lock_file).is_ok() {
                if let Ok(data) = fs::read_to_string(log_path) {
                    let mut lines: Vec<&str> = data.lines().collect();
                    if let Some(last) = lines.last() {
                        if serde_json::from_str::<serde_json::Value>(last).is_err() {
                            lines.pop();
                            if let Ok(mut f) = File::create(log_path) {
                                for line in &lines {
                                    let _ = writeln!(f, "{line}");
                                }
                                let _ = f.sync_all();
                            }
                            logger.emit(&ObserveEvent::TruncatedLineDiscarded {
                                ts: Utc::now(),
                                message: "Discarded truncated last line from JSONL".into(),
                            });
                        }
                    }
                }
                let _ = fs2::FileExt::unlock(&lock_file);
            }
        }
    }

    // Fix 2: Stale or corrupt index — delete to trigger rebuild on next read
    if diagnosis.index_watermark_stale || diagnosis.sqlite_integrity != "ok" {
        if db_path.exists() {
            let _ = fs::remove_file(db_path);
        }
        let wm_path = db_path.with_file_name("index.watermark");
        if wm_path.exists() {
            let _ = fs::remove_file(&wm_path);
        }
        logger.emit(&ObserveEvent::IndexRebuild {
            ts: Utc::now(),
            message: format!(
                "Deleted stale index (watermark_stale={}, integrity={})",
                diagnosis.index_watermark_stale, diagnosis.sqlite_integrity
            ),
        });
    }

    // Fix 3: Stale claims — write typed unclaim events (requires flock)
    if !diagnosis.stale_claims.is_empty() {
        if let Ok(lock_file) = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
        {
            if fs2::FileExt::lock_exclusive(&lock_file).is_ok() {
                if let Ok(mut f) = fs::OpenOptions::new().append(true).open(log_path) {
                    let now = Utc::now();
                    for claim in &diagnosis.stale_claims {
                        let mut fields = std::collections::HashMap::new();
                        fields.insert("status".to_string(), serde_json::json!("open"));
                        fields.insert("assignee".to_string(), serde_json::Value::Null);
                        fields.insert("claimed_at".to_string(), serde_json::Value::Null);
                        fields.insert("claim_deadline".to_string(), serde_json::Value::Null);
                        fields.insert("last_heartbeat".to_string(), serde_json::Value::Null);

                        let event = Event::Update {
                            ts: now,
                            actor: "doctor".to_string(),
                            id: claim.bead_id.clone(),
                            fields,
                        };
                        if let Ok(line) = serde_json::to_string(&event) {
                            let _ = writeln!(f, "{line}");
                        }
                        logger.emit(&ObserveEvent::StaleClaimReleased {
                            ts: now,
                            bead_id: claim.bead_id.clone(),
                            previous_holder: claim.assignee.clone(),
                        });
                    }
                    let _ = f.sync_all();
                }
                let _ = fs2::FileExt::unlock(&lock_file);
            }
        }
    }
}
