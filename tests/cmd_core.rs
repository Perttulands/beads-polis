//! Integration tests for beads-v2 core commands.
//!
//! Tests the real Engine (JSONL log + SQLite index) in isolated temp dirs.
//! Pattern: append events via engine.log, re-open engine to rebuild index, query.

use beads_polis::engine::Engine;
use beads_polis::event::{BeadSnapshot, Event};
use beads_polis::index::Filters;
use beads_polis::bead::Status;
use chrono::Utc;
use std::collections::HashMap;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create an Engine in a fresh temp dir, returning (engine, dir).
/// The TempDir must be held alive for the duration of the test.
fn fresh_engine() -> (Engine, TempDir) {
    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    let engine = Engine::open(&beads_dir).unwrap();
    (engine, tmp)
}

/// Re-open the engine from the same temp dir.
/// Removes the index DB to force a full rebuild from JSONL.
fn reopen(tmp: &TempDir) -> Engine {
    let beads_dir = tmp.path().join(".beads");
    // Remove the index so open_or_rebuild does a full replay from JSONL.
    let _ = std::fs::remove_file(beads_dir.join("index.db"));
    let _ = std::fs::remove_file(beads_dir.join("index.db-wal"));
    let _ = std::fs::remove_file(beads_dir.join("index.db-shm"));
    Engine::open(&beads_dir).unwrap()
}

/// Build a minimal Create event.
fn create_event(id: &str, title: &str, project: &str, priority: u8) -> Event {
    let now = Utc::now();
    Event::Create {
        ts: now,
        actor: "test".into(),
        bead: BeadSnapshot {
            id: id.into(),
            title: title.into(),
            description: None,
            status: "open".into(),
            priority,
            bead_type: "task".into(),
            project: project.into(),
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
        },
    }
}

/// Build a Create event with dependencies.
fn create_event_with_deps(id: &str, title: &str, deps: Vec<String>) -> Event {
    let now = Utc::now();
    Event::Create {
        ts: now,
        actor: "test".into(),
        bead: BeadSnapshot {
            id: id.into(),
            title: title.into(),
            description: None,
            status: "open".into(),
            priority: 2,
            bead_type: "task".into(),
            project: "test".into(),
            assignee: None,
            parent: None,
            dependencies: deps,
            labels: vec![],
            created_at: now,
            updated_at: now,
            closed_at: None,
            close_reason: None,
            claimed_at: None,
            claim_deadline: None,
            last_heartbeat: None,
        },
    }
}

/// Build a Create event with a description.
fn create_event_with_desc(id: &str, title: &str, desc: &str) -> Event {
    let now = Utc::now();
    Event::Create {
        ts: now,
        actor: "test".into(),
        bead: BeadSnapshot {
            id: id.into(),
            title: title.into(),
            description: Some(desc.into()),
            status: "open".into(),
            priority: 2,
            bead_type: "task".into(),
            project: "test".into(),
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
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn create_and_show_roundtrip() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Fix auth bug", "gate", 1)).unwrap();

    let engine = reopen(&tmp);
    let bead = engine.index.query_show("t-0001");
    assert!(bead.is_some(), "bead should exist after create");
    let bead = bead.unwrap();
    assert_eq!(bead.id, "t-0001");
    assert_eq!(bead.title, "Fix auth bug");
    assert_eq!(bead.project, "gate");
    assert_eq!(bead.priority, 1);
    assert_eq!(bead.status, Status::Open);
}

#[test]
fn list_filters_by_status() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Open one", "proj", 2)).unwrap();
    engine.log.append(&create_event("t-0002", "Open two", "proj", 2)).unwrap();
    engine.log.append(&create_event("t-0003", "Will close", "proj", 2)).unwrap();
    // Close the third bead
    engine.log.append(&Event::Close {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-0003".into(),
        reason: "Done".into(),
    }).unwrap();

    let engine = reopen(&tmp);

    let open = engine.index.query_list(&Filters {
        status: Some(Status::Open),
        ..Filters::default()
    });
    assert_eq!(open.len(), 2, "should have 2 open beads");

    let closed = engine.index.query_list(&Filters {
        status: Some(Status::Closed),
        ..Filters::default()
    });
    assert_eq!(closed.len(), 1, "should have 1 closed bead");
    assert_eq!(closed[0].id, "t-0003");
}

#[test]
fn list_filters_by_project() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Gate task", "gate", 2)).unwrap();
    engine.log.append(&create_event("t-0002", "Work task", "work", 2)).unwrap();
    engine.log.append(&create_event("t-0003", "Gate task 2", "gate", 2)).unwrap();

    let engine = reopen(&tmp);

    let gate_beads = engine.index.query_list(&Filters {
        project: Some("gate".into()),
        ..Filters::default()
    });
    assert_eq!(gate_beads.len(), 2, "should have 2 gate beads");
    assert!(gate_beads.iter().all(|b| b.project == "gate"));

    let work_beads = engine.index.query_list(&Filters {
        project: Some("work".into()),
        ..Filters::default()
    });
    assert_eq!(work_beads.len(), 1);
}

