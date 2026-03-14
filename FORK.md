# beads-polis — Fork of beads_rust

**Upstream:** https://github.com/Dicklesworthstone/beads_rust
**Upstream author:** Jeffrey Emanuel (Dicklesworthstone)
**Fork purpose:** Event-sourced rewrite for Polis multi-agent operations

---

## Why This Fork Exists

beads_rust was built as a single-user SQLite-primary work tracker. When Polis
adopted it for multi-agent operations, two problems emerged:

1. **No concurrent write safety.** The upstream daemon was never implemented
   (`--no-daemon` is documented as "effectively no-op in v1"), and frankensqlite's
   `UnixFile::lock()` is a no-op — it updates an in-memory field but never calls
   `posix_lock()`. Multiple agents writing simultaneously corrupted the database.

2. **frankensqlite corruption.** The upstream switched to frankensqlite (a
   from-scratch SQLite reimplementation) in v0.1.15, which produced corrupt
   databases under concurrent access — B-tree page overflows, missing index
   entries, and unrecoverable WAL states.

We initially patched the fork with POSIX `flock` to serialize writers, but
the frankensqlite corruption was a deeper problem. Rather than continuing
to patch around a broken storage layer, we rewrote the entire system.

---

## Event-Sourced Rewrite

**beads-polis** keeps the `br` CLI interface and data format but replaces the
storage architecture entirely:

| | Old (beads_rust fork) | New (beads-polis) |
|---|---|---|
| Write path | SQLite primary, JSONL export | JSONL append-only (source of truth) |
| Read path | SQLite queries | SQLite derived index, auto-rebuilt |
| Concurrency | flock patch on broken SQLite | flock on JSONL, SQLite is disposable |
| Corruption recovery | Manual | Automatic (delete index, replay JSONL) |
| Lines of code | ~20,000 | ~3,300 |
| Test suite | ~2,100 inherited tests | 90+ focused tests (unit, integration, e2e) |

See [PRD.md](PRD.md) for the full design rationale.

---

## Divergence Log

| Date | Description |
|------|-------------|
| 2026-02-28 | fix(storage): POSIX flock via fs2 on beads.lock — serializes concurrent writers |
| 2026-03-12 | beads-v2: event-sourced rewrite — JSONL-first, 3,300 LOC, replaces entire storage layer |

---

## Relationship with Upstream

beads-polis is a complete rewrite. We no longer track upstream changes.
The old fork code has been removed. The `br` binary is built from the repo root.

---

## Attribution

beads_rust is MIT licensed. Original work by Jeffrey Emanuel.
Rewrite by Polis agents (opus1, codex1) for Polis multi-agent operations.
Maintained as part of the Polis city system by Perttu Landström.
