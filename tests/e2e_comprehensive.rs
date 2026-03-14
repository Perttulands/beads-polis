//! Comprehensive end-to-end tests exercising the actual `br` binary.
//! Covers: doctor auto-fix, bad-line resilience, descriptions, dependency
//! management, filter combinations, sync modes, operator override, error
//! edges, and multi-agent permission scenarios.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn br(tmp: &TempDir) -> Command {
    let bin = env!("CARGO_BIN_EXE_br");
    let mut cmd = Command::new(bin);
    cmd.env("POLIS_ACTOR", "e2e-test");
    cmd.env("BEADS_DIR", tmp.path().to_str().unwrap());
    cmd
}

fn br_actor(tmp: &TempDir, actor: &str) -> Command {
    let bin = env!("CARGO_BIN_EXE_br");
    let mut cmd = Command::new(bin);
    cmd.env("POLIS_ACTOR", actor);
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
    assert!(success, "command failed: stderr={stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}")
    })
}

fn create_bead(tmp: &TempDir, title: &str) -> String {
    let val = run_json(br(tmp).args(["create", title]));
    val["id"].as_str().unwrap().to_string()
}

// ===========================================================================
// Doctor: auto-fix truncated last line
// ===========================================================================

#[test]
fn e2e_doctor_fixes_truncated_last_line() {
    let tmp = TempDir::new().unwrap();

    // Create a valid bead
    let id = create_bead(&tmp, "Before truncation");

    // Append a truncated line to events.jsonl
    let events_path = tmp.path().join("events.jsonl");
    let mut content = fs::read_to_string(&events_path).unwrap();
    content.push_str("{\"op\":\"update\",\"ts\":\"2026-03-12T\n"); // truncated
    fs::write(&events_path, &content).unwrap();

    // Doctor should detect the truncated line
    let val = run_json(br(&tmp).args(["doctor"]));
    assert!(val["truncated_last_line"].as_bool().unwrap(), "should detect truncated last line");
    assert_eq!(val["jsonl_invalid_lines"], 1);

    // Doctor auto-fix should have cleaned it up
    // Run doctor again — should be clean
    let val = run_json(br(&tmp).args(["doctor"]));
    assert!(!val["truncated_last_line"].as_bool().unwrap(), "truncated line should be fixed");
    assert_eq!(val["jsonl_invalid_lines"], 0);

    // Original bead should still be intact
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Before truncation");
}

// ===========================================================================
// Bad middle lines are skipped, not fatal
// ===========================================================================

#[test]
fn e2e_bad_middle_line_skipped() {
    let tmp = TempDir::new().unwrap();

    // Create two beads
    let id1 = create_bead(&tmp, "First bead");
    let id2 = create_bead(&tmp, "Second bead");

    // Insert garbage in the middle of events.jsonl
    let events_path = tmp.path().join("events.jsonl");
    let content = fs::read_to_string(&events_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.len() >= 2);
    let mut new_content = String::new();
    new_content.push_str(lines[0]);
    new_content.push('\n');
    new_content.push_str("THIS IS GARBAGE NOT JSON\n");
    for line in &lines[1..] {
        new_content.push_str(line);
        new_content.push('\n');
    }
    fs::write(&events_path, &new_content).unwrap();

    // Delete index to force rebuild from the corrupt-ish JSONL
    let _ = fs::remove_file(tmp.path().join("index.db"));
    let _ = fs::remove_file(tmp.path().join("index.watermark"));

    // Both beads should still be accessible (bad line skipped)
    let val = run_json(br(&tmp).args(["show", &id1]));
    assert_eq!(val["title"], "First bead");

    let val = run_json(br(&tmp).args(["show", &id2]));
    assert_eq!(val["title"], "Second bead");

    // Doctor should report 1 invalid line
    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_invalid_lines"], 1);
}

// ===========================================================================
// Description field
// ===========================================================================

