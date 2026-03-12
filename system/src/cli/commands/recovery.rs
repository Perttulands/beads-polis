use crate::cli::{BackupArgs, RestoreArgs};
use crate::config::{self, ConfigPaths};
use crate::error::{BeadsError, Result};
use crate::output::OutputContext;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupFileEntry {
    logical_path: String,
    source_path: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupManifest {
    format: String,
    version: u32,
    created_at: String,
    beads_dir: String,
    db_path: String,
    jsonl_path: String,
    files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackupSource {
    logical_path: PathBuf,
    source_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceVerification {
    sqlite_integrity: String,
    jsonl_parseable: bool,
}

pub fn execute_health(cli: &config::CliOverrides, ctx: &OutputContext) -> Result<()> {
    crate::cli::commands::doctor::execute_named("health", cli, ctx)
}

pub fn execute_backup(
    args: &BackupArgs,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let paths = config::resolve_paths(&beads_dir, cli.db.as_ref())?;
    let sources = collect_backup_sources(&paths, args.include_history)?;
    let backup_dir = backup_dir_for_args(&beads_dir, args.output.as_ref());
    fs::create_dir_all(&backup_dir)?;

    let mut files = Vec::new();
    for source in sources {
        let target_path = backup_dir.join(&source.logical_path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source.source_path, &target_path)?;
        files.push(build_manifest_entry(
            &source.logical_path,
            &source.source_path,
            &target_path,
        )?);
    }

    let manifest = BackupManifest {
        format: "br-backup".to_string(),
        version: 1,
        created_at: Utc::now().to_rfc3339(),
        beads_dir: beads_dir.display().to_string(),
        db_path: paths.db_path.display().to_string(),
        jsonl_path: paths.jsonl_path.display().to_string(),
        files,
    };

    let manifest_path = backup_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    if ctx.is_json() {
        ctx.json_pretty(&json!({
            "action": "backup",
            "directory": backup_dir.display().to_string(),
            "manifest": manifest_path.display().to_string(),
            "files": manifest.files.len(),
            "include_history": args.include_history,
        }));
        return Ok(());
    }

    if ctx.is_quiet() {
        return Ok(());
    }

    println!("Created backup bundle at {}", backup_dir.display());
    println!("Manifest: {}", manifest_path.display());
    println!("Files copied: {}", manifest.files.len());
    Ok(())
}

pub fn execute_restore(
    args: &RestoreArgs,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    let backup_dir = args.source.canonicalize().map_err(|err| {
        BeadsError::Config(format!(
            "Backup directory '{}' is not accessible: {err}",
            args.source.display()
        ))
    })?;
    let manifest = load_manifest(&backup_dir)?;
    verify_manifest_files(&backup_dir, &manifest)?;

    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let paths = config::resolve_paths(&beads_dir, cli.db.as_ref())?;
    let restore_targets = restore_targets(&paths, manifest.files.as_slice())?;

    if !args.force {
        let existing: Vec<String> = restore_targets
            .iter()
            .filter(|(_, target)| target.exists())
            .map(|(_, target)| target.display().to_string())
            .collect();
        if !existing.is_empty() {
            return Err(BeadsError::Config(format!(
                "Restore would overwrite existing files. Re-run with --force: {}",
                existing.join(", ")
            )));
        }
    }

    for (entry, target_path) in &restore_targets {
        let source_path = backup_dir.join(&entry.logical_path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source_path, target_path)?;
    }

    let verification = if args.verify {
        Some(verify_workspace(&paths)?)
    } else {
        None
    };

    if ctx.is_json() {
        ctx.json_pretty(&json!({
            "action": "restore",
            "source": backup_dir.display().to_string(),
            "restored": true,
            "verified": args.verify,
            "verification": verification,
        }));
        return Ok(());
    }

    if ctx.is_quiet() {
        return Ok(());
    }

    println!("Restored backup bundle from {}", backup_dir.display());
    if let Some(result) = verification {
        println!(
            "Verification: sqlite_integrity={}, jsonl_parseable={}",
            result.sqlite_integrity, result.jsonl_parseable
        );
    }
    Ok(())
}

fn backup_dir_for_args(beads_dir: &Path, output: Option<&PathBuf>) -> PathBuf {
    output.cloned().unwrap_or_else(|| {
        beads_dir
            .join(".br_backups")
            .join(Utc::now().format("%Y%m%d_%H%M%S").to_string())
    })
}

fn collect_backup_sources(paths: &ConfigPaths, include_history: bool) -> Result<Vec<BackupSource>> {
    let mut sources = vec![
        BackupSource {
            logical_path: PathBuf::from("metadata.json"),
            source_path: paths.beads_dir.join("metadata.json"),
        },
        BackupSource {
            logical_path: PathBuf::from("config.yaml"),
            source_path: paths.beads_dir.join("config.yaml"),
        },
        BackupSource {
            logical_path: PathBuf::from("db").join(file_name_for(&paths.db_path)?),
            source_path: paths.db_path.clone(),
        },
        BackupSource {
            logical_path: PathBuf::from("jsonl").join(file_name_for(&paths.jsonl_path)?),
            source_path: paths.jsonl_path.clone(),
        },
    ];

    if include_history {
        let history_dir = paths.beads_dir.join(".br_history");
        if history_dir.exists() {
            collect_dir_sources(&history_dir, Path::new("history"), &mut sources)?;
        }
    }

    Ok(sources
        .into_iter()
        .filter(|source| source.source_path.exists())
        .collect())
}

fn collect_dir_sources(
    dir: &Path,
    logical_root: &Path,
    sources: &mut Vec<BackupSource>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let logical_path = logical_root.join(entry.file_name());
        if path.is_dir() {
            collect_dir_sources(&path, &logical_path, sources)?;
        } else if path.is_file() {
            sources.push(BackupSource {
                logical_path,
                source_path: path,
            });
        }
    }
    Ok(())
}

fn file_name_for(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BeadsError::Config(format!("Path '{}' has no file name", path.display())))
}

fn build_manifest_entry(
    logical_path: &Path,
    source_path: &Path,
    copied_path: &Path,
) -> Result<BackupFileEntry> {
    let metadata = fs::metadata(copied_path)?;
    Ok(BackupFileEntry {
        logical_path: logical_path.display().to_string(),
        source_path: source_path.display().to_string(),
        sha256: hash_file(copied_path)?,
        size_bytes: metadata.len(),
    })
}

fn load_manifest(backup_dir: &Path) -> Result<BackupManifest> {
    let manifest_path = backup_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(BeadsError::Config(format!(
            "Backup manifest not found at {}",
            manifest_path.display()
        )));
    }
    let manifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    Ok(manifest)
}

