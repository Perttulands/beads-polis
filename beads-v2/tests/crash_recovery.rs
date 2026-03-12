//! Crash recovery simulation tests.
//!
//! Simulates a crash mid-write by appending a truncated JSON line
//! and verifying that the system correctly detects and discards it.

use beads_v2::bead::Bead;
use beads_v2::event::{BeadSnapshot, Event};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use tempfile::TempDir;

const NUM_VALID_EVENTS: usize = 10;

/// Helper: build a BeadSnapshot.
fn make_snapshot(id: &str) -> BeadSnapshot {
    let now = Utc::now();
    BeadSnapshot {
        id: id.to_string(),
        title: format!("Bead {}", id),
        description: None,
        status: "open".to_string(),
        priority: 2,
        bead_type: "task".to_string(),
        project: "crash".to_string(),
        assignee: None,
        parent: None,
        dependencies: vec![],
        labels: vec![],
        created_at: now,
        updated_at: now,
        closed_at: None,
        close_reason: None,
        claimed_at: None,
        claim_deadline: None,
        last_heartbeat: None,
    }
}

/// Write N valid events, then append a truncated line.
/// Verify that reading back produces exactly N events.
#[test]
fn truncated_last_line_is_discarded_on_read() {
    let dir = TempDir::new().unwrap();
    let jsonl_path = dir.path().join("events.jsonl");

    // Write N valid Create events
    {
        let mut file = fs::File::create(&jsonl_path).unwrap();
        for i in 0..NUM_VALID_EVENTS {
            let event = Event::Create {
                ts: Utc::now(),
                actor: "test".to_string(),
                bead: make_snapshot(&format!("crash-{:03}", i)),
            };
            let line = serde_json::to_string(&event).unwrap();
            writeln!(file, "{}", line).unwrap();
        }
        file.sync_all().unwrap();
    }

    // Append a truncated line (simulating crash mid-write)
    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&jsonl_path)
            .unwrap();
        // Write partial JSON — missing closing brace
        write!(
            file,
            r#"{{"op":"create","ts":"2026-03-12T18:00:00Z","actor":"crash"#
        )
        .unwrap();
        // No newline — simulates a crash before the line was complete
        file.sync_all().unwrap();
    }

    // Read back and parse, discarding invalid last line
    let file = fs::File::open(&jsonl_path).unwrap();
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut discarded = Vec::new();

    for (i, line) in reader.lines().enumerate() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(&line) {
            Ok(ev) => events.push(ev),
            Err(e) => {
                eprintln!(
                    "WARNING: discarding truncated line {} ({}): {}",
                    i + 1,
                    e,
                    &line[..line.len().min(60)]
                );
                discarded.push(i);
            }
        }
    }

    assert_eq!(
        events.len(),
        NUM_VALID_EVENTS,
        "Should have exactly {} valid events",
        NUM_VALID_EVENTS
    );
    assert_eq!(
        discarded.len(),
        1,
        "Should have discarded exactly 1 truncated line"
    );

    // Verify all valid events produce correct beads
    let mut beads: HashMap<String, Bead> = HashMap::new();
    for event in &events {
        let id = event.id().to_string();
        match event {
            Event::Create { bead: snap, .. } => {
                beads.insert(id, Bead::from_snapshot(snap));
            }
            _ => {
                if let Some(bead) = beads.get_mut(&id) {
                    bead.apply_event(event);
                }
            }
        }
    }
    assert_eq!(beads.len(), NUM_VALID_EVENTS);
}

/// After discarding a truncated line, subsequent appends work correctly.
#[test]
fn append_after_truncation_recovery() {
    let dir = TempDir::new().unwrap();
    let jsonl_path = dir.path().join("events.jsonl");

    // Write 3 valid events
    {
        let mut file = fs::File::create(&jsonl_path).unwrap();
        for i in 0..3 {
            let event = Event::Create {
                ts: Utc::now(),
                actor: "test".to_string(),
                bead: make_snapshot(&format!("rec-{:03}", i)),
            };
            writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
        }
        // Append truncated line
        write!(file, r#"{{"op":"update","ts":"2026-03-12"#).unwrap();
        file.sync_all().unwrap();
    }

    // "Recovery": read valid events, rewrite file without truncated line,
    // then append a new event.
    let valid_lines: Vec<String> = {
        let file = fs::File::open(&jsonl_path).unwrap();
        BufReader::new(file)
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.trim().is_empty() && serde_json::from_str::<Event>(l).is_ok())
            .collect()
    };

    assert_eq!(valid_lines.len(), 3);

    // Simulate what EventLog should do: truncate the file to remove invalid data
    // and then append a new valid event
    {
        let mut file = fs::File::create(&jsonl_path).unwrap();
        for line in &valid_lines {
            writeln!(file, "{}", line).unwrap();
        }
        // Append new event after recovery
        let new_event = Event::Close {
            ts: Utc::now(),
            actor: "test".to_string(),
            id: "rec-000".to_string(),
            reason: "recovered and closed".to_string(),
        };
        writeln!(file, "{}", serde_json::to_string(&new_event).unwrap()).unwrap();
        file.sync_all().unwrap();
    }

    // Verify: 4 valid lines total
    let final_lines: Vec<Event> = {
        let file = fs::File::open(&jsonl_path).unwrap();
        BufReader::new(file)
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(&l).unwrap())
            .collect()
    };

    assert_eq!(final_lines.len(), 4);

    // Replay and verify state
    let mut beads: HashMap<String, Bead> = HashMap::new();
    for event in &final_lines {
        let id = event.id().to_string();
        match event {
            Event::Create { bead: snap, .. } => {
                beads.insert(id, Bead::from_snapshot(snap));
            }
            _ => {
                if let Some(bead) = beads.get_mut(&id) {
                    bead.apply_event(event);
                }
            }
        }
    }
    assert_eq!(
        beads["rec-000"].status,
        beads_v2::bead::Status::Closed,
        "rec-000 should be closed after recovery"
    );
}

/// Full crash recovery test using EventLog.
/// Ignored until EventLog is implemented.
#[test]
#[ignore]
fn eventlog_crash_recovery() {
    // TODO: When EventLog is implemented:
    // 1. Write N events via EventLog
    // 2. Manually append truncated line to the JSONL file
    // 3. Open EventLog again
    // 4. Verify read_all() returns exactly N events
    // 5. Verify a warning was logged about the discarded line
    // 6. Append another event and verify it works
    // 7. Verify index rebuild produces correct state
    panic!("EventLog not yet implemented");
}
