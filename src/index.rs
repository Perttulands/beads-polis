//! SQLite index — derived, disposable, rebuilt from JSONL when stale.

use crate::bead::{Bead, BeadType, Status};
use crate::event::Event;
use crate::log::EventLog;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SCHEMA: &str = r"
    CREATE TABLE IF NOT EXISTS beads (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        description TEXT DEFAULT '',
        status TEXT NOT NULL DEFAULT 'open',
        priority INTEGER NOT NULL DEFAULT 2,
        bead_type TEXT NOT NULL DEFAULT 'task',
        project TEXT NOT NULL DEFAULT '',
        assignee TEXT,
        parent TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        closed_at TEXT,
        close_reason TEXT,
        claimed_at TEXT,
        claim_deadline TEXT,
        last_heartbeat TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_beads_status ON beads(status);
    CREATE INDEX IF NOT EXISTS idx_beads_priority ON beads(priority);
    CREATE INDEX IF NOT EXISTS idx_beads_project ON beads(project);
    CREATE INDEX IF NOT EXISTS idx_beads_ready
        ON beads(status, priority, created_at)
        WHERE status IN ('open', 'in_progress');

    CREATE TABLE IF NOT EXISTS dependencies (
        bead_id TEXT NOT NULL,
        depends_on TEXT NOT NULL,
        PRIMARY KEY (bead_id, depends_on)
    );
    CREATE INDEX IF NOT EXISTS idx_deps_bead ON dependencies(bead_id);
    CREATE INDEX IF NOT EXISTS idx_deps_target ON dependencies(depends_on);

    CREATE TABLE IF NOT EXISTS labels (
        bead_id TEXT NOT NULL,
        label TEXT NOT NULL,
        PRIMARY KEY (bead_id, label)
    );
    CREATE INDEX IF NOT EXISTS idx_labels_bead ON labels(bead_id);
    CREATE INDEX IF NOT EXISTS idx_labels_label ON labels(label);

    CREATE TABLE IF NOT EXISTS blocked_cache (
        bead_id TEXT PRIMARY KEY,
        blocked_by TEXT NOT NULL  -- JSON array of blocking bead IDs
    );
";

/// Query filters for list operations.
#[derive(Debug, Default)]
pub struct Filters {
    pub project: Option<String>,
    pub status: Option<Status>,
    pub priority: Option<u8>,
    pub bead_type: Option<BeadType>,
    pub label: Option<String>,
    pub assignee: Option<String>,
}

pub struct Index {
    conn: Connection,
    db_path: PathBuf,
}

impl Index {
    /// Open the index, rebuilding from JSONL if stale or missing or corrupt.
    pub fn open_or_rebuild(db_path: &Path, log: &EventLog) -> io::Result<Self> {
        // Check if DB exists and is healthy
        let needs_rebuild = if db_path.exists() {
            match Connection::open(db_path) {
                Ok(conn) => {
                    let integrity_ok = conn
                        .query_row("PRAGMA integrity_check", [], |row| {
                            row.get::<_, String>(0)
                        })
                        .map(|r| r == "ok")
                        .unwrap_or(false);
                    if !integrity_ok {
                        eprintln!("beads: index failed integrity check, rebuilding");
                        true
                    } else {
                        // Check watermark staleness
                        log.is_stale()?
                    }
                }
                Err(_) => true,
            }
        } else {
            true
        };

        if needs_rebuild {
            // Acquire flock to serialize rebuilds across processes
            let lock_file = log.acquire_lock()?;
            // Re-check staleness under lock (another process may have rebuilt)
            let still_needs_rebuild = if db_path.exists() {
                log.is_stale().unwrap_or(true)
            } else {
                true
            };
            if still_needs_rebuild {
                let events = log.read_all()?;
                Self::rebuild(db_path, &events)?;
                // Update watermark after rebuild
                let count = log.line_count()?;
                let watermark_path = db_path.with_file_name("index.watermark");
                fs::write(&watermark_path, count.to_string())?;
            }
            drop(lock_file); // release flock
        }

        let conn = Connection::open(db_path)
            .map_err(io::Error::other)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(io::Error::other)?;

        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
        })
    }

    /// Full rebuild: replay events, write to .tmp, atomic rename.
    pub fn rebuild(db_path: &Path, events: &[Event]) -> io::Result<()> {
        let tmp_path = db_path.with_extension("db.tmp");

        // Remove stale tmp if exists
        let _ = fs::remove_file(&tmp_path);

        let conn = Connection::open(&tmp_path)
            .map_err(io::Error::other)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(io::Error::other)?;
        conn.execute_batch(SCHEMA)
            .map_err(io::Error::other)?;

        // Replay events to compute current state
        let beads = replay_all(events);

        // Insert all beads
        let tx_err = |e: rusqlite::Error| io::Error::other(e);
        conn.execute_batch("BEGIN").map_err(tx_err)?;

        for bead in &beads {
            insert_bead(&conn, bead).map_err(tx_err)?;
        }

        // Build blocked cache
        rebuild_blocked_cache(&conn, &beads).map_err(tx_err)?;

        conn.execute_batch("COMMIT").map_err(tx_err)?;
        drop(conn);

        // Atomic rename
        fs::rename(&tmp_path, db_path)?;

        // Clean up WAL/SHM from tmp if they exist
        let _ = fs::remove_file(tmp_path.with_extension("db.tmp-wal"));
        let _ = fs::remove_file(tmp_path.with_extension("db.tmp-shm"));

        Ok(())
    }

    /// Insert a single bead into an already-open index (for incremental updates).
    /// Also updates the blocked_cache for this bead.
    pub fn upsert_bead(&self, bead: &Bead) -> Result<(), rusqlite::Error> {
        insert_bead(&self.conn, bead)?;
        update_blocked_cache_single(&self.conn, bead)?;
        Ok(())
    }

    /// Query: unblocked beads sorted by priority, optionally filtered by project.
    pub fn query_ready(&self, project: Option<&str>) -> Vec<Bead> {
        let sql = if project.is_some() {
            "SELECT id FROM beads WHERE status IN ('open','in_progress') \
             AND id NOT IN (SELECT bead_id FROM blocked_cache) \
             AND project = ?1 \
             ORDER BY priority ASC, created_at ASC"
        } else {
            "SELECT id FROM beads WHERE status IN ('open','in_progress') \
             AND id NOT IN (SELECT bead_id FROM blocked_cache) \
             ORDER BY priority ASC, created_at ASC"
        };

        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let ids: Vec<String> = if let Some(p) = project {
            match stmt.query_map(params![p], |row| row.get(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => { eprintln!("query_map error: {e}"); return Vec::new(); }
            }
        } else {
            match stmt.query_map([], |row| row.get(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(e) => { eprintln!("query_map error: {e}"); return Vec::new(); }
            }
        };

        ids.iter().filter_map(|id| self.query_show(id)).collect()
    }

    /// Query: list beads with filters.
    pub fn query_list(&self, filters: &Filters) -> Vec<Bead> {
        let mut conditions = Vec::new();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(ref p) = filters.project {
            conditions.push(format!("project = ?{}", idx));
            bind_values.push(Box::new(p.clone()));
            idx += 1;
        }
        if let Some(ref s) = filters.status {
            conditions.push(format!("status = ?{}", idx));
            bind_values.push(Box::new(s.as_str().to_string()));
            idx += 1;
        }
        if let Some(p) = filters.priority {
            conditions.push(format!("priority = ?{}", idx));
            bind_values.push(Box::new(p as i32));
            idx += 1;
        }
        if let Some(ref t) = filters.bead_type {
            conditions.push(format!("bead_type = ?{}", idx));
            bind_values.push(Box::new(t.as_str().to_string()));
            idx += 1;
        }
        if let Some(ref a) = filters.assignee {
            conditions.push(format!("assignee = ?{}", idx));
            bind_values.push(Box::new(a.clone()));
            idx += 1;
        }
        if let Some(ref l) = filters.label {
            conditions.push(format!(
                "id IN (SELECT bead_id FROM labels WHERE label = ?{})",
                idx
            ));
            bind_values.push(Box::new(l.clone()));
            let _ = idx; // suppress unused warning
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id FROM beads {} ORDER BY priority ASC, created_at ASC",
            where_clause
        );

        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

        let ids: Vec<String> = match stmt.query_map(params_ref.as_slice(), |row| row.get(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => { eprintln!("query_map error: {e}"); return Vec::new(); }
        };

        ids.iter().filter_map(|id| self.query_show(id)).collect()
    }

    /// Query: get a single bead by ID.
    pub fn query_show(&self, id: &str) -> Option<Bead> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, description, status, priority, bead_type, project, \
                 assignee, parent, created_at, updated_at, closed_at, close_reason, \
                 claimed_at, claim_deadline, last_heartbeat FROM beads WHERE id = ?1",
            )
            .ok()?;

        let bead = stmt
            .query_row(params![id], |row| {
                Ok(Bead {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    status: Status::from_str_lossy(
                        &row.get::<_, String>(3).unwrap_or_default(),
                    ),
                    priority: row.get::<_, i32>(4).unwrap_or(2) as u8,
                    bead_type: BeadType::from_str_lossy(
                        &row.get::<_, String>(5).unwrap_or_default(),
                    ),
                    project: row.get(6)?,
                    assignee: row.get(7)?,
                    parent: row.get(8)?,
                    dependencies: Vec::new(), // filled below
                    labels: Vec::new(),       // filled below
                    created_at: parse_dt(&row.get::<_, String>(9).unwrap_or_default()),
                    updated_at: parse_dt(&row.get::<_, String>(10).unwrap_or_default()),
                    closed_at: row.get::<_, Option<String>>(11).ok().flatten().map(|s| parse_dt(&s)),
                    close_reason: row.get(12)?,
                    claimed_at: row.get::<_, Option<String>>(13).ok().flatten().map(|s| parse_dt(&s)),
                    claim_deadline: row.get::<_, Option<String>>(14).ok().flatten().map(|s| parse_dt(&s)),
                    last_heartbeat: row.get::<_, Option<String>>(15).ok().flatten().map(|s| parse_dt(&s)),
                })
            })
            .ok()?;

        let mut bead = bead;

        // Load dependencies
        if let Ok(mut dep_stmt) = self
            .conn
            .prepare("SELECT depends_on FROM dependencies WHERE bead_id = ?1")
        {
            bead.dependencies = match dep_stmt.query_map(params![id], |row| row.get(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            };
        }

        // Load labels
        if let Ok(mut lbl_stmt) = self
            .conn
            .prepare("SELECT label FROM labels WHERE bead_id = ?1")
        {
            bead.labels = match lbl_stmt.query_map(params![id], |row| row.get(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            };
        }

        Some(bead)
    }

    /// Query: full-text search on title and description.
    pub fn query_search(&self, query: &str) -> Vec<Bead> {
        let pattern = format!("%{}%", query);
        let sql = "SELECT id FROM beads WHERE title LIKE ?1 OR description LIKE ?1 \
                   ORDER BY priority ASC, created_at ASC";
        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let ids: Vec<String> = match stmt.query_map(params![pattern], |row| row.get(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(e) => { eprintln!("query_map error: {e}"); return Vec::new(); }
        };

        ids.iter().filter_map(|id| self.query_show(id)).collect()
    }

    /// Get the database path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

// --- Internal helpers ---

fn parse_dt(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now())
}

/// Replay all events to compute the current state of every bead.
fn replay_all(events: &[Event]) -> Vec<Bead> {
    let mut beads: HashMap<String, Bead> = HashMap::new();

    for event in events {
        let id = event.id().to_string();
        match event {
            Event::Create { bead: snap, .. } | Event::Snapshot { bead: snap, .. } => {
                beads.insert(id, Bead::from_snapshot(snap));
            }
            _ => {
                if let Some(bead) = beads.get_mut(&id) {
                    bead.apply_event(event);
                }
                // Ignore events for unknown beads (defensive)
            }
        }
    }

    beads.into_values().collect()
}

fn insert_bead(conn: &Connection, bead: &Bead) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO beads \
         (id, title, description, status, priority, bead_type, project, \
          assignee, parent, created_at, updated_at, closed_at, close_reason, \
          claimed_at, claim_deadline, last_heartbeat) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            bead.id,
            bead.title,
            bead.description,
            bead.status.as_str(),
            bead.priority as i32,
            bead.bead_type.as_str(),
            bead.project,
            bead.assignee,
            bead.parent,
            bead.created_at.to_rfc3339(),
            bead.updated_at.to_rfc3339(),
            bead.closed_at.map(|dt| dt.to_rfc3339()),
            bead.close_reason,
            bead.claimed_at.map(|dt| dt.to_rfc3339()),
            bead.claim_deadline.map(|dt| dt.to_rfc3339()),
            bead.last_heartbeat.map(|dt| dt.to_rfc3339()),
        ],
    )?;

    // Clear and re-insert dependencies
    conn.execute("DELETE FROM dependencies WHERE bead_id = ?1", params![bead.id])?;
    for dep in &bead.dependencies {
        conn.execute(
            "INSERT OR IGNORE INTO dependencies (bead_id, depends_on) VALUES (?1, ?2)",
            params![bead.id, dep],
        )?;
    }

    // Clear and re-insert labels
    conn.execute("DELETE FROM labels WHERE bead_id = ?1", params![bead.id])?;
    for label in &bead.labels {
        conn.execute(
            "INSERT OR IGNORE INTO labels (bead_id, label) VALUES (?1, ?2)",
            params![bead.id, label],
        )?;
    }

    Ok(())
}