fn verify_manifest_files(backup_dir: &Path, manifest: &BackupManifest) -> Result<()> {
    for entry in &manifest.files {
        let path = backup_dir.join(&entry.logical_path);
        if !path.exists() {
            return Err(BeadsError::Config(format!(
                "Backup file missing from bundle: {}",
                path.display()
            )));
        }
        let hash = hash_file(&path)?;
        if hash != entry.sha256 {
            return Err(BeadsError::Config(format!(
                "Backup checksum mismatch for {}",
                entry.logical_path
            )));
        }
    }
    Ok(())
}

fn restore_targets(
    paths: &ConfigPaths,
    entries: &[BackupFileEntry],
) -> Result<Vec<(BackupFileEntry, PathBuf)>> {
    let mut targets = Vec::with_capacity(entries.len());
    for entry in entries {
        let logical = Path::new(&entry.logical_path);
        let target_path = match logical
            .components()
            .next()
            .and_then(|part| part.as_os_str().to_str())
        {
            Some("db") => paths.db_path.clone(),
            Some("jsonl") => paths.jsonl_path.clone(),
            Some("history") => paths
                .beads_dir
                .join(".br_history")
                .join(logical.strip_prefix("history").unwrap()),
            Some(_) | None => paths.beads_dir.join(logical),
        };
        targets.push((entry.clone(), target_path));
    }
    Ok(targets)
}

fn verify_workspace(paths: &ConfigPaths) -> Result<WorkspaceVerification> {
    let conn = rusqlite::Connection::open(&paths.db_path)?;
    let sqlite_integrity = conn
        .query_row("PRAGMA integrity_check", [], |row| {
            row.get::<_, Option<String>>(0)
        })?
        .unwrap_or_else(|| "error".to_string());
    if !sqlite_integrity.trim().eq_ignore_ascii_case("ok") {
        return Err(BeadsError::Config(format!(
            "Restored SQLite integrity check failed: {sqlite_integrity}"
        )));
    }
    let jsonl_parseable = verify_jsonl(&paths.jsonl_path)?;
    Ok(WorkspaceVerification {
        sqlite_integrity,
        jsonl_parseable,
    })
}

