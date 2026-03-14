//! Claim/heartbeat semantics tests.
//!
//! Tests the agent claim protocol: claim, heartbeat, unclaim,
//! double-claim rejection, deadline expiry, and assignee-only close.

use beads_polis::bead::{Bead, Status};
use beads_polis::event::{BeadSnapshot, Event};
use chrono::{Duration, Utc};
use std::collections::HashMap;

fn make_snapshot(id: &str) -> BeadSnapshot {
    let now = Utc::now();
    BeadSnapshot {
        id: id.to_string(),
        title: format!("Claim test {}", id),
        description: None,
        status: "open".to_string(),
        priority: 2,
        bead_type: "task".to_string(),
        project: "claim-test".to_string(),
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

fn create_event(id: &str) -> Event {
    Event::Create {
        ts: Utc::now(),
        actor: "setup".to_string(),
        bead: make_snapshot(id),
    }
}

/// Simulate a claim by building the update event the CLI would produce.
fn claim_event(id: &str, actor: &str, lock_duration: Duration) -> Event {
    let now = Utc::now();
    let deadline = now + lock_duration;
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("in_progress"));
    fields.insert("assignee".to_string(), serde_json::json!(actor));
    fields.insert(
        "claimed_at".to_string(),
        serde_json::json!(now.to_rfc3339()),
    );
    fields.insert(
        "claim_deadline".to_string(),
        serde_json::json!(deadline.to_rfc3339()),
    );

    Event::Update {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    }
}

/// Simulate a heartbeat by extending the deadline.
fn heartbeat_event(id: &str, actor: &str, extend_by: Duration) -> Event {
    let now = Utc::now();
    let new_deadline = now + extend_by;
    let mut fields = HashMap::new();
    fields.insert(
        "last_heartbeat".to_string(),
        serde_json::json!(now.to_rfc3339()),
    );
    fields.insert(
        "claim_deadline".to_string(),
        serde_json::json!(new_deadline.to_rfc3339()),
    );

    Event::Update {
        ts: now,
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    }
}

/// Simulate an unclaim by clearing assignee and reverting status.
fn unclaim_event(id: &str, actor: &str) -> Event {
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("open"));
    fields.insert("assignee".to_string(), serde_json::Value::Null);
    fields.insert("claimed_at".to_string(), serde_json::Value::Null);
    fields.insert("claim_deadline".to_string(), serde_json::Value::Null);
    fields.insert("last_heartbeat".to_string(), serde_json::Value::Null);

    Event::Update {
        ts: Utc::now(),
        actor: actor.to_string(),
        id: id.to_string(),
        fields,
    }
}

#[test]
fn claim_sets_assignee_status_deadline() {
    let create = create_event("claim-001");
    let claim = claim_event("claim-001", "athena", Duration::hours(1));

    let bead = Bead::from_events(&[create, claim]).unwrap();

    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));
    assert!(bead.claimed_at.is_some());
    assert!(bead.claim_deadline.is_some());

    // Deadline should be approximately 1 hour from claimed_at
    let claimed = bead.claimed_at.unwrap();
    let deadline = bead.claim_deadline.unwrap();
    let diff = deadline - claimed;
    // Allow some tolerance for test execution time
    assert!(
        diff.num_minutes() >= 59 && diff.num_minutes() <= 61,
        "Deadline should be ~1 hour from claimed_at, got {} minutes",
        diff.num_minutes()
    );
}

#[test]
fn second_claim_by_different_actor_should_fail() {
    // This tests the business rule validation that should happen in the CLI/EventLog layer.
    // At the event replay level, we verify what state looks like if a second claim
    // is erroneously applied (the CLI should reject this, but we test the data model).
    let create = create_event("claim-002");
    let claim1 = claim_event("claim-002", "athena", Duration::hours(1));

    let bead = Bead::from_events(&[create, claim1]).unwrap();

    // Verify the bead is claimed
    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));

    // Business rule check: another actor trying to claim should be rejected.
    // This is enforced by the CLI, not by apply_event. We test the check logic:
    let is_claimed = bead.status == Status::InProgress && bead.assignee.is_some();
    let deadline_active = bead
        .claim_deadline
        .map(|d| d > Utc::now())
        .unwrap_or(false);

    assert!(
        is_claimed && deadline_active,
        "Bead should be actively claimed"
    );

    // The structured error would be:
    // {"error":"already_claimed","holder":"athena","deadline":"...","bead":"claim-002"}
    let holder = bead.assignee.as_deref().unwrap();
    assert_eq!(holder, "athena");
}

