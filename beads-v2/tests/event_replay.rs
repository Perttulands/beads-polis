//! Unit tests for event sourcing replay logic.
//!
//! Tests that the Bead::from_events / Bead::apply_event functions
//! produce correct state given various event sequences.

use beads_v2::bead::{Bead, BeadType, Status};
use beads_v2::event::{BeadSnapshot, Event};
use chrono::{Duration, Utc};
use std::collections::HashMap;

/// Helper: build a minimal BeadSnapshot for testing.
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

/// Helper: create a Create event.
fn create_event(id: &str, title: &str) -> Event {
    Event::Create {
        ts: Utc::now(),
        actor: "test-actor".to_string(),
        bead: test_snapshot(id, title),
    }
}

/// Helper: create an Update event with arbitrary fields.
fn update_event(id: &str, fields: HashMap<String, serde_json::Value>) -> Event {
    Event::Update {
        ts: Utc::now(),
        actor: "test-actor".to_string(),
        id: id.to_string(),
        fields,
    }
}

/// Helper: create a Close event.
fn close_event(id: &str, reason: &str) -> Event {
    Event::Close {
        ts: Utc::now(),
        actor: "test-actor".to_string(),
        id: id.to_string(),
        reason: reason.to_string(),
    }
}

/// Helper: create a Reopen event.
fn reopen_event(id: &str) -> Event {
    Event::Reopen {
        ts: Utc::now(),
        actor: "test-actor".to_string(),
        id: id.to_string(),
    }
}

#[test]
fn create_event_produces_correct_bead_state() {
    let snap = test_snapshot("test-001", "Fix the widget");
    let event = Event::Create {
        ts: snap.created_at,
        actor: "athena".to_string(),
        bead: snap.clone(),
    };

    let bead = Bead::from_events(&[event]).unwrap();

    assert_eq!(bead.id, "test-001");
    assert_eq!(bead.title, "Fix the widget");
    assert_eq!(bead.status, Status::Open);
    assert_eq!(bead.priority, 2);
    assert_eq!(bead.bead_type, BeadType::Task);
    assert_eq!(bead.project, "test");
    assert!(bead.assignee.is_none());
    assert!(bead.closed_at.is_none());
    assert!(bead.close_reason.is_none());
}

#[test]
fn update_event_modifies_only_specified_fields() {
    let create = create_event("test-002", "Original title");
    let mut fields = HashMap::new();
    fields.insert("title".to_string(), serde_json::json!("Updated title"));
    fields.insert("priority".to_string(), serde_json::json!(0));
    let update = update_event("test-002", fields);

    let bead = Bead::from_events(&[create, update]).unwrap();

    assert_eq!(bead.title, "Updated title");
    assert_eq!(bead.priority, 0);
    // Unmodified fields remain unchanged
    assert_eq!(bead.status, Status::Open);
    assert_eq!(bead.project, "test");
    assert_eq!(bead.bead_type, BeadType::Task);
    assert!(bead.description.is_none());
}

#[test]
fn close_event_sets_status_close_reason_closed_at() {
    let create = create_event("test-003", "Close me");
    let close = close_event("test-003", "Done in commit abc123");

    let bead = Bead::from_events(&[create, close]).unwrap();

    assert_eq!(bead.status, Status::Closed);
    assert_eq!(bead.close_reason.as_deref(), Some("Done in commit abc123"));
    assert!(bead.closed_at.is_some());
}

#[test]
fn reopen_after_close_works() {
    let create = create_event("test-004", "Reopen me");
    let close = close_event("test-004", "Thought it was done");
    let reopen = reopen_event("test-004");

    let bead = Bead::from_events(&[create, close, reopen]).unwrap();

    assert_eq!(bead.status, Status::Open);
    assert!(bead.closed_at.is_none());
    assert!(bead.close_reason.is_none());
}

#[test]
fn out_of_order_events_by_timestamp_still_produce_correct_state() {
    // Events are applied in array order (log order), not timestamp order.
    // Even if timestamps are out of order, replay is deterministic by position.
    let now = Utc::now();
    let earlier = now - Duration::hours(2);
    let later = now + Duration::hours(2);

    let create = Event::Create {
        ts: later, // "future" timestamp on create
        actor: "actor".to_string(),
        bead: test_snapshot("test-005", "Out of order"),
    };

    let mut fields = HashMap::new();
    fields.insert("title".to_string(), serde_json::json!("Updated"));
    let update = Event::Update {
        ts: earlier, // "past" timestamp on update
        actor: "actor".to_string(),
        id: "test-005".to_string(),
        fields,
    };

    // Despite the update having an earlier timestamp, it is applied after create
    // because it appears later in the event array (log order).
    let bead = Bead::from_events(&[create, update]).unwrap();

    assert_eq!(bead.title, "Updated");
    // updated_at reflects the event timestamp, even if it's "earlier"
    assert_eq!(bead.updated_at, earlier);
}

