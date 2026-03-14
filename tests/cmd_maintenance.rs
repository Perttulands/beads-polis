//! Integration tests for maintenance commands (doctor, rebuild, compact)
//! and end-to-end CLI tests using the `br` binary.

use beads_polis::compact;
use beads_polis::doctor;
use beads_polis::engine::Engine;
use beads_polis::event::{BeadSnapshot, Event};
use beads_polis::index::Index;
use beads_polis::log::EventLog;
use chrono::Utc;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_snapshot(id: &str, title: &str) -> BeadSnapshot {
    let now = Utc::now();
    BeadSnapshot {
        id: id.to_string(),
        title: title.to_string(),
        description: None,
        status: "open".to_string(),
        priority: 2,
        bead_type: "task".to_string(),
        project: "test".to_string(),
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

fn create_event(id: &str, title: &str) -> Event {
    Event::Create {
        ts: Utc::now(),
        actor: "test".to_string(),
        bead: test_snapshot(id, title),
    }
}

fn update_event(id: &str, fields: std::collections::HashMap<String, serde_json::Value>) -> Event {
    Event::Update {
        ts: Utc::now(),
        actor: "test".to_string(),
        id: id.to_string(),
        fields,
    }
}

/// Set up a beads dir with N create events, returning (TempDir, beads_dir path).
fn setup_beads_dir(count: usize) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    fs::create_dir_all(&beads_dir).unwrap();

    let log_path = beads_dir.join("events.jsonl");
    let log = EventLog::open(&log_path).unwrap();

    for i in 0..count {
        let id = format!("pol-{:04x}", i);
        let event = create_event(&id, &format!("Bead {}", i));
        log.append(&event).unwrap();
    }

    (tmp, beads_dir)
}

// ---------------------------------------------------------------------------
// Doctor tests
// ---------------------------------------------------------------------------

#[test]
fn doctor_on_clean_db_reports_healthy() {
    let (_tmp, beads_dir) = setup_beads_dir(3);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    // Open engine to build the index
    let _engine = Engine::open(&beads_dir).unwrap();

    let diag = doctor::diagnose(&log_path, &db_path);

    assert_eq!(diag.jsonl_lines, 3);
    assert_eq!(diag.jsonl_valid_lines, 3);
    assert_eq!(diag.jsonl_invalid_lines, 0);
    assert!(!diag.truncated_last_line);
    assert!(!diag.index_watermark_stale);
    assert_eq!(diag.sqlite_integrity, "ok");
    assert!(diag.stale_claims.is_empty());
}

#[test]
fn doctor_detects_stale_watermark() {
    let (_tmp, beads_dir) = setup_beads_dir(5);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    // Open engine so index.db and watermark exist
    let _engine = Engine::open(&beads_dir).unwrap();

    // Manually set watermark to 0 to simulate staleness
    let wm_path = beads_dir.join("index.watermark");
    fs::write(&wm_path, "0").unwrap();

    let diag = doctor::diagnose(&log_path, &db_path);

    assert!(diag.index_watermark_stale);
    assert_eq!(diag.index_watermark, Some(0));
}

#[test]
fn doctor_detects_truncated_line() {
    let (_tmp, beads_dir) = setup_beads_dir(2);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    // Append a truncated JSON line
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .unwrap();
    writeln!(f, r#"{{"op":"update","ts":"2026-03-12T10:00:00Z","actor":"te"#).unwrap();
    f.sync_all().unwrap();

    let diag = doctor::diagnose(&log_path, &db_path);

    assert!(diag.truncated_last_line);
    assert_eq!(diag.jsonl_invalid_lines, 1);
}

// ---------------------------------------------------------------------------
// Rebuild tests
// ---------------------------------------------------------------------------

#[test]
fn rebuild_produces_correct_index() {
    let (_tmp, beads_dir) = setup_beads_dir(3);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    // Build the index initially via Engine
    let _engine = Engine::open(&beads_dir).unwrap();
    drop(_engine);

    // Delete the index
    fs::remove_file(&db_path).unwrap();
    assert!(!db_path.exists());

    // Rebuild from the log
    let log = EventLog::open(&log_path).unwrap();
    let events = log.read_all().unwrap();
    Index::rebuild(&db_path, &events).unwrap();

    // Open and verify all beads are queryable
    let index = Index::open_or_rebuild(&db_path, &log).unwrap();
    for i in 0..3 {
        let id = format!("pol-{:04x}", i);
        let bead = index.query_show(&id);
        assert!(bead.is_some(), "Bead {} should exist after rebuild", id);
        assert_eq!(bead.unwrap().title, format!("Bead {}", i));
    }
}

#[test]
fn rebuild_after_new_events() {
    let (_tmp, beads_dir) = setup_beads_dir(2);
    let log_path = beads_dir.join("events.jsonl");
    let db_path = beads_dir.join("index.db");

    // Build initial index
    let _engine = Engine::open(&beads_dir).unwrap();
    drop(_engine);

    // Append more events directly to the log
    let log = EventLog::open(&log_path).unwrap();
    let extra_event = create_event("pol-new1", "New bead after index");
    log.append(&extra_event).unwrap();

    // Delete index and rebuild
    fs::remove_file(&db_path).unwrap();
    let events = log.read_all().unwrap();
    Index::rebuild(&db_path, &events).unwrap();

    // Verify new bead is visible
    let index = Index::open_or_rebuild(&db_path, &log).unwrap();
    let bead = index.query_show("pol-new1");
    assert!(bead.is_some(), "Newly added bead should be visible after rebuild");
    assert_eq!(bead.unwrap().title, "New bead after index");

    // Original beads should still be there
    let bead0 = index.query_show("pol-0000");
    assert!(bead0.is_some());
}

// ---------------------------------------------------------------------------
// Compact tests
// ---------------------------------------------------------------------------

#[test]
fn compact_reduces_to_snapshots() {
    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    fs::create_dir_all(&beads_dir).unwrap();

    let log_path = beads_dir.join("events.jsonl");
    let snap_dir = beads_dir.join("snapshots");
    let log = EventLog::open(&log_path).unwrap();

    // Write 5 beads, each with create + 3 updates = 20 events total
    for i in 0..5 {
        let id = format!("pol-{:04x}", i);
        let create = create_event(&id, &format!("Bead {}", i));
        log.append(&create).unwrap();

        for j in 0..3 {
            let mut fields = std::collections::HashMap::new();
            fields.insert(
                "title".to_string(),
                serde_json::json!(format!("Bead {} v{}", i, j + 1)),
            );
            let update = update_event(&id, fields);
            log.append(&update).unwrap();
        }
    }

    let old_count = log.line_count().unwrap();
    assert_eq!(old_count, 20);

    // Compact
    let bead_count = compact::compact(&log_path, &snap_dir).unwrap();
    assert_eq!(bead_count, 5);

    // New log should have exactly 5 lines (one snapshot per bead)
    let new_log = EventLog::open(&log_path).unwrap();
    let new_count = new_log.line_count().unwrap();
    assert_eq!(new_count, 5);
    assert!(new_count < old_count);

    // Archived snapshot should exist
    let archive_entries: Vec<_> = fs::read_dir(&snap_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!archive_entries.is_empty(), "Archive snapshot should exist");
}

#[test]
fn compact_preserves_bead_state() {
    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    fs::create_dir_all(&beads_dir).unwrap();

    let log_path = beads_dir.join("events.jsonl");
    let snap_dir = beads_dir.join("snapshots");
    let db_path = beads_dir.join("index.db");
    let log = EventLog::open(&log_path).unwrap();

    // Create beads with specific state transitions
    for i in 0..5 {
        let id = format!("pol-{:04x}", i);
        let create = create_event(&id, &format!("Bead {}", i));
        log.append(&create).unwrap();

        // Update title and priority
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "title".to_string(),
            serde_json::json!(format!("Final title {}", i)),
        );
        fields.insert("priority".to_string(), serde_json::json!(i as u64));
        let update = update_event(&id, fields);
        log.append(&update).unwrap();
    }

    // Build index before compaction to capture expected state
    let events_before = log.read_all().unwrap();
    Index::rebuild(&db_path, &events_before).unwrap();
    let index_before = Index::open_or_rebuild(&db_path, &log).unwrap();

    let mut expected_titles: Vec<(String, String, u8)> = Vec::new();
    for i in 0..5 {
        let id = format!("pol-{:04x}", i);
        let bead = index_before.query_show(&id).unwrap();
        expected_titles.push((id, bead.title.clone(), bead.priority));
    }
    drop(index_before);

    // Compact
    compact::compact(&log_path, &snap_dir).unwrap();

    // Rebuild index from compacted log
    let log_after = EventLog::open(&log_path).unwrap();
    let events_after = log_after.read_all().unwrap();
    // Remove stale index files before rebuild
    let _ = fs::remove_file(&db_path);
    Index::rebuild(&db_path, &events_after).unwrap();
    let index_after = Index::open_or_rebuild(&db_path, &log_after).unwrap();

    // Verify all beads have correct state
    for (id, expected_title, expected_priority) in &expected_titles {
        let bead = index_after.query_show(id);
        assert!(bead.is_some(), "Bead {} should exist after compaction", id);
        let bead = bead.unwrap();
        assert_eq!(
            &bead.title, expected_title,
            "Title mismatch for bead {}",
            id
        );
        assert_eq!(
            bead.priority, *expected_priority,
            "Priority mismatch for bead {}",
            id
        );
    }
}

// ---------------------------------------------------------------------------
// End-to-end CLI tests
// ---------------------------------------------------------------------------

/// Find the built `br` binary path.
fn br_binary() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let bin = std::path::Path::new(manifest_dir)
        .join("target")
        .join("debug")
        .join("br");
    assert!(bin.exists(), "Binary not found at {:?}. Run `cargo build` first.", bin);
    bin
}