#[test]
fn e2e_description_create_and_search() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args([
        "create", "Auth timeout",
        "--description", "The relay auth token expires after 30s causing reconnect storms",
    ]));
    let id = val["id"].as_str().unwrap().to_string();

    // Show includes description
    let val = run_json(br(&tmp).args(["show", &id]));
    assert!(val["description"].as_str().unwrap().contains("reconnect storms"));

    // Search matches description text
    let val = run_json(br(&tmp).args(["search", "reconnect storms"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);
}

// ===========================================================================
// Update: add and remove dependencies
// ===========================================================================

#[test]
fn e2e_update_add_remove_deps() {
    let tmp = TempDir::new().unwrap();

    let dep1 = create_bead(&tmp, "Dep 1");
    let dep2 = create_bead(&tmp, "Dep 2");
    let dep3 = create_bead(&tmp, "Dep 3");
    let main = create_bead(&tmp, "Main task");

    // Add two deps
    run_json(br(&tmp).args(["update", &main, "--add-dep", &dep1, "--add-dep", &dep2]));
    let val = run_json(br(&tmp).args(["show", &main]));
    let deps: Vec<&str> = val["dependencies"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(deps.contains(&dep1.as_str()));
    assert!(deps.contains(&dep2.as_str()));

    // Remove one, add another
    run_json(br(&tmp).args(["update", &main, "--rm-dep", &dep1, "--add-dep", &dep3]));
    let val = run_json(br(&tmp).args(["show", &main]));
    let deps: Vec<&str> = val["dependencies"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(!deps.contains(&dep1.as_str()), "dep1 should be removed");
    assert!(deps.contains(&dep2.as_str()), "dep2 should remain");
    assert!(deps.contains(&dep3.as_str()), "dep3 should be added");
}

// ===========================================================================
// Ready: project filter
// ===========================================================================

#[test]
fn e2e_ready_project_filter() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "Gate task", "--project", "gate", "-p", "1"]));
    run_json(br(&tmp).args(["create", "Relay task", "--project", "relay", "-p", "1"]));
    run_json(br(&tmp).args(["create", "Another gate", "--project", "gate", "-p", "2"]));

    // Ready with project filter
    let val = run_json(br(&tmp).args(["ready", "--project", "gate"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should return only gate beads");
    for b in arr {
        assert_eq!(b["project"], "gate");
    }
}

// ===========================================================================
// List: combined filters
// ===========================================================================

#[test]
fn e2e_list_combined_filters() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "P0 bug", "-p", "0", "-t", "bug", "--project", "gate"]));
    run_json(br(&tmp).args(["create", "P2 task", "-p", "2", "-t", "task", "--project", "gate"]));
    run_json(br(&tmp).args(["create", "P0 feature", "-p", "0", "-t", "feature", "--project", "relay"]));

    // Filter by project + priority
    let val = run_json(br(&tmp).args(["list", "--project", "gate", "--priority", "0"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["title"], "P0 bug");

    // Filter by type
    let val = run_json(br(&tmp).args(["list", "-t", "bug"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["title"], "P0 bug");
}

// ===========================================================================
// Sync: import-only rebuilds index
// ===========================================================================

#[test]
fn e2e_sync_import_only() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Sync test bead");

    // Delete index
    let _ = fs::remove_file(tmp.path().join("index.db"));
    let _ = fs::remove_file(tmp.path().join("index.watermark"));

    // Sync import-only
    let val = run_json(br(&tmp).args(["sync", "--import-only"]));
    assert_eq!(val["action"], "import");
    assert_eq!(val["events"], 1);

    // Bead is accessible
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Sync test bead");
}

// ===========================================================================
// Sync: snapshot (compact alias)
// ===========================================================================

#[test]
fn e2e_sync_snapshot() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Snapshot test");
    for i in 0..3 {
        run_json(br(&tmp).args(["update", &id, "--title", &format!("v{i}")]));
    }

    let val = run_json(br(&tmp).args(["sync", "--snapshot"]));
    assert_eq!(val["action"], "snapshot");
    assert_eq!(val["beads"], 1);

    // Data preserved
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "v2");
}

// ===========================================================================
// Operator override: can close another agent's claimed bead
// ===========================================================================