#[test]
fn list_filters_by_priority() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Critical", "proj", 0)).unwrap();
    engine.log.append(&create_event("t-0002", "Medium", "proj", 2)).unwrap();
    engine.log.append(&create_event("t-0003", "Backlog", "proj", 4)).unwrap();

    let engine = reopen(&tmp);

    let critical = engine.index.query_list(&Filters {
        priority: Some(0),
        ..Filters::default()
    });
    assert_eq!(critical.len(), 1);
    assert_eq!(critical[0].title, "Critical");

    let backlog = engine.index.query_list(&Filters {
        priority: Some(4),
        ..Filters::default()
    });
    assert_eq!(backlog.len(), 1);
    assert_eq!(backlog[0].title, "Backlog");
}

#[test]
fn update_modifies_fields() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Old title", "proj", 2)).unwrap();

    // Update title and priority
    let mut fields = HashMap::new();
    fields.insert("title".into(), serde_json::json!("New title"));
    fields.insert("priority".into(), serde_json::json!(0));
    engine.log.append(&Event::Update {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-0001".into(),
        fields,
    }).unwrap();

    let engine = reopen(&tmp);
    let bead = engine.index.query_show("t-0001").unwrap();
    assert_eq!(bead.title, "New title");
    assert_eq!(bead.priority, 0);
}

#[test]
fn update_add_remove_deps() {
    let (engine, tmp) = fresh_engine();

    // Create beads A, B, C. B depends on A.
    engine.log.append(&create_event("t-a", "Bead A", "proj", 2)).unwrap();
    engine.log.append(&create_event_with_deps("t-b", "Bead B", vec!["t-a".into()])).unwrap();
    engine.log.append(&create_event("t-c", "Bead C", "proj", 2)).unwrap();

    // Add dep on C, remove dep on A
    let mut fields = HashMap::new();
    fields.insert("dependencies".into(), serde_json::json!(["t-c"]));
    engine.log.append(&Event::Update {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-b".into(),
        fields,
    }).unwrap();

    let engine = reopen(&tmp);
    let bead_b = engine.index.query_show("t-b").unwrap();
    assert_eq!(bead_b.dependencies, vec!["t-c".to_string()]);
    assert!(!bead_b.dependencies.contains(&"t-a".to_string()));
}

#[test]
fn close_sets_status_and_reason() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "To close", "proj", 2)).unwrap();
    engine.log.append(&Event::Close {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-0001".into(),
        reason: "Completed successfully".into(),
    }).unwrap();

    let engine = reopen(&tmp);
    let bead = engine.index.query_show("t-0001").unwrap();
    assert_eq!(bead.status, Status::Closed);
    assert_eq!(bead.close_reason.as_deref(), Some("Completed successfully"));
    assert!(bead.closed_at.is_some());
}

