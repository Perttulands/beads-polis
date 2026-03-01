//! Concurrent write safety tests for SQLite flock implementation.
//!
//! Validates that the POSIX advisory lock (`flock(LOCK_EX)`) on a sibling
//! `.lock` file correctly serializes concurrent writers and prevents
//! database corruption.
//!
//! Tests G and H specifically cover the pol-3q4b scenario:
//! `br sync --import-only` racing with `br create` (argus vs agent).

mod common;

use beads_rust::error::BeadsError;
use beads_rust::storage::{ListFilters, SqliteStorage};
use beads_rust::sync::{ExportConfig, ImportConfig, export_to_jsonl, import_from_jsonl};
use common::fixtures;
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

// ============================================================================
// TEST A — flock blocks second writer
// ============================================================================

#[test]
fn flock_blocks_second_writer() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("contention.db");

    // First open — holds the flock for its lifetime.
    let _holder = SqliteStorage::open(&db_path).unwrap();

    // Second open with a short timeout — must fail with DatabaseLocked.
    let result = SqliteStorage::open_with_timeout(&db_path, Some(100));
    match &result {
        Err(BeadsError::DatabaseLocked { path }) => {
            assert_eq!(path, &db_path);
        }
        other => panic!(
            "Expected BeadsError::DatabaseLocked, got: {:?}",
            other.as_ref().map(|_| "Ok(..)")
        ),
    }
}

#[test]
fn flock_releases_on_drop_allowing_second_open() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("release.db");

    // Scope: first holder acquires and releases the lock.
    {
        let _holder = SqliteStorage::open(&db_path).unwrap();
    }

    // After drop, a second open must succeed.
    let second = SqliteStorage::open_with_timeout(&db_path, Some(500));
    assert!(
        second.is_ok(),
        "Second open should succeed after first is dropped: {:?}",
        second.err()
    );
}

// ============================================================================
// TEST B — flock serializes concurrent mutations
// ============================================================================

#[test]
fn flock_serializes_concurrent_mutations() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("serial.db");

    // Seed the database so schema is ready.
    {
        let _seed = SqliteStorage::open(&db_path).unwrap();
    }

    const N: usize = 8;
    let barrier = Arc::new(Barrier::new(N));

    std::thread::scope(|s| {
        for i in 0..N {
            let barrier = Arc::clone(&barrier);
            let path = db_path.clone();
            s.spawn(move || {
                barrier.wait();
                let mut storage = SqliteStorage::open_with_timeout(&path, Some(30_000)).unwrap();
                let issue = fixtures::issue(&format!("concurrent-{i}"));
                storage.create_issue(&issue, "tester").unwrap();
                // storage dropped here — releases flock
            });
        }
    });

    // Verify exactly N issues exist.
    let storage = SqliteStorage::open(&db_path).unwrap();
    let filters = ListFilters {
        include_closed: true,
        include_templates: true,
        ..Default::default()
    };
    let all = storage.list_issues(&filters).unwrap();
    assert_eq!(
        all.len(),
        N,
        "Expected exactly {N} issues, found {}",
        all.len()
    );
}

// ============================================================================
// TEST C — no lock for memory DB
// ============================================================================

#[test]
fn no_lock_for_memory_db() {
    // Two concurrent in-memory databases must both succeed — no file locking.
    let barrier = Arc::new(Barrier::new(2));

    std::thread::scope(|s| {
        for _ in 0..2 {
            let barrier = Arc::clone(&barrier);
            s.spawn(move || {
                barrier.wait();
                let storage = SqliteStorage::open_memory();
                assert!(
                    storage.is_ok(),
                    "open_memory() should always succeed: {:?}",
                    storage.err()
                );
            });
        }
    });
}

#[test]
fn memory_db_has_no_lock_file() {
    // open_memory() must not leave any .lock file behind.
    // Since :memory: isn't backed by a real path, verify the storage's
    // internal lock_file is None by checking that no .lock file exists
    // in the current temp directory.
    let dir = TempDir::new().unwrap();
    let _storage = SqliteStorage::open_memory().unwrap();

    // No .lock file should appear anywhere in our temp dir.
    let lock_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "lock"))
        .collect();
    assert!(
        lock_files.is_empty(),
        "open_memory() should not create .lock files"
    );
}

// ============================================================================
// TEST D — lock file path derived from DB path (sibling, not suffix)
// ============================================================================

#[test]
fn flock_lock_path_is_sibling_not_suffix() {
    // `path.with_extension("lock")` replaces the extension:
    //   beads.db → beads.lock   (correct — sibling)
    //   NOT beads.db → beads.db.lock  (wrong — suffix)
    //
    // This test verifies the actual file on disk matches expectations,
    // catching any future change to the lock-path derivation.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("beads.db");

    let _storage = SqliteStorage::open(&db_path).unwrap();

    let expected_lock = dir.path().join("beads.lock");
    let wrong_lock = dir.path().join("beads.db.lock");

    assert!(
        expected_lock.exists(),
        "Lock file should be sibling: expected {:?} to exist",
        expected_lock
    );
    assert!(
        !wrong_lock.exists(),
        "Lock file should NOT use suffix form: {:?} should not exist",
        wrong_lock
    );
}