#[test]
fn duplicate_events_are_idempotent() {
    let create = create_event("test-006", "Duplicated");
    let close1 = close_event("test-006", "Reason A");
    let close2 = close_event("test-006", "Reason A");

    // Applying the same close twice should not panic and should end in same state
    let bead = Bead::from_events(&[create, close1.clone(), close2]).unwrap();

    assert_eq!(bead.status, Status::Closed);
    assert_eq!(bead.close_reason.as_deref(), Some("Reason A"));
}

#[test]
fn truncated_last_json_line_is_detected_and_discarded() {
    // Simulate reading a JSONL file where the last line is truncated.
    // This tests the parsing logic directly — not EventLog (which doesn't exist yet).
    let create = create_event("test-007", "Truncation test");
    let create_json = serde_json::to_string(&create).unwrap();

    let lines = vec![
        create_json.clone(),
        // Truncated line: valid JSON prefix but incomplete
        r#"{"op":"update","ts":"2026-03-12T18:00:00Z","actor":"at"#.to_string(),
    ];

    let mut events = Vec::new();
    let mut truncated_count = 0;
    for (i, line) in lines.iter().enumerate() {
        match serde_json::from_str::<Event>(line) {
            Ok(ev) => events.push(ev),
            Err(_) => {
                // Only the last line should be treated as truncated
                if i == lines.len() - 1 {
                    truncated_count += 1;
                }
            }
        }
    }

    assert_eq!(events.len(), 1);
    assert_eq!(truncated_count, 1);

    let bead = Bead::from_events(&events).unwrap();
    assert_eq!(bead.id, "test-007");
}

#[test]
fn empty_events_file_produces_empty_state() {
    let events: Vec<Event> = vec![];
    let result = Bead::from_events(&events);
    assert!(result.is_none(), "Empty events should produce None");
}

#[test]
fn from_events_returns_none_for_non_create_first_event() {
    // If the first event is an Update (no prior Create), from_events returns None.
    let mut fields = HashMap::new();
    fields.insert("title".to_string(), serde_json::json!("Orphan update"));
    let update = update_event("orphan-001", fields);

    let result = Bead::from_events(&[update]);
    assert!(result.is_none());
}

#[test]
fn update_assignee_and_claim_fields() {
    let create = create_event("test-008", "Claim test");
    let now = Utc::now();
    let deadline = now + Duration::hours(1);
    let mut fields = HashMap::new();
    fields.insert("status".to_string(), serde_json::json!("in_progress"));
    fields.insert("assignee".to_string(), serde_json::json!("athena"));
    fields.insert(
        "claimed_at".to_string(),
        serde_json::json!(now.to_rfc3339()),
    );
    fields.insert(
        "claim_deadline".to_string(),
        serde_json::json!(deadline.to_rfc3339()),
    );
    let update = update_event("test-008", fields);

    let bead = Bead::from_events(&[create, update]).unwrap();

    assert_eq!(bead.status, Status::InProgress);
    assert_eq!(bead.assignee.as_deref(), Some("athena"));
    assert!(bead.claimed_at.is_some());
    assert!(bead.claim_deadline.is_some());
}

#[test]
fn multiple_updates_accumulate() {
    let create = create_event("test-009", "Multi-update");

    let mut f1 = HashMap::new();
    f1.insert("priority".to_string(), serde_json::json!(1));
    let u1 = update_event("test-009", f1);

    let mut f2 = HashMap::new();
    f2.insert("title".to_string(), serde_json::json!("Multi-update v2"));
    f2.insert(
        "labels".to_string(),
        serde_json::json!(["urgent", "backend"]),
    );
    let u2 = update_event("test-009", f2);

    let bead = Bead::from_events(&[create, u1, u2]).unwrap();

    assert_eq!(bead.priority, 1); // from first update
    assert_eq!(bead.title, "Multi-update v2"); // from second update
    assert_eq!(bead.labels, vec!["urgent", "backend"]); // from second update
}

#[test]
fn serialization_roundtrip() {
    let create = create_event("rt-001", "Roundtrip test");
    let json = serde_json::to_string(&create).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id(), "rt-001");
    assert_eq!(create.ts(), parsed.ts());
}

/// Replay a full lifecycle: create -> update -> close -> reopen -> close
#[test]
fn full_lifecycle_replay() {
    let create = create_event("life-001", "Full lifecycle");

    let mut fields = HashMap::new();
    fields.insert("priority".to_string(), serde_json::json!(0));
    fields.insert("assignee".to_string(), serde_json::json!("hermes"));
    let update = update_event("life-001", fields);

    let close1 = close_event("life-001", "First attempt");
    let reopen = reopen_event("life-001");
    let close2 = close_event("life-001", "Actually done this time");

    let bead = Bead::from_events(&[create, update, close1, reopen, close2]).unwrap();

    assert_eq!(bead.status, Status::Closed);
    assert_eq!(
        bead.close_reason.as_deref(),
        Some("Actually done this time")
    );
    assert_eq!(bead.priority, 0); // preserved from update
    assert_eq!(bead.assignee.as_deref(), Some("hermes")); // preserved from update
    assert!(bead.closed_at.is_some());
}
