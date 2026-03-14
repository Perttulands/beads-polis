//! Concurrency stress test.
//!
//! Spawns 20 threads, each doing 50 iterations of create/update/close,
//! all writing to the same JSONL file. Verifies that every line is valid
//! JSON and that replaying all events produces consistent state.
//!
//! Ignored until EventLog is implemented.

use beads_polis::bead::Bead;
use beads_polis::event::{BeadSnapshot, Event};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

const NUM_THREADS: usize = 20;
const ITERATIONS: usize = 50;

/// Helper: build a BeadSnapshot with given id.
fn make_snapshot(id: &str) -> BeadSnapshot {
    let now = Utc::now();
    BeadSnapshot {
        id: id.to_string(),
        title: format!("Bead {}", id),
        description: None,
        status: "open".to_string(),
        priority: 2,
        bead_type: "task".to_string(),
        project: "stress".to_string(),
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

/// Direct-to-file concurrent write test using flock for serialization.
/// This tests the concurrency invariant without needing EventLog.
#[test]
fn concurrent_jsonl_writes_produce_valid_json_lines() {
    use fs2::FileExt;

    let dir = TempDir::new().unwrap();
    let jsonl_path = dir.path().join("events.jsonl");
    let lock_path = dir.path().join("events.jsonl.lock");

    // Create the files
    fs::File::create(&jsonl_path).unwrap();
    fs::File::create(&lock_path).unwrap();

    let jsonl_path = Arc::new(jsonl_path);
    let lock_path = Arc::new(lock_path);
    let barrier = Arc::new(Barrier::new(NUM_THREADS));

    let handles: Vec<_> = (0..NUM_THREADS)
        .map(|thread_id| {
            let jsonl_path = Arc::clone(&jsonl_path);
            let lock_path = Arc::clone(&lock_path);
            let barrier = Arc::clone(&barrier);

            std::thread::spawn(move || {
                barrier.wait(); // synchronize start

                for iter in 0..ITERATIONS {
                    let bead_id = format!("t{}-i{}", thread_id, iter);

                    // Three events per iteration: create, update, close
                    let events = vec![
                        Event::Create {
                            ts: Utc::now(),
                            actor: format!("thread-{}", thread_id),
                            bead: make_snapshot(&bead_id),
                        },
                        Event::Update {
                            ts: Utc::now(),
                            actor: format!("thread-{}", thread_id),
                            id: bead_id.clone(),
                            fields: {
                                let mut m = HashMap::new();
                                m.insert(
                                    "status".to_string(),
                                    serde_json::json!("in_progress"),
                                );
                                m
                            },
                        },
                        Event::Close {
                            ts: Utc::now(),
                            actor: format!("thread-{}", thread_id),
                            id: bead_id,
                            reason: "done".to_string(),
                        },
                    ];

                    // Acquire flock, append, fsync, release
                    let lock_file = fs::File::open(lock_path.as_ref()).unwrap();
                    lock_file.lock_exclusive().unwrap();

                    let mut file = fs::OpenOptions::new()
                        .append(true)
                        .open(jsonl_path.as_ref())
                        .unwrap();

                    for event in &events {
                        let line = serde_json::to_string(event).unwrap();
                        writeln!(file, "{}", line).unwrap();
                    }
                    file.sync_all().unwrap();

                    lock_file.unlock().unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Verify: every line is valid JSON
    let file = fs::File::open(jsonl_path.as_ref()).unwrap();
    let reader = BufReader::new(file);
    let mut line_count = 0;
    let mut events: Vec<Event> = Vec::new();

    for (i, line) in reader.lines().enumerate() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("Invalid JSON on line {}: {}\nContent: {}", i + 1, e, line));
        events.push(event);
        line_count += 1;
    }

    let expected_lines = NUM_THREADS * ITERATIONS * 3; // 3 events per iteration
    assert_eq!(
        line_count, expected_lines,
        "Expected {} lines, got {}",
        expected_lines, line_count
    );

    // Verify: replay produces consistent state (all beads end up closed)
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

    assert_eq!(
        beads.len(),
        NUM_THREADS * ITERATIONS,
        "Expected {} beads, got {}",
        NUM_THREADS * ITERATIONS,
        beads.len()
    );

    for (id, bead) in &beads {
        assert_eq!(
            bead.status,
            beads_polis::bead::Status::Closed,
            "Bead {} should be closed but is {:?}",
            id,
            bead.status
        );
    }
}

/// Test that uses EventLog for concurrent writes and verifies SQLite index.
/// Ignored until EventLog and Index are implemented.
#[test]
#[ignore]
fn concurrent_eventlog_writes_with_index_integrity() {
    // TODO: When EventLog is implemented:
    // 1. Create EventLog pointing at temp dir
    // 2. Spawn 20 threads, each doing 50 iterations via EventLog::append
    // 3. After all threads complete:
    //    - Read all events from EventLog
    //    - Verify all lines are valid JSON
    //    - Rebuild SQLite index
    //    - Run PRAGMA integrity_check
    //    - Verify replayed state matches index state
    panic!("EventLog not yet implemented");
}
