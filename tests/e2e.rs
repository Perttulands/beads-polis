//! End-to-end tests that exercise the actual `br` binary.
//! These test real CLI usage — the same commands a user or agent runs.

use std::process::Command;
use tempfile::TempDir;

fn br(tmp: &TempDir) -> Command {
    let bin = env!("CARGO_BIN_EXE_br");
    let mut cmd = Command::new(bin);
    cmd.env("POLIS_ACTOR", "e2e-test");
    cmd.env("BEADS_DIR", tmp.path().to_str().unwrap());
    cmd
}

fn run(cmd: &mut Command) -> (String, String, bool) {
    let out = cmd.output().expect("failed to execute br");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (stdout, stderr, out.status.success())
}

fn run_json(cmd: &mut Command) -> serde_json::Value {
    let (stdout, stderr, success) = run(cmd.arg("--json"));
    assert!(success, "command failed: stderr={}", stderr);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}")
    })
}

// ---------------------------------------------------------------------------
// CRUD lifecycle
// ---------------------------------------------------------------------------

#[test]
fn e2e_create_show_list_close() {
    let tmp = TempDir::new().unwrap();

    // Create
    let val = run_json(br(&tmp).args(["create", "Fix the bug", "-p", "1", "-t", "bug", "--project", "gate"]));
    let id = val["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("pol-"), "id should start with pol-: {id}");
    assert_eq!(val["title"], "Fix the bug");

    // Show
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["id"], id);
    assert_eq!(val["title"], "Fix the bug");
    assert_eq!(val["priority"], 1);
    assert_eq!(val["bead_type"], "bug");
    assert_eq!(val["project"], "gate");
    assert_eq!(val["status"], "open");

    // List (should have 1 bead)
    let val = run_json(br(&tmp).args(["list"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);

    // Close
    let val = run_json(br(&tmp).args(["close", &id, "--reason", "fixed in commit abc"]));
    assert_eq!(val["id"], id);
    assert_eq!(val["status"], "closed");

    // Show after close
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "closed");
    assert_eq!(val["close_reason"], "fixed in commit abc");

    // List open should be empty
    let val = run_json(br(&tmp).args(["list", "--status", "open"]));
    assert_eq!(val.as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

#[test]
fn e2e_update_fields() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Original title", "-p", "3"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Update multiple fields
    let val = run_json(br(&tmp).args([
        "update", &id,
        "--title", "Updated title",
        "--priority", "0",
        "--project", "relay",
        "--status", "deferred",
    ]));
    assert_eq!(val["title"], "Updated title");
    assert_eq!(val["priority"], 0);
    assert_eq!(val["project"], "relay");
    assert_eq!(val["status"], "deferred");

    // Show confirms
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Updated title");
    assert_eq!(val["priority"], 0);
}

// ---------------------------------------------------------------------------
// Dependencies and ready
// ---------------------------------------------------------------------------

#[test]
fn e2e_deps_block_ready() {
    let tmp = TempDir::new().unwrap();

    // Create blocker
    let val = run_json(br(&tmp).args(["create", "Blocker", "-p", "1"]));
    let blocker_id = val["id"].as_str().unwrap().to_string();

    // Create blocked bead
    let val = run_json(br(&tmp).args(["create", "Blocked", "-p", "1", "--dep", &blocker_id]));
    let blocked_id = val["id"].as_str().unwrap().to_string();

    // Ready should only show blocker (blocked bead is blocked)
    let val = run_json(br(&tmp).args(["ready"]));
    let arr = val.as_array().unwrap();
    let ids: Vec<&str> = arr.iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&blocker_id.as_str()), "blocker should be ready");
    assert!(!ids.contains(&blocked_id.as_str()), "blocked bead should NOT be ready");

    // Close blocker
    run_json(br(&tmp).args(["close", &blocker_id, "--reason", "done"]));

    // Now blocked bead should be ready
    let val = run_json(br(&tmp).args(["ready"]));
    let arr = val.as_array().unwrap();
    let ids: Vec<&str> = arr.iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&blocked_id.as_str()), "blocked bead should be ready after dep closed");
}

// ---------------------------------------------------------------------------
// Claim / heartbeat / unclaim
// ---------------------------------------------------------------------------

#[test]
fn e2e_claim_heartbeat_unclaim_cycle() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Claimable task", "-p", "2"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Claim
    let val = run_json(br(&tmp).args(["claim", &id, "--lock-for", "2h"]));
    assert_eq!(val["id"], id);
    assert_eq!(val["assignee"], "e2e-test");
    assert!(val["claim_deadline"].as_str().is_some());

    // Show confirms in_progress
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "in_progress");
    assert_eq!(val["assignee"], "e2e-test");

    // Heartbeat
    let val = run_json(br(&tmp).args(["heartbeat", &id]));
    assert!(val["claim_deadline"].as_str().is_some());

    // Unclaim
    let val = run_json(br(&tmp).args(["unclaim", &id]));
    assert_eq!(val["status"], "open");

    // Show confirms open again
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "open");
    assert!(val["assignee"].is_null());
}