fn verify_jsonl(path: &Path) -> Result<bool> {
    let contents = fs::read_to_string(path)?;
    for (line_no, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if serde_json::from_str::<serde_json::Value>(trimmed).is_err() {
            return Err(BeadsError::Config(format!(
                "Restored JSONL is malformed at line {}",
                line_no + 1
            )));
        }
    }
    Ok(true)
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_paths(root: &Path) -> ConfigPaths {
        let beads_dir = root.join(".beads");
        fs::create_dir_all(beads_dir.join(".br_history")).unwrap();
        fs::write(
            beads_dir.join("metadata.json"),
            "{\"database\":\"beads.db\",\"jsonl_export\":\"issues.jsonl\"}",
        )
        .unwrap();
        fs::write(beads_dir.join("config.yaml"), "issue_prefix: pol\n").unwrap();
        fs::write(beads_dir.join("beads.db"), "sqlite-bytes").unwrap();
        fs::write(beads_dir.join("issues.jsonl"), "{\"id\":\"pol-1\"}\n").unwrap();
        fs::write(
            beads_dir
                .join(".br_history")
                .join("issues.20260312_120000.jsonl"),
            "{\"id\":\"pol-0\"}\n",
        )
        .unwrap();
        ConfigPaths::resolve(&beads_dir, None).unwrap()
    }

    #[test]
    fn collect_backup_sources_includes_history_when_requested() {
        let temp = TempDir::new().unwrap();
        let paths = sample_paths(temp.path());
        let sources = collect_backup_sources(&paths, true).unwrap();
        let logical_paths: Vec<String> = sources
            .into_iter()
            .map(|source| source.logical_path.display().to_string())
            .collect();
        assert!(logical_paths.contains(&"metadata.json".to_string()));
        assert!(logical_paths.contains(&"config.yaml".to_string()));
        assert!(logical_paths.iter().any(|path| path.starts_with("db/")));
        assert!(logical_paths.iter().any(|path| path.starts_with("jsonl/")));
        assert!(
            logical_paths
                .iter()
                .any(|path| path.starts_with("history/"))
        );
    }

    #[test]
    fn verify_manifest_files_detects_checksum_mismatch() {
        let temp = TempDir::new().unwrap();
        let bundle = temp.path();
        fs::write(bundle.join("payload.txt"), "ok").unwrap();
        let manifest = BackupManifest {
            format: "br-backup".to_string(),
            version: 1,
            created_at: Utc::now().to_rfc3339(),
            beads_dir: bundle.display().to_string(),
            db_path: "db".to_string(),
            jsonl_path: "jsonl".to_string(),
            files: vec![BackupFileEntry {
                logical_path: "payload.txt".to_string(),
                source_path: "payload.txt".to_string(),
                sha256: "deadbeef".to_string(),
                size_bytes: 2,
            }],
        };

        let err = verify_manifest_files(bundle, &manifest).unwrap_err();
        assert!(format!("{err}").contains("checksum mismatch"));
    }

    #[test]
    fn restore_targets_map_bundle_paths_back_to_workspace_paths() {
        let temp = TempDir::new().unwrap();
        let paths = sample_paths(temp.path());
        let entries = vec![
            BackupFileEntry {
                logical_path: "db/beads.db".to_string(),
                source_path: "unused".to_string(),
                sha256: "x".to_string(),
                size_bytes: 1,
            },
            BackupFileEntry {
                logical_path: "jsonl/issues.jsonl".to_string(),
                source_path: "unused".to_string(),
                sha256: "y".to_string(),
                size_bytes: 1,
            },
            BackupFileEntry {
                logical_path: "history/issues.20260312_120000.jsonl".to_string(),
                source_path: "unused".to_string(),
                sha256: "z".to_string(),
                size_bytes: 1,
            },
        ];

        let targets = restore_targets(&paths, &entries).unwrap();
        assert_eq!(targets[0].1, paths.db_path);
        assert_eq!(targets[1].1, paths.jsonl_path);
        assert!(
            targets[2]
                .1
                .ends_with(".beads/.br_history/issues.20260312_120000.jsonl")
        );
    }
}