#[test]
fn claim_after_deadline_expires_succeeds() {
    let create = create_event("claim-003");

    // First claim with a deadline in the past (simulating expired claim)
    let now = Utc::now();
    let past_deadline = now - Duration::hours(1);
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("in_progress"));
    fields.insert("assignee".to_string(), serde_json::json!("athena"));
    fields.insert(
        "claimed_at".to_string(),
        serde_json::json!((now - Duration::hours(2)).to_rfc3339()),
    );
    fields.insert(
        "claim_deadline".to_string(),
        serde_json::json!(past_deadline.to_rfc3339()),
    );
    let expired_claim = Event::Update {
        ts: now - Duration::hours(2),
        actor: "athena".to_string(),
        id: "claim-003".to_string(),
        fields,
    };

    let bead = Bead::from_events(&[create, expired_claim]).unwrap();

    // Verify the deadline has passed
    assert!(bead.claim_deadline.unwrap() < Utc::now());

    // Business rule: since deadline expired, another agent CAN claim
    let deadline_expired = bead
        .claim_deadline
        .map(|d| d < Utc::now())
        .unwrap_or(true);
    assert!(deadline_expired, "Deadline should be expired");

    // Apply second claim by different actor
    let reclaim = claim_event("claim-003", "hermes", Duration::hours(1));
    let mut bead2 = bead.clone();
    bead2.apply_event(&reclaim);

    assert_eq!(bead2.assignee.as_deref(), Some("hermes"));
    assert_eq!(bead2.status, Status::InProgress);
    assert!(bead2.claim_deadline.unwrap() > Utc::now());
}

#[test]
fn heartbeat_extends_deadline() {
    let create = create_event("claim-004");
    let claim = claim_event("claim-004", "athena", Duration::hours(1));
    let heartbeat = heartbeat_event("claim-004", "athena", Duration::hours(1));

    let bead = Bead::from_events(&[create, claim, heartbeat]).unwrap();

    assert_eq!(bead.assignee.as_deref(), Some("athena"));
    assert!(bead.last_heartbeat.is_some());

    // Deadline should be extended (approximately 1 hour from the heartbeat time)
    let hb = bead.last_heartbeat.unwrap();
    let deadline = bead.claim_deadline.unwrap();
    let diff = deadline - hb;
    assert!(
        diff.num_minutes() >= 59 && diff.num_minutes() <= 61,
        "Deadline should be ~1 hour from heartbeat, got {} minutes",
        diff.num_minutes()
    );
}

#[test]
fn unclaim_releases_the_bead() {
    let create = create_event("claim-005");
    let claim = claim_event("claim-005", "athena", Duration::hours(1));
    let unclaim = unclaim_event("claim-005", "athena");

    let bead = Bead::from_events(&[create, claim, unclaim]).unwrap();

    assert_eq!(bead.status, Status::Open);
    assert!(
        bead.assignee.is_none(),
        "Assignee should be cleared after unclaim"
    );
    assert!(
        bead.claimed_at.is_none(),
        "claimed_at should be cleared after unclaim"
    );
    assert!(
        bead.claim_deadline.is_none(),
        "claim_deadline should be cleared after unclaim"
    );
}

#[test]
fn only_current_assignee_can_close_in_progress_bead() {
    let create = create_event("claim-006");
    let claim = claim_event("claim-006", "athena", Duration::hours(1));

    let bead = Bead::from_events(&[create, claim]).unwrap();

    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));

    // Business rule check: only current assignee or operator can close
    let requesting_actor = "hermes";
    let can_close = bead.assignee.as_deref() == Some(requesting_actor)
        || requesting_actor == "operator"
        || bead.status != Status::InProgress;

    assert!(
        !can_close,
        "hermes should NOT be able to close a bead claimed by athena"
    );

    // The assignee CAN close
    let requesting_actor = "athena";
    let can_close = bead.assignee.as_deref() == Some(requesting_actor)
        || requesting_actor == "operator"
        || bead.status != Status::InProgress;

    assert!(can_close, "athena should be able to close her own bead");

    // The operator CAN close
    let requesting_actor = "operator";
    let can_close = bead.assignee.as_deref() == Some(requesting_actor)
        || requesting_actor == "operator"
        || bead.status != Status::InProgress;

    assert!(
        can_close,
        "operator should always be able to close any bead"
    );
}