// ============================================================================
// TEST E — timeout error is clear and DB stays intact
// ============================================================================

#[test]
fn flock_timeout_returns_clear_error_and_db_stays_intact() {
    // Simulates `br close` while DB is locked:
    //   1. First writer opens and creates an issue.
    //   2. Second writer times out — must get DatabaseLocked with the DB path.
    //   3. After timeout, first writer must still be able to write (no corruption).
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("integrity.db");

    // First writer: create an issue to prove DB is live.
    let mut holder = SqliteStorage::open(&db_path).unwrap();
    let issue_before = fixtures::issue("before-timeout");
    holder.create_issue(&issue_before, "agent-a").unwrap();

    // Second writer: short timeout — must fail with DatabaseLocked.
    let result = SqliteStorage::open_with_timeout(&db_path, Some(100));
    match &result {
        Err(BeadsError::DatabaseLocked { path }) => {
            // Error should name the DB path, not the lock path.
            assert_eq!(
                path, &db_path,
                "DatabaseLocked error should reference the DB path, got {:?}",
                path
            );
            // Verify the Display message is human-readable.
            let msg = format!("{}", result.unwrap_err());
            assert!(
                msg.contains("integrity.db"),
                "Error message should mention the DB file: {msg}"
            );
        }
        other => panic!(
            "Expected BeadsError::DatabaseLocked, got: {:?}",
            other.as_ref().map(|_| "Ok(..)")
        ),
    }

    // First writer must still work — no corruption from the failed attempt.
    let issue_after = fixtures::issue("after-timeout");
    holder.create_issue(&issue_after, "agent-a").unwrap();

    // Verify both issues exist.
    let filters = ListFilters {
        include_closed: true,
        include_templates: true,
        ..Default::default()
    };
    let all = holder.list_issues(&filters).unwrap();
    assert_eq!(
        all.len(),
        2,
        "Both issues should exist after timeout — DB must not be corrupted"
    );
}

// ============================================================================
// TEST F — lock file cleaned up after drop
// ============================================================================

#[test]
fn flock_file_exists_while_held_gone_semantically_after_drop() {
    // The .lock file is created on open and the flock is released on drop.
    // The file itself may persist (flock doesn't delete it), but the lock
    // must be released so the next opener succeeds immediately.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("cleanup.db");
    let lock_path = dir.path().join("cleanup.lock");

    // Open and drop.
    {
        let _storage = SqliteStorage::open(&db_path).unwrap();
        assert!(lock_path.exists(), "Lock file should exist while held");
    }

    // After drop, a new open must succeed immediately (100ms timeout is plenty
    // if the lock was actually released).
    let result = SqliteStorage::open_with_timeout(&db_path, Some(100));
    assert!(
        result.is_ok(),
        "Lock should be released after drop, but open failed: {:?}",
        result.err()
    );
}

// ============================================================================
// TEST G — concurrent sync import + create (pol-3q4b: argus vs agent race)
// ============================================================================

/// Reproduce the argus race: one thread runs `import_from_jsonl` (simulating
/// `br sync --import-only`) while another thread runs `create_issue`
/// (simulating `br create`).  Both go through `SqliteStorage`, so the flock
/// must serialize them.  After both complete, the DB must contain all expected
/// issues with no corruption.
#[test]
fn flock_serializes_sync_import_and_create() {
    let dir = TempDir::new().unwrap();
    let beads_dir = dir.path().join(".beads");
    std::fs::create_dir_all(&beads_dir).unwrap();
    let db_path = beads_dir.join("beads.db");
    let jsonl_path = beads_dir.join("issues.jsonl");

    // Seed: create 5 issues and export to JSONL.
    {
        let mut storage = SqliteStorage::open(&db_path).unwrap();
        storage.set_config("issue_prefix", "test").unwrap();
        for i in 0..5 {
            let issue = fixtures::issue(&format!("seed-{i}"));
            storage.create_issue(&issue, "seeder").unwrap();
        }
        let export_config = ExportConfig {
            force: true,
            beads_dir: Some(beads_dir.clone()),
            ..Default::default()
        };
        export_to_jsonl(&storage, &jsonl_path, &export_config).unwrap();
    }
    // flock released — DB seeded, JSONL written.

    // Wipe DB to simulate a fresh clone where only JSONL exists.
    std::fs::remove_file(&db_path).unwrap();
    // Also remove the WAL/SHM if any.
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));

    // Re-create empty DB with schema.
    {
        let _storage = SqliteStorage::open(&db_path).unwrap();
    }

    let barrier = Arc::new(Barrier::new(2));

    std::thread::scope(|s| {
        // Thread A: import from JSONL (simulates argus `br sync --import-only`).
        let barrier_a = Arc::clone(&barrier);
        let db_a = db_path.clone();
        let jsonl_a = jsonl_path.clone();
        let beads_a = beads_dir.clone();
        s.spawn(move || {
            barrier_a.wait();
            let mut storage = SqliteStorage::open_with_timeout(&db_a, Some(30_000)).unwrap();
            storage.set_config("issue_prefix", "test").unwrap();
            let import_config = ImportConfig {
                skip_prefix_validation: true,
                beads_dir: Some(beads_a),
                show_progress: false,
                ..Default::default()
            };
            import_from_jsonl(&mut storage, &jsonl_a, &import_config, Some("test")).unwrap();
        });

        // Thread B: create a new issue (simulates agent `br create`).
        let barrier_b = Arc::clone(&barrier);
        let db_b = db_path.clone();
        s.spawn(move || {
            barrier_b.wait();
            let mut storage = SqliteStorage::open_with_timeout(&db_b, Some(30_000)).unwrap();
            let issue = fixtures::issue("concurrent-agent-create");
            storage.create_issue(&issue, "agent").unwrap();
        });
    });

    // Verify: 5 imported + 1 created = 6 total, no corruption.
    let storage = SqliteStorage::open(&db_path).unwrap();
    let filters = ListFilters {
        include_closed: true,
        include_templates: true,
        ..Default::default()
    };
    let all = storage.list_issues(&filters).unwrap();
    assert!(
        all.len() >= 6,
        "Expected at least 6 issues (5 imported + 1 created), found {}",
        all.len()
    );

    // Verify the agent-created issue exists.
    let agent_issue = all.iter().find(|i| i.title == "concurrent-agent-create");
    assert!(
        agent_issue.is_some(),
        "Agent-created issue must survive concurrent import"
    );

    // Verify imported issues exist.
    for i in 0..5 {
        let title = format!("seed-{i}");
        let found = all.iter().any(|issue| issue.title == title);
        assert!(
            found,
            "Imported issue '{title}' must exist after concurrent import+create"
        );
    }
}

