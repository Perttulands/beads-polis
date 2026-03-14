//! Integration tests for agent workflow commands: claim, heartbeat, unclaim.
//!
//! These exercise the real Engine (EventLog + Index) using tempfile isolation.
//! Since CLI command handlers are private, we replicate their logic by appending
//! events to the log and rebuilding/upserting the index — exactly what the
//! handlers do internally.

use beads_v2::bead::Status;
use beads_v2::engine::Engine;
use beads_v2::event::{BeadSnapshot, Event};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a fresh Engine in a temp directory.
fn fresh_engine() -> (Engine, TempDir) {
    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    let engine = Engine::open(&beads_dir).unwrap();
    (engine, tmp)
}

/// Insert a bead via Create event, append to log, and upsert into the index.
fn create_bead(engine: &Engine, id: &str, title: &str) {
    let now = Utc::now();
    let event = Event::Create {
        ts: now,
        actor: "setup".to_string(),
        bead: BeadSnapshot {
            id: id.to_string(),
            title: title.to_string(),
            description: None,
            status: "open".to_string(),
            priority: 2,
            bead_type: "task".to_string(),
            project: "test-project".to_string(),
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
    };
    engine.log.append(&event).unwrap();
    // Upsert into index so query_show works immediately
    let bead = beads_v2::bead::Bead::from_snapshot(match &event {
        Event::Create { bead, .. } => bead,
        _ => unreachable!(),
    });
    engine.index.upsert_bead(&bead).unwrap();
}

/// Apply a claim: status=in_progress, set assignee, claimed_at, claim_deadline, last_heartbeat.
fn apply_claim(engine: &Engine, id: &str, actor: &str, lock_secs: i64) {
    let now = Utc::now();
    let deadline = now + Duration::seconds(lock_secs);
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("in_progress"));
    fields.insert("assignee".to_string(), serde_json::json!(actor));
    fields.insert("claimed_at".to_string(), serde_json::json!(now.to_rfc3339()));
    fields.insert("claim_deadline".to_string(), serde_json::json!(deadline.to_rfc3339()));
    fields.insert("last_heartbeat".to_string(), serde_json::json!(now.to_rfc3339()));

    let event = Event::Update {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    };
    engine.log.append(&event).unwrap();

    let mut bead = engine.index.query_show(id).unwrap();
    bead.apply_event(&event);
    engine.index.upsert_bead(&bead).unwrap();
}

/// Apply a claim with an already-expired deadline (for testing re-claim after expiry).
fn apply_expired_claim(engine: &Engine, id: &str, actor: &str) {
    let now = Utc::now();
    let past_deadline = now - Duration::hours(1);
    let claimed_at = now - Duration::hours(2);
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("in_progress"));
    fields.insert("assignee".to_string(), serde_json::json!(actor));
    fields.insert("claimed_at".to_string(), serde_json::json!(claimed_at.to_rfc3339()));
    fields.insert("claim_deadline".to_string(), serde_json::json!(past_deadline.to_rfc3339()));
    fields.insert("last_heartbeat".to_string(), serde_json::json!(claimed_at.to_rfc3339()));

    let event = Event::Update {
        ts: claimed_at,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    };
    engine.log.append(&event).unwrap();

    let mut bead = engine.index.query_show(id).unwrap();
    bead.apply_event(&event);
    engine.index.upsert_bead(&bead).unwrap();
}

/// Apply a heartbeat: extends claim_deadline by 1 hour from now.
fn apply_heartbeat(engine: &Engine, id: &str, actor: &str) {
    let now = Utc::now();
    let new_deadline = now + Duration::hours(1);
    let mut fields = HashMap::new();
    fields.insert("last_heartbeat".to_string(), serde_json::json!(now.to_rfc3339()));
    fields.insert("claim_deadline".to_string(), serde_json::json!(new_deadline.to_rfc3339()));

    let event = Event::Update {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    };
    engine.log.append(&event).unwrap();

    let mut bead = engine.index.query_show(id).unwrap();
    bead.apply_event(&event);
    engine.index.upsert_bead(&bead).unwrap();
}

/// Apply an unclaim: revert to open, clear all claim fields.
fn apply_unclaim(engine: &Engine, id: &str, actor: &str) {
    let now = Utc::now();
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("open"));
    fields.insert("assignee".to_string(), serde_json::Value::Null);
    fields.insert("claimed_at".to_string(), serde_json::Value::Null);
    fields.insert("claim_deadline".to_string(), serde_json::Value::Null);
    fields.insert("last_heartbeat".to_string(), serde_json::Value::Null);

    let event = Event::Update {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    };
    engine.log.append(&event).unwrap();

    let mut bead = engine.index.query_show(id).unwrap();
    bead.apply_event(&event);
    engine.index.upsert_bead(&bead).unwrap();
}

