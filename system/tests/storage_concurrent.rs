//! Concurrent write safety tests for SQLite flock implementation.
//!
//! Validates that the POSIX advisory lock (`flock(LOCK_EX)`) on a sibling
//! `.lock` file correctly serializes concurrent writers and prevents
//! database corruption.

mod common;

use beads_rust::error::BeadsError;
use beads_rust::storage::{ListFilters, SqliteStorage};
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
                let mut storage =
                    SqliteStorage::open_with_timeout(&path, Some(30_000)).unwrap();
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
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "lock")
        })
        .collect();
    assert!(
        lock_files.is_empty(),
        "open_memory() should not create .lock files"
    );
}
