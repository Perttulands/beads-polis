//! Tests for br lint bead quality gate.

use std::process::Command;
use tempfile::TempDir;

fn br(beads_dir: &std::path::Path) -> Command {
    let bin = env!("CARGO_BIN_EXE_br");
    let mut cmd = Command::new(bin);
    cmd.env("POLIS_ACTOR", "test-agent");
    cmd.env("BEADS_DIR", beads_dir.to_str().unwrap());
    cmd
}

fn run(cmd: &mut Command) -> (String, String, bool) {
    let out = cmd.output().expect("failed to execute br");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (stdout, stderr, out.status.success())
}

fn create_bead(beads_dir: &std::path::Path, title: &str, desc: Option<&str>) -> String {
    let mut cmd = br(beads_dir);
    cmd.args(["--json", "create", title]);
    if let Some(d) = desc {
        cmd.args(["--description", d]);
    }
    let out = cmd.output().expect("failed to run br create");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).expect("bad json");
    val["id"].as_str().expect("no id").to_string()
}

#[test]
fn lint_title_too_short_fails() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "Fix bug", Some("This is a proper description with done condition. Done when tests pass."));

    // Title "Fix bug" is 7 chars < 10 — should fail lint
    let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
    assert!(!success, "lint should fail for short title");
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["passed"], false);
    let errors = val["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("title")),
        "expected title error in {:?}", errors);
}

#[test]
fn lint_body_missing_fails() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    // Create bead with no description
    let id = create_bead(bd, "A sufficient title here", None);

    let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
    assert!(!success, "lint should fail when body is missing");
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["passed"], false);
    let errors = val["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("description") || e.as_str().unwrap().contains("body")),
        "expected body/description error in {:?}", errors);
}

#[test]
fn lint_no_done_condition_keywords_fails() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "A sufficient title for this bead", Some("This is a description that does not contain any done-condition keywords at all."));

    let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
    assert!(!success, "lint should fail when no done-condition keywords");
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["passed"], false);
    let errors = val["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e.as_str().unwrap().contains("done")),
        "expected done-condition error in {:?}", errors);
}

#[test]
fn lint_valid_bead_passes() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "Implement user authentication flow",
        Some("Add OAuth2 login for the web frontend. Done when all auth tests pass and login flow works end-to-end."));

    let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
    assert!(success, "lint should pass for valid bead, got: {}", stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["passed"], true);
    let errors = val["errors"].as_array().unwrap();
    assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
}

#[test]
fn lint_exit_code_1_on_failure() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "Bad", None);

    let out = br(bd).args(["lint", &id]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "expected exit code 1 on lint failure");
}

#[test]
fn lint_exit_code_0_on_success() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "Implement proper bead validation pipeline",
        Some("Validate bead quality before dispatch. Done when lint checks pass for title, body, and done-condition."));

    let out = br(bd).args(["lint", &id]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "expected exit code 0 on lint success");
}

#[test]
fn lint_json_output_format() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "Short", None);

    let (stdout, _stderr, _) = run(br(bd).args(["--json", "lint", &id]));
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(val.get("passed").is_some(), "missing 'passed' field");
    assert!(val.get("errors").is_some(), "missing 'errors' field");
    assert!(val["errors"].is_array(), "'errors' should be array");
}

#[test]
fn lint_body_too_short_fails() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();
    let id = create_bead(bd, "A proper title for testing", Some("Too short"));

    let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
    assert!(!success, "lint should fail for body < 20 chars");
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["passed"], false);
}

#[test]
fn lint_done_condition_keyword_variants() {
    let tmp = TempDir::new().unwrap();
    let bd = tmp.path();

    // Test various done-condition keywords
    for keyword in &["done when", "success:", "test:", "passes", "done:", "complete", "implemented", "fixed"] {
        let title = format!("Bead testing keyword {}", keyword);
        let desc = format!("This is a sufficient description. {} the requirements are met.", keyword);
        let id = create_bead(bd, &title, Some(&desc));

        let (stdout, _stderr, success) = run(br(bd).args(["--json", "lint", &id]));
        assert!(success, "lint should pass for keyword '{}', got: {}", keyword, stdout);
    }
}
