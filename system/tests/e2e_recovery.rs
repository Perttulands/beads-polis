//! End-to-end recoverability tests for `br health`, `br backup`, and `br restore --verify`.

mod common;

use common::cli::{BrWorkspace, extract_json_payload, run_br, run_br_with_env};
use serde_json::Value;
use std::fs;

fn actor_env() -> [(&'static str, &'static str); 1] {
    [("POLIS_ACTOR", "tester")]
}

fn init_workspace(workspace: &BrWorkspace) {
    let init = run_br(workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);
}

fn create_issue(workspace: &BrWorkspace, title: &str, label: &str) {
    let create = run_br_with_env(workspace, ["create", title], actor_env(), label);
    assert!(create.status.success(), "create failed: {}", create.stderr);
}

#[test]
fn e2e_health_alias_matches_doctor_checks() {
    let _log = common::test_log("e2e_health_alias_matches_doctor_checks");
    let workspace = BrWorkspace::new();
    init_workspace(&workspace);

    let doctor = run_br(&workspace, ["doctor", "--json"], "doctor_json");
    assert!(doctor.status.success(), "doctor failed: {}", doctor.stderr);
    let doctor_json: Value =
        serde_json::from_str(&extract_json_payload(&doctor.stdout)).expect("doctor json");

    let health = run_br(&workspace, ["health", "--json"], "health_json");
    assert!(health.status.success(), "health failed: {}", health.stderr);
    let health_json: Value =
        serde_json::from_str(&extract_json_payload(&health.stdout)).expect("health json");

    assert_eq!(
        doctor_json["ok"], health_json["ok"],
        "health should report the same status as doctor"
    );
    assert_eq!(
        doctor_json["checks"], health_json["checks"],
        "health should reuse doctor checks"
    );
}

#[test]
fn e2e_backup_and_restore_verify_round_trip() {
    let _log = common::test_log("e2e_backup_and_restore_verify_round_trip");
    let workspace = BrWorkspace::new();
    init_workspace(&workspace);
    create_issue(&workspace, "Original issue", "create_original");

    let backup_dir = workspace.root.join("backup-bundle");
    let backup = run_br(
        &workspace,
        [
            "backup",
            "--output",
            backup_dir.to_str().expect("backup dir utf-8"),
        ],
        "backup",
    );
    assert!(backup.status.success(), "backup failed: {}", backup.stderr);

    let manifest_path = backup_dir.join("manifest.json");
    assert!(manifest_path.exists(), "manifest should exist");
    let manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read manifest")).unwrap();
    assert_eq!(manifest["format"], "br-backup");
    assert!(
        manifest["files"]
            .as_array()
            .is_some_and(|files| !files.is_empty()),
        "backup manifest should list copied files"
    );

    create_issue(&workspace, "Mutated issue", "create_mutated");
    let mutated_jsonl = workspace.root.join(".beads").join("issues.jsonl");
    let mutated_before_restore = fs::read_to_string(&mutated_jsonl).expect("read mutated jsonl");
    assert!(
        mutated_before_restore.contains("Mutated issue"),
        "workspace should differ before restore"
    );

    let restore = run_br(
        &workspace,
        [
            "restore",
            backup_dir.to_str().expect("backup dir utf-8"),
            "--force",
            "--verify",
        ],
        "restore_verify",
    );
    assert!(
        restore.status.success(),
        "restore failed: {}",
        restore.stderr
    );

    let restored_jsonl = fs::read_to_string(&mutated_jsonl).expect("read restored jsonl");
    let bundled_jsonl = fs::read_to_string(backup_dir.join("jsonl").join("issues.jsonl"))
        .expect("read bundled jsonl");
    assert_eq!(
        restored_jsonl, bundled_jsonl,
        "restore should copy the bundled JSONL back into the workspace"
    );
    assert!(
        !restored_jsonl.contains("Mutated issue"),
        "restored workspace should roll back post-backup changes"
    );

    let health = run_br(&workspace, ["health", "--json"], "health_after_restore");
    assert!(health.status.success(), "health failed: {}", health.stderr);
    let health_json: Value =
        serde_json::from_str(&extract_json_payload(&health.stdout)).expect("health json");
    assert_eq!(health_json["ok"], Value::Bool(true));
}

#[test]
fn e2e_restore_verify_rejects_tampered_backup() {
    let _log = common::test_log("e2e_restore_verify_rejects_tampered_backup");
    let workspace = BrWorkspace::new();
    init_workspace(&workspace);
    create_issue(&workspace, "Original issue", "create_original");

    let backup_dir = workspace.root.join("tampered-backup");
    let backup = run_br(
        &workspace,
        [
            "backup",
            "--output",
            backup_dir.to_str().expect("backup dir utf-8"),
        ],
        "backup_tampered",
    );
    assert!(backup.status.success(), "backup failed: {}", backup.stderr);

    let bundled_jsonl = backup_dir.join("jsonl").join("issues.jsonl");
    fs::write(&bundled_jsonl, "{\"tampered\":true}\n").expect("tamper backup");

    let restore = run_br(
        &workspace,
        [
            "restore",
            backup_dir.to_str().expect("backup dir utf-8"),
            "--force",
            "--verify",
        ],
        "restore_tampered",
    );
    assert!(
        !restore.status.success(),
        "restore should fail when the bundle has been tampered with"
    );
    assert!(
        restore.stderr.contains("checksum mismatch")
            || restore.stderr.contains("Backup checksum mismatch"),
        "restore should report checksum verification failure: {}",
        restore.stderr
    );
}