/// Apply a close event.
fn apply_close(engine: &Engine, id: &str, actor: &str, reason: &str) {
    let now = Utc::now();
    let event = Event::Close {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        reason: reason.to_string(),
    };
    engine.log.append(&event).unwrap();

    let mut bead = engine.index.query_show(id).unwrap();
    bead.apply_event(&event);
    engine.index.upsert_bead(&bead).unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn claim_sets_in_progress_and_assignee() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-001", "Test claim bead");
    apply_claim(&engine, "ag-001", "athena", 3600);

    let bead = engine.index.query_show("ag-001").unwrap();
    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));
    assert!(bead.claimed_at.is_some(), "claimed_at should be set");
    assert!(bead.claim_deadline.is_some(), "claim_deadline should be set");
    assert!(bead.last_heartbeat.is_some(), "last_heartbeat should be set");

    // Deadline should be ~1 hour from claimed_at
    let claimed = bead.claimed_at.unwrap();
    let deadline = bead.claim_deadline.unwrap();
    let diff = (deadline - claimed).num_seconds();
    assert!(
        (3590..=3610).contains(&diff),
        "Deadline should be ~3600s from claimed_at, got {}s",
        diff
    );
}

#[test]
fn claim_by_different_actor_on_active_claim() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-002", "Double claim test");
    apply_claim(&engine, "ag-002", "athena", 3600);

    let bead = engine.index.query_show("ag-002").unwrap();

    // Replicate the CLI's conflict check: in_progress + different holder + active deadline
    assert_eq!(bead.status, Status::InProgress);
    let holder = bead.assignee.as_deref().unwrap();
    assert_eq!(holder, "athena");
    let deadline_active = bead.claim_deadline.map(|d| d > Utc::now()).unwrap_or(false);
    assert!(deadline_active, "Claim deadline should still be active");

    // A second actor ("hermes") should be rejected by CLI logic
    let second_actor = "hermes";
    let is_conflict = bead.status == Status::InProgress
        && bead.assignee.as_deref().map(|h| h != second_actor).unwrap_or(false)
        && bead.claim_deadline.map(|d| d > Utc::now()).unwrap_or(false);
    assert!(is_conflict, "Second actor should be blocked by active claim");
}

#[test]
fn claim_after_deadline_expired() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-003", "Expired claim test");
    apply_expired_claim(&engine, "ag-003", "athena");

    let bead = engine.index.query_show("ag-003").unwrap();

    // Verify deadline has passed
    assert!(bead.claim_deadline.unwrap() < Utc::now(), "Deadline should be expired");

    // The expired-deadline check that cmd_claim uses: deadline <= now means re-claim is allowed
    let deadline_expired = bead.claim_deadline.map(|d| d <= Utc::now()).unwrap_or(true);
    assert!(deadline_expired, "CLI should allow re-claim after expiry");

    // Apply re-claim by different actor
    apply_claim(&engine, "ag-003", "hermes", 3600);

    let bead2 = engine.index.query_show("ag-003").unwrap();
    assert_eq!(bead2.status, Status::InProgress);
    assert_eq!(bead2.assignee.as_deref(), Some("hermes"));
    assert!(bead2.claim_deadline.unwrap() > Utc::now());
}

#[test]
fn heartbeat_extends_deadline() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-004", "Heartbeat test");
    apply_claim(&engine, "ag-004", "athena", 3600);

    let before = engine.index.query_show("ag-004").unwrap();
    let old_deadline = before.claim_deadline.unwrap();

    // Small sleep not needed — heartbeat sets deadline to now+1h which is always >= old
    apply_heartbeat(&engine, "ag-004", "athena");

    let after = engine.index.query_show("ag-004").unwrap();
    assert!(after.last_heartbeat.is_some(), "last_heartbeat should be set");

    let new_deadline = after.claim_deadline.unwrap();
    assert!(
        new_deadline >= old_deadline,
        "Heartbeat should extend (or maintain) the deadline"
    );

    // New deadline should be ~1 hour from the heartbeat timestamp
    let hb = after.last_heartbeat.unwrap();
    let diff = (new_deadline - hb).num_seconds();
    assert!(
        (3590..=3610).contains(&diff),
        "Deadline should be ~1h from heartbeat, got {}s",
        diff
    );
}

#[test]
fn unclaim_reverts_to_open() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-005", "Unclaim test");
    apply_claim(&engine, "ag-005", "athena", 3600);

    // Verify claimed state first
    let claimed = engine.index.query_show("ag-005").unwrap();
    assert_eq!(claimed.status, Status::InProgress);

    apply_unclaim(&engine, "ag-005", "athena");

    let bead = engine.index.query_show("ag-005").unwrap();
    assert_eq!(bead.status, Status::Open);
    assert!(bead.assignee.is_none(), "assignee should be cleared");
    assert!(bead.claimed_at.is_none(), "claimed_at should be cleared");
    assert!(bead.claim_deadline.is_none(), "claim_deadline should be cleared");
    assert!(bead.last_heartbeat.is_none(), "last_heartbeat should be cleared");
}

