//! Tests for br city ready|list cross-project aggregation.

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

fn run_json(cmd: &mut Command) -> serde_json::Value {
    let (stdout, stderr, success) = run(cmd.arg("--json"));
    assert!(success, "command failed: stderr={}", stderr);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("invalid JSON: {e}\nstdout: {stdout}\nstderr: {stderr}")
    })
}

fn write_config(beads_dir: &std::path::Path, projects: &[(&str, &std::path::Path)]) {
    let mut yaml = String::from("issue_prefix: pol\nprojects:\n");
    for (name, path) in projects {
        yaml.push_str(&format!("  {}: {}\n", name, path.display()));
    }
    std::fs::write(beads_dir.join("config.yaml"), yaml).unwrap();
}

/// Create beads in a separate project dir, return the project dir (parent of .beads).
fn create_project_beads(tmp: &TempDir, project_name: &str, titles: &[&str]) -> std::path::PathBuf {
    let project_dir = tmp.path().join(project_name);
    let beads_dir = project_dir.join(".beads");
    std::fs::create_dir_all(&beads_dir).unwrap();

    for title in titles {
        run_json(br(&beads_dir).args(["create", title, "--project", project_name]));
    }
    project_dir
}

#[test]
fn city_ready_returns_beads_from_all_project_dbs() {
    let tmp = TempDir::new().unwrap();
    let local_beads = tmp.path().join("local");

    // Create local beads
    run_json(br(&local_beads).args(["create", "Local task one", "-p", "1"]));

    // Create external project with beads
    let relay_dir = create_project_beads(&tmp, "relay", &["Relay feature alpha"]);

    // Configure external projects
    write_config(&local_beads, &[("relay", &relay_dir)]);

    // City ready should return beads from both local and external
    let val = run_json(br(&local_beads).args(["city", "ready"]));
    let arr = val.as_array().expect("expected array");
    assert!(arr.len() >= 2, "expected at least 2 beads, got {}", arr.len());

    // Verify project annotation on external beads
    let projects: Vec<&str> = arr.iter()
        .filter_map(|b| b["project"].as_str())
        .collect();
    assert!(projects.contains(&"relay"), "expected relay project in results: {:?}", projects);
}

#[test]
fn city_list_returns_beads_from_all_project_dbs() {
    let tmp = TempDir::new().unwrap();
    let local_beads = tmp.path().join("local");

    run_json(br(&local_beads).args(["create", "Local bead", "--project", "main"]));

    let ext_dir = create_project_beads(&tmp, "forge", &["Forge bead one", "Forge bead two"]);
    write_config(&local_beads, &[("forge", &ext_dir)]);

    let val = run_json(br(&local_beads).args(["city", "list"]));
    let arr = val.as_array().expect("expected array");
    assert!(arr.len() >= 3, "expected at least 3 beads, got {}", arr.len());
}

#[test]
fn city_deduplication_works() {
    let tmp = TempDir::new().unwrap();
    let local_beads = tmp.path().join("local");

    // Create a bead locally with known project
    run_json(br(&local_beads).args(["create", "Shared bead", "--project", "shared"]));

    // Point config to the same beads dir (self-reference) — should deduplicate
    write_config(&local_beads, &[("self", tmp.path().join("local").as_ref())]);

    let val = run_json(br(&local_beads).args(["city", "ready"]));
    let arr = val.as_array().expect("expected array");
    // IDs should be unique (no duplicates from self-reference)
    let ids: Vec<&str> = arr.iter().filter_map(|b| b["id"].as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "duplicates found: {:?}", ids);
}

#[test]
fn city_annotates_project_field_on_each_result() {
    let tmp = TempDir::new().unwrap();
    let local_beads = tmp.path().join("local");

    let alpha_dir = create_project_beads(&tmp, "alpha", &["Alpha task"]);
    let beta_dir = create_project_beads(&tmp, "beta", &["Beta task"]);

    // Empty local, two external
    std::fs::create_dir_all(&local_beads).unwrap();
    std::fs::write(local_beads.join("events.jsonl"), "").unwrap();
    write_config(&local_beads, &[("alpha", &alpha_dir), ("beta", &beta_dir)]);

    let val = run_json(br(&local_beads).args(["city", "ready"]));
    let arr = val.as_array().expect("expected array");
    assert_eq!(arr.len(), 2);

    let projects: Vec<&str> = arr.iter()
        .filter_map(|b| b["project"].as_str())
        .collect();
    assert!(projects.contains(&"alpha"), "missing alpha: {:?}", projects);
    assert!(projects.contains(&"beta"), "missing beta: {:?}", projects);
}

#[test]
fn city_ready_sorts_by_priority() {
    let tmp = TempDir::new().unwrap();
    let local_beads = tmp.path().join("local");

    // Create beads with different priorities
    run_json(br(&local_beads).args(["create", "Low priority task", "-p", "3", "--project", "main"]));

    let ext_dir = tmp.path().join("ext");
    let ext_beads = ext_dir.join(".beads");
    std::fs::create_dir_all(&ext_beads).unwrap();
    run_json(br(&ext_beads).args(["create", "High priority task", "-p", "1", "--project", "ext"]));

    write_config(&local_beads, &[("ext", &ext_dir)]);

    let val = run_json(br(&local_beads).args(["city", "ready"]));
    let arr = val.as_array().expect("expected array");
    assert!(arr.len() >= 2);

    // First bead should be higher priority (lower number)
    let p0 = arr[0]["priority"].as_u64().unwrap();
    let p1 = arr[1]["priority"].as_u64().unwrap();
    assert!(p0 <= p1, "expected sorted by priority: {} <= {}", p0, p1);
}