#[test]
fn e2e_operator_can_close_claimed_bead() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Agent's task");

    // Agent claims it
    run_json(br_actor(&tmp, "agent-alpha").args(["--json", "claim", &id, "--lock-for", "1h"]));

    // Normal actor cannot close
    let (_, stderr, success) = run(br_actor(&tmp, "agent-beta").args(["--json", "close", &id, "--reason", "nope"]));
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "permission_denied");

    // Operator CAN close
    let val = run_json(br_actor(&tmp, "operator").args(["--json", "close", &id, "--reason", "override"]));
    assert_eq!(val["status"], "closed");
}

// ===========================================================================
// Operator override: can unclaim and heartbeat
// ===========================================================================

#[test]
fn e2e_operator_can_unclaim_and_heartbeat() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Operator target");

    // Agent claims
    run_json(br_actor(&tmp, "agent-x").args(["--json", "claim", &id, "--lock-for", "1h"]));

    // Operator heartbeat
    let val = run_json(br_actor(&tmp, "operator").args(["--json", "heartbeat", &id]));
    assert!(val["claim_deadline"].as_str().is_some());

    // Operator unclaim
    let val = run_json(br_actor(&tmp, "operator").args(["--json", "unclaim", &id]));
    assert_eq!(val["status"], "open");
}

// ===========================================================================
// Heartbeat by wrong actor should fail
// ===========================================================================

#[test]
fn e2e_heartbeat_wrong_actor() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Guarded bead");
    run_json(br_actor(&tmp, "owner").args(["--json", "claim", &id, "--lock-for", "1h"]));

    let (_, stderr, success) = run(br_actor(&tmp, "intruder").args(["--json", "heartbeat", &id]));
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "permission_denied");
}

// ===========================================================================
// Unclaim by wrong actor should fail
// ===========================================================================

#[test]
fn e2e_unclaim_wrong_actor() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Locked bead");
    run_json(br_actor(&tmp, "holder").args(["--json", "claim", &id, "--lock-for", "1h"]));

    let (_, stderr, success) = run(br_actor(&tmp, "thief").args(["--json", "unclaim", &id]));
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "permission_denied");
}

// ===========================================================================
// Close already-closed bead should fail
// ===========================================================================

#[test]
fn e2e_close_already_closed() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Will close");
    run_json(br(&tmp).args(["close", &id, "--reason", "done"]));

    // Second close should error
    let (_, stderr, success) = run(br(&tmp).args(["--json", "close", &id, "--reason", "again"]));
    assert!(!success, "closing already-closed should fail");
    assert!(stderr.contains("already closed"), "error should mention already closed: {stderr}");
}

// ===========================================================================
// Update non-existent bead should fail
// ===========================================================================

#[test]
fn e2e_update_nonexistent() {
    let tmp = TempDir::new().unwrap();

    let (_, stderr, success) = run(br(&tmp).args(["--json", "update", "pol-ghost", "--title", "nope"]));
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "not_found");
}

// ===========================================================================
// Claim non-existent bead should fail
// ===========================================================================

#[test]
fn e2e_claim_nonexistent() {
    let tmp = TempDir::new().unwrap();

    let (_, stderr, success) = run(br(&tmp).args(["--json", "claim", "pol-nope", "--lock-for", "1h"]));
    assert!(!success);
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "not_found");
}

// ===========================================================================
// Heartbeat on unclaimed bead should fail
// ===========================================================================

#[test]
fn e2e_heartbeat_unclaimed_bead() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Not claimed");

    let (_, stderr, success) = run(br(&tmp).args(["--json", "heartbeat", &id]));
    assert!(!success, "heartbeat on unclaimed should fail");
    assert!(stderr.contains("not claimed") || stderr.contains("error"), "stderr: {stderr}");
}

// ===========================================================================
// Claim → close → verify events.jsonl has all events
// ===========================================================================