#[test]
fn claim_unclaim_reclaim_cycle() {
    let create = create_event("claim-007");
    let claim1 = claim_event("claim-007", "athena", Duration::hours(1));
    let unclaim = unclaim_event("claim-007", "athena");
    let claim2 = claim_event("claim-007", "hermes", Duration::hours(2));

    let bead = Bead::from_events(&[create, claim1, unclaim, claim2]).unwrap();

    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("hermes"));
    assert!(bead.claimed_at.is_some());
    assert!(bead.claim_deadline.is_some());
}

/// Full claim rejection test via EventLog + Engine.
/// Claim validation lives in the CLI layer (cmd_claim checks index state).
/// Here we verify the complete flow: create via log, claim, detect conflict,
/// expire, re-claim — using Engine for the round-trip.
#[test]
fn eventlog_claim_rejection() {
    use beads_polis::engine::Engine;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let beads_dir = tmp.path().join(".beads");
    let engine = Engine::open(&beads_dir).unwrap();

    // 1. Create a bead via EventLog
    let snap = make_snapshot("elog-001");
    let create = Event::Create {
        ts: Utc::now(),
        actor: "setup".to_string(),
        bead: snap.clone(),
    };
    engine.log.append(&create).unwrap();
    let bead = beads_polis::bead::Bead::from_snapshot(&snap);
    engine.index.upsert_bead(&bead).unwrap();

    // 2. Claim it as "athena"
    let claim = claim_event("elog-001", "athena", Duration::hours(1));
    engine.log.append(&claim).unwrap();
    let mut bead = engine.index.query_show("elog-001").unwrap();
    bead.apply_event(&claim);
    engine.index.upsert_bead(&bead).unwrap();

    // 3. Attempt to claim as "hermes" — verify conflict detection
    let bead = engine.index.query_show("elog-001").unwrap();
    let second_actor = "hermes";
    let is_conflict = bead.status == Status::InProgress
        && bead.assignee.as_deref().map(|h| h != second_actor).unwrap_or(false)
        && bead.claim_deadline.map(|d| d > Utc::now()).unwrap_or(false);
    assert!(is_conflict, "hermes should be blocked by athena's active claim");

    // 4. Verify error contains holder name and deadline
    let holder = bead.assignee.as_deref().unwrap();
    let deadline_str = bead.claim_deadline.unwrap().to_rfc3339();
    assert_eq!(holder, "athena");
    assert!(!deadline_str.is_empty());

    // 5. Simulate deadline expiry via an update with past deadline
    let now = Utc::now();
    let mut fields = HashMap::new();
    fields.insert("claim_deadline".to_string(), serde_json::json!((now - Duration::hours(1)).to_rfc3339()));
    let expire_evt = Event::Update {
        ts: now,
        actor: "athena".to_string(),
        id: "elog-001".to_string(),
        fields,
    };
    engine.log.append(&expire_evt).unwrap();
    let mut bead = engine.index.query_show("elog-001").unwrap();
    bead.apply_event(&expire_evt);
    engine.index.upsert_bead(&bead).unwrap();

    let bead = engine.index.query_show("elog-001").unwrap();
    let deadline_expired = bead.claim_deadline.map(|d| d <= Utc::now()).unwrap_or(true);
    assert!(deadline_expired, "Deadline should now be expired");

    // 6. Claim as "hermes" — should succeed
    let reclaim = claim_event("elog-001", "hermes", Duration::hours(1));
    engine.log.append(&reclaim).unwrap();
    let mut bead = engine.index.query_show("elog-001").unwrap();
    bead.apply_event(&reclaim);
    engine.index.upsert_bead(&bead).unwrap();

    let bead = engine.index.query_show("elog-001").unwrap();
    assert_eq!(bead.assignee.as_deref(), Some("hermes"));
    assert_eq!(bead.status, Status::InProgress);
    assert!(bead.claim_deadline.unwrap() > Utc::now());
}