#[test]
fn close_already_closed_bead() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Close me", "proj", 2)).unwrap();
    engine.log.append(&Event::Close {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-0001".into(),
        reason: "First close".into(),
    }).unwrap();
    // Close again — should not error at event level
    engine.log.append(&Event::Close {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-0001".into(),
        reason: "Second close".into(),
    }).unwrap();

    let engine = reopen(&tmp);
    let bead = engine.index.query_show("t-0001").unwrap();
    assert_eq!(bead.status, Status::Closed);
    // Last close reason wins
    assert_eq!(bead.close_reason.as_deref(), Some("Second close"));
}

#[test]
fn ready_excludes_blocked() {
    let (engine, tmp) = fresh_engine();

    // A is open, B depends on A (so B is blocked)
    engine.log.append(&create_event("t-a", "Bead A", "proj", 2)).unwrap();
    engine.log.append(&create_event_with_deps("t-b", "Bead B", vec!["t-a".into()])).unwrap();

    let engine = reopen(&tmp);
    let ready = engine.index.query_ready(None);

    let ready_ids: Vec<&str> = ready.iter().map(|b| b.id.as_str()).collect();
    assert!(ready_ids.contains(&"t-a"), "A should be ready (no deps)");
    assert!(!ready_ids.contains(&"t-b"), "B should NOT be ready (blocked by A)");
}

#[test]
fn ready_includes_unblocked() {
    let (engine, tmp) = fresh_engine();

    // A is open, B depends on A
    engine.log.append(&create_event("t-a", "Bead A", "proj", 2)).unwrap();
    engine.log.append(&create_event_with_deps("t-b", "Bead B", vec!["t-a".into()])).unwrap();

    // Close A — B should become unblocked
    engine.log.append(&Event::Close {
        ts: Utc::now(),
        actor: "test".into(),
        id: "t-a".into(),
        reason: "Done".into(),
    }).unwrap();

    let engine = reopen(&tmp);
    let ready = engine.index.query_ready(None);

    let ready_ids: Vec<&str> = ready.iter().map(|b| b.id.as_str()).collect();
    assert!(ready_ids.contains(&"t-b"), "B should be ready after A is closed");
    // A is closed, so it should NOT appear in ready
    assert!(!ready_ids.contains(&"t-a"), "A is closed, should not be ready");
}

#[test]
fn search_finds_by_title() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event("t-0001", "Fix authentication timeout", "proj", 2)).unwrap();
    engine.log.append(&create_event("t-0002", "Add logging", "proj", 2)).unwrap();

    let engine = reopen(&tmp);
    let results = engine.index.query_search("authentication");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "t-0001");
}

#[test]
fn search_finds_by_description() {
    let (engine, tmp) = fresh_engine();

    engine.log.append(&create_event_with_desc(
        "t-0001",
        "Generic title",
        "This involves refactoring the database layer",
    )).unwrap();
    engine.log.append(&create_event("t-0002", "Unrelated", "proj", 2)).unwrap();

    let engine = reopen(&tmp);
    let results = engine.index.query_search("database layer");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "t-0001");
}

#[test]
fn id_generation_is_unique() {
    let (engine, _tmp) = fresh_engine();

    // 2 random bytes = 65536 possible values. Generate 10 to keep collision
    // probability negligible (~0.07%) while still verifying the mechanism.
    let mut ids = std::collections::HashSet::new();
    for _ in 0..10 {
        let id = engine.generate_id();
        // Verify format: prefix-XXXX (4 hex chars)
        assert!(id.starts_with("pol-"), "bad prefix: {}", id);
        assert_eq!(id.len(), 8, "bad length: {}", id);
        ids.insert(id);
    }
    // With 10 draws from 65536, collisions are extremely unlikely
    assert!(ids.len() >= 9, "too many collisions: only {} unique out of 10", ids.len());
}
