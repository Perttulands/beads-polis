//! Compaction — collapse event history into snapshots.

use chrono::Utc;
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::bead::Bead;
use crate::event::{BeadSnapshot, Event};
use crate::observe::{ObserveEvent, ObserveLog};

/// Returns true if events.jsonl exceeds compaction thresholds (>10k lines or >5MB).
pub fn should_compact(log_path: &Path) -> bool {
    let size = fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    if size > 5_000_000 {
        return true;
    }
    let count = BufReader::new(match File::open(log_path) {
        Ok(f) => f,
        Err(_) => return false,
    })
    .lines()
    .count();
    count > 10_000
}

/// Compact the event log: replay all events, write one snapshot per bead, archive old log.
pub fn compact(log_path: &Path, snapshot_dir: &Path) -> Result<usize, String> {
    let lock_path = log_path.with_extension("jsonl.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("open lock: {e}"))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| format!("flock: {e}"))?;

    let result = compact_inner(log_path, snapshot_dir);

    let _ = lock_file.unlock();
    result
}

fn compact_inner(log_path: &Path, snapshot_dir: &Path) -> Result<usize, String> {
    // Replay all events to compute current state
    let file = File::open(log_path).map_err(|e| format!("open log: {e}"))?;
    let reader = BufReader::new(file);
    let mut beads: HashMap<String, Bead> = HashMap::new();
    let mut old_line_count = 0usize;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("read line: {e}"))?;
        old_line_count += 1;
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue, // skip unparseable lines
        };
        let id = event.id().to_string();
        if let Some(bead) = beads.get_mut(&id) {
            bead.apply_event(&event);
        } else if let Some(bead) = Bead::from_events(&[event]) {
            beads.insert(id, bead);
        }
    }

    // Write snapshot events to new file
    let new_path = log_path.with_extension("jsonl.new");
    let mut out = File::create(&new_path).map_err(|e| format!("create new log: {e}"))?;
    let now = Utc::now();
    let bead_count = beads.len();

    for bead in beads.values() {
        let snap_event = Event::Snapshot {
            ts: now,
            actor: "compaction".into(),
            bead: bead_to_snapshot(bead),
        };
        let line = serde_json::to_string(&snap_event).map_err(|e| format!("serialize: {e}"))?;
        writeln!(out, "{line}").map_err(|e| format!("write: {e}"))?;
    }
    out.flush().map_err(|e| format!("flush: {e}"))?;
    out.sync_all().map_err(|e| format!("fsync: {e}"))?;

    // Archive old log to snapshots/
    fs::create_dir_all(snapshot_dir).map_err(|e| format!("create snapshot dir: {e}"))?;
    let date_suffix = now.format("%Y-%m-%d-%H%M%S");
    let archive_name = snapshot_dir.join(format!("events-{date_suffix}.jsonl"));
    fs::rename(log_path, &archive_name).map_err(|e| format!("archive old log: {e}"))?;

    // Atomic rename new → current
    fs::rename(&new_path, log_path).map_err(|e| format!("rename new log: {e}"))?;

    // Emit observability event
    ObserveLog::default_location().emit(&ObserveEvent::CompactionRun {
        ts: now,
        beads: bead_count,
        old_lines: old_line_count,
    });

    Ok(bead_count)
}

fn bead_to_snapshot(b: &Bead) -> BeadSnapshot {
    BeadSnapshot {
        id: b.id.clone(),
        title: b.title.clone(),
        description: b.description.clone(),
        status: b.status.as_str().to_string(),
        priority: b.priority,
        bead_type: b.bead_type.as_str().to_string(),
        project: b.project.clone(),
        assignee: b.assignee.clone(),
        parent: b.parent.clone(),
        dependencies: b.dependencies.clone(),
        labels: b.labels.clone(),
        created_at: b.created_at,
        updated_at: b.updated_at,
        closed_at: b.closed_at,
        close_reason: b.close_reason.clone(),
        claimed_at: b.claimed_at,
        claim_deadline: b.claim_deadline,
        last_heartbeat: b.last_heartbeat,
    }
}

/// Keep the last `keep` snapshots in the directory, delete older ones.
pub fn cleanup_snapshots(snapshot_dir: &Path, keep: usize) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(snapshot_dir)
        .map_err(|e| format!("read snapshot dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "jsonl")
        })
        .collect();
    // Sort by name (date suffix ensures chronological order)
    entries.sort_by_key(|e| e.file_name());
    if entries.len() > keep {
        let to_remove = entries.len() - keep;
        for entry in &entries[..to_remove] {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}