#[test]
fn e2e_full_lifecycle_events_integrity() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["create", "Lifecycle test", "-p", "1", "-t", "bug", "--project", "gate"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Update
    run_json(br(&tmp).args(["update", &id, "--priority", "0"]));

    // Claim
    run_json(br(&tmp).args(["claim", &id, "--lock-for", "2h"]));

    // Heartbeat
    run_json(br(&tmp).args(["heartbeat", &id]));

    // Close
    run_json(br(&tmp).args(["close", &id, "--reason", "shipped"]));

    // Doctor shows correct event count (create + update + claim + heartbeat + close = 5)
    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_lines"], 5, "should have 5 events");
    assert_eq!(val["jsonl_valid_lines"], 5);
    assert_eq!(val["jsonl_invalid_lines"], 0);
    assert_eq!(val["sqlite_integrity"], "ok");

    // Compact and verify data survives
    let val = run_json(br(&tmp).args(["compact"]));
    assert_eq!(val["new_lines"], 1, "compact should produce 1 snapshot");

    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "closed");
    assert_eq!(val["priority"], 0);
    assert_eq!(val["close_reason"], "shipped");
}

// ===========================================================================
// List by assignee filter
// ===========================================================================

#[test]
fn e2e_list_by_assignee() {
    let tmp = TempDir::new().unwrap();

    let id1 = create_bead(&tmp, "Task for alpha");
    let _id2 = create_bead(&tmp, "Task for beta");
    let id3 = create_bead(&tmp, "Another for alpha");

    // Assign via update
    run_json(br(&tmp).args(["update", &id1, "--assignee", "alpha"]));
    run_json(br(&tmp).args(["update", &id3, "--assignee", "alpha"]));

    let val = run_json(br(&tmp).args(["list", "--assignee", "alpha"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    for b in arr {
        assert_eq!(b["assignee"], "alpha");
    }
}

// ===========================================================================
// Multi-dep chain: A blocks B blocks C, close A, then B becomes ready,
// close B, then C becomes ready
// ===========================================================================

#[test]
fn e2e_dependency_chain_unblock() {
    let tmp = TempDir::new().unwrap();

    let a = create_bead(&tmp, "Task A");
    let val = run_json(br(&tmp).args(["create", "Task B", "--dep", &a]));
    let b = val["id"].as_str().unwrap().to_string();
    let val = run_json(br(&tmp).args(["create", "Task C", "--dep", &b]));
    let c = val["id"].as_str().unwrap().to_string();

    // Only A should be ready
    let val = run_json(br(&tmp).args(["ready"]));
    let ids: Vec<&str> = val.as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&a.as_str()));
    assert!(!ids.contains(&b.as_str()));
    assert!(!ids.contains(&c.as_str()));

    // Close A → B becomes ready
    run_json(br(&tmp).args(["close", &a, "--reason", "done"]));
    let val = run_json(br(&tmp).args(["ready"]));
    let ids: Vec<&str> = val.as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&b.as_str()), "B should be ready after A closed");
    assert!(!ids.contains(&c.as_str()), "C still blocked by B");

    // Close B → C becomes ready
    run_json(br(&tmp).args(["close", &b, "--reason", "done"]));
    let val = run_json(br(&tmp).args(["ready"]));
    let ids: Vec<&str> = val.as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&c.as_str()), "C should be ready after B closed");
}

// ===========================================================================
// Bead types: create each type and verify
// ===========================================================================

#[test]
fn e2e_all_bead_types() {
    let tmp = TempDir::new().unwrap();

    for bead_type in ["epic", "feature", "bug", "task", "chore"] {
        let val = run_json(br(&tmp).args(["create", &format!("A {bead_type}"), "-t", bead_type]));
        let id = val["id"].as_str().unwrap().to_string();
        let val = run_json(br(&tmp).args(["show", &id]));
        assert_eq!(val["bead_type"], bead_type, "type should be {bead_type}");
    }

    // List by type
    let val = run_json(br(&tmp).args(["list", "-t", "bug"]));
    assert_eq!(val.as_array().unwrap().len(), 1);
}

// ===========================================================================
// Status transitions: open → deferred → open → in_progress → closed
// ===========================================================================