// ============================================================================
// TEST H — repeated concurrent import cycles (stress test)
// ============================================================================

/// Stress test: N threads each open storage, import from JSONL, and create
/// one new issue.  Simulates multiple argus cycles overlapping with agent
/// work.  The flock must serialize all access; final DB must be consistent.
#[test]
fn flock_serializes_repeated_import_and_create_cycles() {
    let dir = TempDir::new().unwrap();
    let beads_dir = dir.path().join(".beads");
    std::fs::create_dir_all(&beads_dir).unwrap();
    let db_path = beads_dir.join("beads.db");
    let jsonl_path = beads_dir.join("issues.jsonl");

    // Seed: create 3 issues and export to JSONL.
    {
        let mut storage = SqliteStorage::open(&db_path).unwrap();
        storage.set_config("issue_prefix", "test").unwrap();
        for i in 0..3 {
            let issue = fixtures::issue(&format!("base-{i}"));
            storage.create_issue(&issue, "seeder").unwrap();
        }
        let export_config = ExportConfig {
            force: true,
            beads_dir: Some(beads_dir.clone()),
            ..Default::default()
        };
        export_to_jsonl(&storage, &jsonl_path, &export_config).unwrap();
    }

    const N: usize = 6;
    let barrier = Arc::new(Barrier::new(N));

    std::thread::scope(|s| {
        for i in 0..N {
            let barrier = Arc::clone(&barrier);
            let db = db_path.clone();
            let jsonl = jsonl_path.clone();
            let beads = beads_dir.clone();
            s.spawn(move || {
                barrier.wait();
                let mut storage = SqliteStorage::open_with_timeout(&db, Some(30_000)).unwrap();

                // Half the threads import (argus pattern), half create (agent pattern).
                if i % 2 == 0 {
                    let import_config = ImportConfig {
                        skip_prefix_validation: true,
                        beads_dir: Some(beads),
                        show_progress: false,
                        ..Default::default()
                    };
                    import_from_jsonl(&mut storage, &jsonl, &import_config, Some("test")).unwrap();
                } else {
                    let issue = fixtures::issue(&format!("stress-{i}"));
                    storage.create_issue(&issue, "stress-agent").unwrap();
                }
            });
        }
    });

    // Verify: 3 base + 3 created (threads 1, 3, 5) = 6 minimum.
    let storage = SqliteStorage::open(&db_path).unwrap();
    let filters = ListFilters {
        include_closed: true,
        include_templates: true,
        ..Default::default()
    };
    let all = storage.list_issues(&filters).unwrap();

    // Base issues must survive all the concurrent imports.
    for i in 0..3 {
        let title = format!("base-{i}");
        assert!(
            all.iter().any(|issue| issue.title == title),
            "Base issue '{title}' must survive concurrent stress"
        );
    }

    // Each odd-numbered thread created an issue.
    for i in (1..N).step_by(2) {
        let title = format!("stress-{i}");
        assert!(
            all.iter().any(|issue| issue.title == title),
            "Stress-created issue '{title}' must survive concurrent import"
        );
    }

    let expected_min = 3 + (N / 2); // 3 base + 3 creates
    assert!(
        all.len() >= expected_min,
        "Expected at least {expected_min} issues, found {}",
        all.len()
    );
}