#[test]
fn cli_create_and_show() {
    let tmp = TempDir::new().unwrap();
    let db_dir = tmp.path().join(".beads");

    let br = br_binary();

    // Create a bead
    let output = std::process::Command::new(&br)
        .args([
            "create",
            "Test CLI bead",
            "--actor",
            "test-cli",
            "--db",
            db_dir.to_str().unwrap(),
            "--json",
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br create");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "br create failed: stdout={}, stderr={}",
        stdout,
        stderr
    );

    // Parse the JSON output to get the bead ID
    let create_json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Failed to parse create output as JSON: {e}\nstdout: {stdout}"));
    let bead_id = create_json["id"]
        .as_str()
        .expect("create output should have an 'id' field");

    // Show the bead
    let output = std::process::Command::new(&br)
        .args([
            "show",
            bead_id,
            "--actor",
            "test-cli",
            "--db",
            db_dir.to_str().unwrap(),
            "--json",
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br show");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "br show failed: stdout={}, stderr={}",
        stdout,
        stderr
    );

    let show_json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Failed to parse show output as JSON: {e}\nstdout: {stdout}"));
    assert_eq!(show_json["id"].as_str(), Some(bead_id));
    assert_eq!(show_json["title"].as_str(), Some("Test CLI bead"));
}

#[test]
fn cli_list_json() {
    let tmp = TempDir::new().unwrap();
    let db_dir = tmp.path().join(".beads");

    let br = br_binary();

    // Create a bead
    let output = std::process::Command::new(&br)
        .args([
            "create",
            "Listable bead",
            "--actor",
            "test-cli",
            "--db",
            db_dir.to_str().unwrap(),
            "--json",
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br create");
    assert!(output.status.success(), "br create failed");

    // List beads as JSON
    let output = std::process::Command::new(&br)
        .args([
            "list",
            "--json",
            "--actor",
            "test-cli",
            "--db",
            db_dir.to_str().unwrap(),
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "br list --json failed: stdout={}, stderr={}",
        stdout,
        stderr
    );

    let list_json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Failed to parse list output as JSON: {e}\nstdout: {stdout}"));
    let arr = list_json
        .as_array()
        .expect("list output should be a JSON array");
    assert!(!arr.is_empty(), "list output should contain at least one bead");
    assert_eq!(arr[0]["title"].as_str(), Some("Listable bead"));
}

#[test]
fn cli_doctor_json() {
    let tmp = TempDir::new().unwrap();
    let db_dir = tmp.path().join(".beads");

    let br = br_binary();

    // Create a bead first so there is data
    let output = std::process::Command::new(&br)
        .args([
            "create",
            "Doctor test bead",
            "--actor",
            "test-cli",
            "--db",
            db_dir.to_str().unwrap(),
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br create");
    assert!(output.status.success(), "br create failed");

    // Run doctor with JSON output
    let output = std::process::Command::new(&br)
        .args([
            "doctor",
            "--json",
            "--db",
            db_dir.to_str().unwrap(),
        ])
        .env("POLIS_ACTOR", "test-cli")
        .output()
        .expect("failed to run br doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "br doctor --json failed: stdout={}, stderr={}",
        stdout,
        stderr
    );

    let doc_json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Failed to parse doctor output as JSON: {e}\nstdout: {stdout}"));
    assert!(doc_json["jsonl_lines"].is_number(), "doctor output should have jsonl_lines");
    assert_eq!(doc_json["jsonl_lines"].as_u64(), Some(1));
    assert_eq!(doc_json["jsonl_valid_lines"].as_u64(), Some(1));
    assert_eq!(doc_json["jsonl_invalid_lines"].as_u64(), Some(0));
    assert_eq!(doc_json["truncated_last_line"].as_bool(), Some(false));
}