#[test]
fn e2e_status_transitions() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Status test");

    // open → deferred
    run_json(br(&tmp).args(["update", &id, "--status", "deferred"]));
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "deferred");

    // deferred should not appear in ready
    let val = run_json(br(&tmp).args(["ready"]));
    assert_eq!(val.as_array().unwrap().len(), 0);

    // deferred → open
    run_json(br(&tmp).args(["update", &id, "--status", "open"]));
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "open");

    // list by status=deferred should be empty now
    let val = run_json(br(&tmp).args(["list", "--status", "deferred"]));
    assert_eq!(val.as_array().unwrap().len(), 0);

    // list by status=open should have it
    let val = run_json(br(&tmp).args(["list", "--status", "open"]));
    assert_eq!(val.as_array().unwrap().len(), 1);
}

// ===========================================================================
// --actor flag overrides POLIS_ACTOR env
// ===========================================================================

#[test]
fn e2e_actor_flag_override() {
    let tmp = TempDir::new().unwrap();

    // Create with --actor flag overriding env
    let val = run_json(br(&tmp).args(["--actor", "custom-agent", "create", "Flag override test"]));
    let id = val["id"].as_str().unwrap().to_string();

    // Verify the actor is in the event log
    let events_path = tmp.path().join("events.jsonl");
    let content = fs::read_to_string(events_path).unwrap();
    assert!(content.contains("custom-agent"), "event should record custom-agent as actor");

    // Verify bead was created
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["title"], "Flag override test");
}

// ===========================================================================
// Index rebuild after corruption: delete index.db, operations still work
// ===========================================================================

#[test]
fn e2e_operations_after_index_delete() {
    let tmp = TempDir::new().unwrap();

    let id1 = create_bead(&tmp, "Bead one");
    let id2 = create_bead(&tmp, "Bead two");

    // Delete index files
    let _ = fs::remove_file(tmp.path().join("index.db"));
    let _ = fs::remove_file(tmp.path().join("index.watermark"));

    // All operations should still work (auto-rebuild)
    let val = run_json(br(&tmp).args(["show", &id1]));
    assert_eq!(val["title"], "Bead one");

    let val = run_json(br(&tmp).args(["list"]));
    assert_eq!(val.as_array().unwrap().len(), 2);

    // Can still create new beads
    let val = run_json(br(&tmp).args(["create", "Bead three"]));
    assert!(val["id"].as_str().is_some());

    // Total should be 3
    let val = run_json(br(&tmp).args(["list"]));
    assert_eq!(val.as_array().unwrap().len(), 3);

    // Close works too
    run_json(br(&tmp).args(["close", &id2, "--reason", "done"]));
    let val = run_json(br(&tmp).args(["show", &id2]));
    assert_eq!(val["status"], "closed");
}

// ===========================================================================
// Priority ordering: ready returns P0 before P4
// ===========================================================================