/// Update the blocked_cache for a single bead after upsert.
/// Checks each dependency's status in the DB to decide if the bead is blocked.
fn update_blocked_cache_single(conn: &Connection, bead: &Bead) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM blocked_cache WHERE bead_id = ?1",
        params![bead.id],
    )?;

    if !bead.dependencies.is_empty() {
        let mut blocking: Vec<String> = Vec::new();
        for dep_id in &bead.dependencies {
            let is_closed: bool = conn
                .query_row(
                    "SELECT status FROM beads WHERE id = ?1",
                    params![dep_id],
                    |row| row.get::<_, String>(0),
                )
                .map(|s| s == "closed")
                .unwrap_or(false); // unknown dep counts as blocking
            if !is_closed {
                blocking.push(dep_id.clone());
            }
        }

        if !blocking.is_empty() {
            let json = serde_json::to_string(&blocking).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "INSERT INTO blocked_cache (bead_id, blocked_by) VALUES (?1, ?2)",
                params![bead.id, json],
            )?;
        }
    }

    // When a bead is closed, re-evaluate dependents that might become unblocked
    if bead.status == Status::Closed {
        let mut stmt = conn.prepare(
            "SELECT bead_id FROM dependencies WHERE depends_on = ?1",
        )?;
        let dependent_ids: Vec<String> = match stmt.query_map(params![bead.id], |row| row.get(0)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        };

        for dep_bead_id in dependent_ids {
            let mut dep_stmt = conn.prepare(
                "SELECT depends_on FROM dependencies WHERE bead_id = ?1",
            )?;
            let deps: Vec<String> = match dep_stmt.query_map(params![dep_bead_id], |row| row.get(0)) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            };

            let mut still_blocking = Vec::new();
            for d in &deps {
                let s: Option<String> = conn
                    .query_row("SELECT status FROM beads WHERE id = ?1", params![d], |row| row.get(0))
                    .ok();
                if s.as_deref() != Some("closed") {
                    still_blocking.push(d.as_str());
                }
            }

            conn.execute("DELETE FROM blocked_cache WHERE bead_id = ?1", params![dep_bead_id])?;
            if !still_blocking.is_empty() {
                let json = serde_json::to_string(&still_blocking).unwrap_or_else(|_| "[]".to_string());
                conn.execute(
                    "INSERT INTO blocked_cache (bead_id, blocked_by) VALUES (?1, ?2)",
                    params![dep_bead_id, json],
                )?;
            }
        }
    }

    Ok(())
}

/// Rebuild the blocked_cache table. A bead is blocked if any of its
/// dependencies are not in 'closed' status.
fn rebuild_blocked_cache(conn: &Connection, beads: &[Bead]) -> Result<(), rusqlite::Error> {
    conn.execute("DELETE FROM blocked_cache", [])?;

    // Build a set of closed bead IDs for fast lookup
    let closed_ids: std::collections::HashSet<&str> = beads
        .iter()
        .filter(|b| b.status == Status::Closed)
        .map(|b| b.id.as_str())
        .collect();

    for bead in beads {
        if bead.dependencies.is_empty() {
            continue;
        }
        let blocking: Vec<&str> = bead
            .dependencies
            .iter()
            .filter(|dep_id| !closed_ids.contains(dep_id.as_str()))
            .map(|s| s.as_str())
            .collect();
        if !blocking.is_empty() {
            let json = serde_json::to_string(&blocking).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "INSERT INTO blocked_cache (bead_id, blocked_by) VALUES (?1, ?2)",
                params![bead.id, json],
            )?;
        }
    }

    Ok(())
}