// ---------------------------------------------------------------------------
// Claim conflict: different actor
// ---------------------------------------------------------------------------

#[test]
fn e2e_claim_conflict() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Contested task", "-p", "1"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Actor A claims
    run_json(br(&tmp).args(["claim", &id, "--lock-for", "1h"]));

    // Actor B tries to claim — should fail
    let mut cmd = br(&tmp);
    cmd.env("POLIS_ACTOR", "other-agent");
    cmd.args(["--json", "claim", &id, "--lock-for", "1h"]);
    let (stdout, stderr, success) = run(&mut cmd);
    assert!(!success, "should fail: stdout={stdout} stderr={stderr}");

    // Parse error
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "already_claimed");
    assert_eq!(err["holder"], "e2e-test");
}

// ---------------------------------------------------------------------------
// Permission: non-assignee can't close in_progress
// ---------------------------------------------------------------------------

#[test]
fn e2e_close_permission_denied() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Protected task"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Claim by e2e-test
    run_json(br(&tmp).args(["claim", &id, "--lock-for", "1h"]));

    // Different actor tries to close — should fail
    let mut cmd = br(&tmp);
    cmd.env("POLIS_ACTOR", "intruder");
    cmd.args(["--json", "close", &id, "--reason", "stealing"]);
    let (_stdout, stderr, success) = run(&mut cmd);
    assert!(!success, "non-assignee should not be able to close");

    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "permission_denied");
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[test]
fn e2e_search() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "Fix relay timeout", "-p", "1"]));
    run_json(br(&tmp).args(["create", "Add CLI help text", "-p", "2"]));
    run_json(br(&tmp).args(["create", "Relay reconnect logic", "-p", "1"]));

    // Search for "relay"
    let val = run_json(br(&tmp).args(["search", "relay"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should find 2 beads matching 'relay'");
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

#[test]
fn e2e_doctor_healthy() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "Test bead"]));

    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_lines"], 1);
    assert_eq!(val["jsonl_valid_lines"], 1);
    assert_eq!(val["jsonl_invalid_lines"], 0);
    assert_eq!(val["sqlite_integrity"], "ok");
    assert!(!val["index_watermark_stale"].as_bool().unwrap());
    assert!(!val["truncated_last_line"].as_bool().unwrap());
}

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

#[test]
fn e2e_rebuild() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Rebuild test"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Delete index
    let _ = std::fs::remove_file(tmp.path().join("index.db"));

    // Rebuild
    let val = run_json(br(&tmp).args(["rebuild"]));
    assert_eq!(val["action"], "rebuild");

    // Show still works
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Rebuild test");
}

// ---------------------------------------------------------------------------
// Compact
// ---------------------------------------------------------------------------

#[test]
fn e2e_compact() {
    let tmp = TempDir::new().unwrap();

    // Create a bead and do several updates to generate multiple events
    let val = run_json(br(&tmp).args(["create", "Compact test"]));
    let id = val["id"].as_str().unwrap().to_string();

    for i in 0..5 {
        run_json(br(&tmp).args(["update", &id, "--title", &format!("Title v{}", i)]));
    }

    // Should have 6 events (1 create + 5 updates)
    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_lines"], 6);

    // Compact
    let val = run_json(br(&tmp).args(["compact"]));
    assert_eq!(val["action"], "compact");
    assert_eq!(val["beads"], 1);
    assert_eq!(val["old_lines"], 6);
    assert_eq!(val["new_lines"], 1);

    // Data preserved
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Title v4"); // Last update wins
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

#[test]
fn e2e_labels() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args([
        "create", "Labeled task", "-l", "networking", "-l", "urgent",
    ]));
    let id = val["id"].as_str().unwrap().to_string();

    let val = run_json(br(&tmp).args(["show", &id]));
    let labels: Vec<&str> = val["labels"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(labels.contains(&"networking"));
    assert!(labels.contains(&"urgent"));
}

// ---------------------------------------------------------------------------
// Human output (non-JSON)
// ---------------------------------------------------------------------------

#[test]
fn e2e_human_output() {
    let tmp = TempDir::new().unwrap();

    // Create (human output)
    let (stdout, _, success) = run(br(&tmp).args(["create", "Human readable"]));
    assert!(success);
    assert!(stdout.contains("created pol-"), "expected 'created pol-' in: {stdout}");
    assert!(stdout.contains("Human readable"), "expected title in: {stdout}");

    // List (human output)
    let (stdout, _, success) = run(br(&tmp).args(["list"]));
    assert!(success);
    assert!(stdout.contains("Human readable"));
    assert!(stdout.contains("○")); // status icon for open
}

// ---------------------------------------------------------------------------
// Error: no actor
// ---------------------------------------------------------------------------