#[test]
fn e2e_ready_priority_ordering() {
    let tmp = TempDir::new().unwrap();

    run_json(br(&tmp).args(["create", "Backlog", "-p", "4"]));
    run_json(br(&tmp).args(["create", "Critical", "-p", "0"]));
    run_json(br(&tmp).args(["create", "Medium", "-p", "2"]));

    let val = run_json(br(&tmp).args(["ready"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    // Should be sorted by priority ascending
    let priorities: Vec<u64> = arr.iter().map(|b| b["priority"].as_u64().unwrap()).collect();
    assert!(priorities[0] <= priorities[1] && priorities[1] <= priorities[2],
        "ready should be sorted by priority: {priorities:?}");
}

// ===========================================================================
// Search: no results returns empty array
// ===========================================================================

#[test]
fn e2e_search_no_results() {
    let tmp = TempDir::new().unwrap();

    create_bead(&tmp, "Hello world");

    let val = run_json(br(&tmp).args(["search", "zzz_nonexistent_xyz"]));
    let arr = val.as_array().unwrap();
    assert_eq!(arr.len(), 0, "search with no matches should return empty array");
}

// ===========================================================================
// Sequential claim conflict: first agent claims, second is rejected
// ===========================================================================

#[test]
fn e2e_sequential_claim_conflict() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Race target");

    // First agent claims
    run_json(br_actor(&tmp, "racer-0").args(["--json", "claim", &id, "--lock-for", "1h"]));

    // Second agent tries — should fail because first holds active claim
    let (_, stderr, success) = run(
        br_actor(&tmp, "racer-1").args(["--json", "claim", &id, "--lock-for", "1h"])
    );
    assert!(!success, "second claim should fail");
    let err: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap_or_default();
    assert_eq!(err["error"], "already_claimed");
    assert_eq!(err["holder"], "racer-0");

    // Bead should be in_progress with racer-0
    let val = run_json(br(&tmp).args(["show", &id]));
    assert_eq!(val["status"], "in_progress");
    assert_eq!(val["assignee"], "racer-0");
}

// ===========================================================================
// Multiple labels and filter preservation through compact
// ===========================================================================

#[test]
fn e2e_labels_survive_compact() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args([
        "create", "Labeled thing",
        "-l", "networking", "-l", "p0", "-l", "gate",
        "--project", "gate", "-p", "0", "-t", "bug",
    ]));
    let id = val["id"].as_str().unwrap().to_string();

    // Update to generate more events
    run_json(br(&tmp).args(["update", &id, "--title", "Updated labeled thing"]));

    // Compact
    run_json(br(&tmp).args(["compact"]));

    // Labels survived compaction
    let val = run_json(br(&tmp).args(["show", &id]));
    let labels: Vec<&str> = val["labels"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str()).collect();
    assert!(labels.contains(&"networking"));
    assert!(labels.contains(&"p0"));
    assert!(labels.contains(&"gate"));
    assert_eq!(val["project"], "gate");
    assert_eq!(val["priority"], 0);
    assert_eq!(val["bead_type"], "bug");
    assert_eq!(val["title"], "Updated labeled thing");
}

// ===========================================================================
// Empty database: list, ready, search all return empty arrays
// ===========================================================================

#[test]
fn e2e_empty_database() {
    let tmp = TempDir::new().unwrap();

    let val = run_json(br(&tmp).args(["list"]));
    assert_eq!(val.as_array().unwrap().len(), 0);

    let val = run_json(br(&tmp).args(["ready"]));
    assert_eq!(val.as_array().unwrap().len(), 0);

    let val = run_json(br(&tmp).args(["search", "anything"]));
    assert_eq!(val.as_array().unwrap().len(), 0);
}

// ===========================================================================
// Doctor on empty database reports healthy
// ===========================================================================

#[test]
fn e2e_doctor_empty_db() {
    let tmp = TempDir::new().unwrap();

    // Need to initialize the dir first
    let _ = fs::create_dir_all(tmp.path());
    let events = tmp.path().join("events.jsonl");
    fs::write(&events, "").unwrap();

    let val = run_json(br(&tmp).args(["doctor"]));
    assert_eq!(val["jsonl_lines"], 0);
    assert_eq!(val["jsonl_valid_lines"], 0);
    assert_eq!(val["jsonl_invalid_lines"], 0);
    assert!(!val["truncated_last_line"].as_bool().unwrap());
}

// ===========================================================================
// Claim sets correct deadline and timestamps
// ===========================================================================

#[test]
fn e2e_claim_deadline_correctness() {
    let tmp = TempDir::new().unwrap();

    let id = create_bead(&tmp, "Deadline check");

    let val = run_json(br(&tmp).args(["claim", &id, "--lock-for", "30m"]));
    let deadline = val["claim_deadline"].as_str().unwrap();

    // Parse the deadline — should be ~30 minutes from now
    let deadline_dt = chrono::DateTime::parse_from_rfc3339(deadline).unwrap();
    let now = chrono::Utc::now();
    let diff = deadline_dt.signed_duration_since(now);
    // Should be between 25 and 35 minutes (allowing for test execution time)
    assert!(diff.num_minutes() >= 25 && diff.num_minutes() <= 35,
        "deadline should be ~30m from now, got {diff}");
}