#[test]
fn full_claim_lifecycle() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-006", "Full lifecycle");

    // 1. Starts open
    let b = engine.index.query_show("ag-006").unwrap();
    assert_eq!(b.status, Status::Open);
    assert!(b.assignee.is_none());

    // 2. Claim by athena
    apply_claim(&engine, "ag-006", "athena", 3600);
    let b = engine.index.query_show("ag-006").unwrap();
    assert_eq!(b.status, Status::InProgress);
    assert_eq!(b.assignee.as_deref(), Some("athena"));

    // 3. Heartbeat
    apply_heartbeat(&engine, "ag-006", "athena");
    let b = engine.index.query_show("ag-006").unwrap();
    assert!(b.last_heartbeat.is_some());
    assert!(b.claim_deadline.unwrap() > Utc::now());

    // 4. Unclaim
    apply_unclaim(&engine, "ag-006", "athena");
    let b = engine.index.query_show("ag-006").unwrap();
    assert_eq!(b.status, Status::Open);
    assert!(b.assignee.is_none());

    // 5. Re-claim by hermes
    apply_claim(&engine, "ag-006", "hermes", 7200);
    let b = engine.index.query_show("ag-006").unwrap();
    assert_eq!(b.status, Status::InProgress);
    assert_eq!(b.assignee.as_deref(), Some("hermes"));
}

#[test]
fn close_by_assignee_succeeds() {
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-007", "Close by assignee");
    apply_claim(&engine, "ag-007", "athena", 3600);

    apply_close(&engine, "ag-007", "athena", "task completed");

    let bead = engine.index.query_show("ag-007").unwrap();
    assert_eq!(bead.status, Status::Closed);
    assert_eq!(bead.close_reason.as_deref(), Some("task completed"));
    assert!(bead.closed_at.is_some());
}

#[test]
fn close_by_non_assignee_state() {
    // Events are append-only — the Close event always applies at the data layer.
    // Permission checks are in the CLI layer. Here we verify the event applies correctly
    // regardless of who the actor is.
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-008", "Close by non-assignee");
    apply_claim(&engine, "ag-008", "athena", 3600);

    // Apply close by different actor (would be rejected by CLI, but events are append-only)
    apply_close(&engine, "ag-008", "hermes", "overridden close");

    let bead = engine.index.query_show("ag-008").unwrap();
    assert_eq!(bead.status, Status::Closed);
    assert_eq!(bead.close_reason.as_deref(), Some("overridden close"));
    assert!(bead.closed_at.is_some());
    // Assignee field is NOT cleared by close — it records who was working on it
    assert_eq!(bead.assignee.as_deref(), Some("athena"));
}

#[test]
fn eventlog_claim_rejection() {
    // Full round-trip through EventLog: create, claim, verify conflict detection,
    // then re-claim after expiry.
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-009", "EventLog claim rejection");

    // Claim as athena with a short deadline
    apply_claim(&engine, "ag-009", "athena", 3600);

    let bead = engine.index.query_show("ag-009").unwrap();
    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));

    // Attempt claim as hermes — replicate cmd_claim conflict check
    let second_actor = "hermes";
    let is_conflict = bead.status == Status::InProgress
        && bead.assignee.as_deref().map(|h| h != second_actor).unwrap_or(false)
        && bead.claim_deadline.map(|d| d > Utc::now()).unwrap_or(false);
    assert!(is_conflict, "hermes should be rejected while athena's claim is active");

    // Verify structured error fields exist
    let holder = bead.assignee.as_deref().unwrap();
    let deadline = bead.claim_deadline.unwrap().to_rfc3339();
    assert_eq!(holder, "athena");
    assert!(!deadline.is_empty());

    // Now simulate expiry: apply an expired claim overwriting the active one
    apply_expired_claim(&engine, "ag-009", "athena");

    let bead2 = engine.index.query_show("ag-009").unwrap();
    let deadline_expired = bead2.claim_deadline.map(|d| d <= Utc::now()).unwrap_or(true);
    assert!(deadline_expired, "Deadline should now be expired");

    // hermes can now claim
    apply_claim(&engine, "ag-009", "hermes", 3600);
    let bead3 = engine.index.query_show("ag-009").unwrap();
    assert_eq!(bead3.assignee.as_deref(), Some("hermes"));
    assert_eq!(bead3.status, Status::InProgress);
    assert!(bead3.claim_deadline.unwrap() > Utc::now());
}

#[test]
fn index_survives_rebuild_with_claim_state() {
    // Verify that claim fields survive an index rebuild from the event log.
    let (engine, _tmp) = fresh_engine();
    create_bead(&engine, "ag-010", "Rebuild persistence");
    apply_claim(&engine, "ag-010", "athena", 3600);
    apply_heartbeat(&engine, "ag-010", "athena");

    let before = engine.index.query_show("ag-010").unwrap();

    // Force a full rebuild from the JSONL log
    let db_path = engine.beads_dir.join("index.db");
    let events = engine.log.read_all().unwrap();
    beads_v2::index::Index::rebuild(&db_path, &events).unwrap();

    // Re-open the engine and verify state is preserved
    let engine2 = Engine::open(&engine.beads_dir).unwrap();
    let after = engine2.index.query_show("ag-010").unwrap();

    assert_eq!(after.status, before.status);
    assert_eq!(after.assignee, before.assignee);
    assert!(after.claimed_at.is_some());
    assert!(after.claim_deadline.is_some());
    assert!(after.last_heartbeat.is_some());
}