#[test]
fn e2e_no_actor_error() {
    let tmp = TempDir::new().unwrap();
    let bin = env!("CARGO_BIN_EXE_br");
    let mut cmd = Command::new(bin);
    cmd.env("BEADS_DIR", tmp.path().to_str().unwrap());
    cmd.env_remove("POLIS_ACTOR");
    cmd.args(["--json", "create", "Should fail"]);
    let (_stdout, stderr, success) = run(&mut cmd);
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "no_actor");
}

// ---------------------------------------------------------------------------
// Error: show non-existent bead
// ---------------------------------------------------------------------------

#[test]
fn e2e_not_found() {
    let tmp = TempDir::new().unwrap();
    let mut cmd = br(&tmp);
    cmd.args(["--json", "show", "pol-does-not-exist"]);
    let (_stdout, stderr, success) = run(&mut cmd);
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "not_found");
}

// ---------------------------------------------------------------------------
// Sync --migrate from legacy format
// ---------------------------------------------------------------------------

#[test]
fn e2e_migrate_legacy() {
    let tmp = TempDir::new().unwrap();

    // Write a legacy issues.jsonl
    let legacy = r#"{"id":"pol-test1","title":"Legacy bead","status":"open","priority":1,"issue_type":"task","created_at":"2026-03-01T10:00:00Z","created_by":"test","updated_at":"2026-03-01T10:00:00Z","source_repo":".","compaction_level":0,"original_size":0}
{"id":"pol-test2","title":"Legacy closed","status":"closed","priority":2,"issue_type":"bug","created_at":"2026-03-01T10:00:00Z","created_by":"test","updated_at":"2026-03-02T10:00:00Z","closed_at":"2026-03-02T10:00:00Z","close_reason":"fixed","source_repo":".","compaction_level":0,"original_size":0}"#;
    std::fs::write(tmp.path().join("issues.jsonl"), legacy).unwrap();

    // Migrate
    let val = run_json(br(&tmp).args(["sync", "--migrate"]));
    assert_eq!(val["action"], "migrate");
    assert_eq!(val["migrated"], 2);
    assert_eq!(val["skipped"], 0);

    // Verify data
    let val = run_json(br(&tmp).args(["show", "pol-test1"]));
    assert_eq!(val["title"], "Legacy bead");
    assert_eq!(val["status"], "open");
    assert_eq!(val["priority"], 1);

    let val = run_json(br(&tmp).args(["show", "pol-test2"]));
    assert_eq!(val["title"], "Legacy closed");
    assert_eq!(val["status"], "closed");
    assert_eq!(val["close_reason"], "fixed");

    // List open should have 1
    let val = run_json(br(&tmp).args(["list", "--status", "open"]));
    assert_eq!(val.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// City commands
// ---------------------------------------------------------------------------

#[test]
fn e2e_city_ready_and_list() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "Gate task", "--project", "gate", "-p", "1"]));
    run_json(br(&tmp).args(["create", "Relay task", "--project", "relay", "-p", "2"]));

    // City ready (all projects)
    let val = run_json(br(&tmp).args(["city", "ready"]));
    assert_eq!(val.as_array().unwrap().len(), 2);

    // City list with filter
    let val = run_json(br(&tmp).args(["city", "list", "--project", "gate"]));
    assert_eq!(val.as_array().unwrap().len(), 1);
    assert_eq!(val[0]["project"], "gate");
}

// ---------------------------------------------------------------------------
// Concurrent writes (fork two creates, verify both land)
// ---------------------------------------------------------------------------

#[test]
fn e2e_concurrent_creates() {
    let tmp = TempDir::new().unwrap();
    let bin = env!("CARGO_BIN_EXE_br");

    // First, seed the directory so concurrent processes don't race on dir creation
    let mut seed = Command::new(bin);
    seed.env("POLIS_ACTOR", "seed");
    seed.env("BEADS_DIR", tmp.path().to_str().unwrap());
    seed.args(["--json", "create", "Seed bead"]);
    let out = seed.output().expect("seed failed");
    assert!(out.status.success(), "seed create failed");

    // Now spawn 10 creates concurrently
    let mut handles = Vec::new();
    for i in 0..10 {
        let mut cmd = Command::new(bin);
        cmd.env("POLIS_ACTOR", format!("agent-{i}"));
        cmd.env("BEADS_DIR", tmp.path().to_str().unwrap());
        cmd.args(["--json", "create", &format!("Concurrent bead {i}")]);
        handles.push(cmd.spawn().expect("failed to spawn"));
    }

    // Wait for all
    for mut h in handles {
        let status = h.wait().expect("failed to wait");
        assert!(status.success(), "create failed");
    }

    // Verify all 11 landed (1 seed + 10 concurrent)
    let val = run_json(br(&tmp).args(["list"]));
    assert_eq!(val.as_array().unwrap().len(), 11, "1 seed + 10 concurrent creates should land");

    // Doctor should be healthy
    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_valid_lines"], 11);
    assert_eq!(val["jsonl_invalid_lines"], 0);
    assert_eq!(val["sqlite_integrity"], "ok");
}
