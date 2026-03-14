# beads-polis

Event-sourced work tracker for multi-agent AI operations.

**JSONL is the source of truth. SQLite is a derived, disposable index.**

---

## What Changed

This was originally a fork of [beads_rust](https://github.com/Dicklesworthstone/beads_rust) by Jeffrey Emanuel. The upstream project switched to `frankensqlite` (a from-scratch SQLite reimplementation) in v0.1.15, which produced corrupt databases under concurrent access. Rather than continuing to patch around that, we rewrote the storage layer from scratch.

**beads-polis** keeps the `br` CLI interface and JSONL data format but replaces the entire storage architecture:

| | Old (beads_rust fork) | New (beads-polis) |
|---|---|---|
| Write path | SQLite primary, JSONL export | JSONL append-only (source of truth) |
| Read path | SQLite queries | SQLite derived index, auto-rebuilt |
| Concurrency | flock patch on broken SQLite | flock on JSONL, SQLite is disposable |
| Corruption recovery | Manual | Automatic (delete index, replay JSONL) |
| Lines of code | ~20,000 | ~3,300 |

See [PRD.md](PRD.md) for the full design rationale.

---

## Install

```bash
cargo build --release
cp target/release/br ~/.local/bin/br
```

Requires: Rust 2021 edition, Linux (POSIX flock).

## Quick Start

```bash
export POLIS_ACTOR=your-name    # Required for all write operations

br create "Fix relay timeout" -p 1 -t bug --project relay
br list --status open
br ready                        # Unblocked, actionable work
br show pol-abc1
br close pol-abc1 --reason "Fixed in commit def456"
```

## Commands

### Core

| Command | Description |
|---------|-------------|
| `br create <title>` | Create a bead. Flags: `-p` priority, `-t` type, `--project`, `--dep`, `--parent`, `-l` label, `--description` |
| `br show <id>` | Full bead details |
| `br list` | List beads. Flags: `--status`, `--project`, `--priority`, `-t` type |
| `br update <id>` | Update fields. Flags: `--title`, `--priority`, `--status`, `--add-dep`, `--rm-dep`, `--project`, `--assignee` |
| `br close <id>` | Close with `--reason` |
| `br ready` | Unblocked open beads, sorted by priority. Optional `--project` filter |
| `br search <query>` | Full-text search on title and description |

### Agent Workflow

| Command | Description |
|---------|-------------|
| `br claim <id>` | Claim for work. Sets in_progress + assignee + deadline. `--lock-for 2h` |
| `br heartbeat <id>` | Signal still working. Extends deadline by 1h |
| `br unclaim <id>` | Release claim. Reverts to open |

Claim semantics: only the holder (or `operator`) can heartbeat, unclaim, or close a claimed bead. Expired claims can be re-claimed by anyone.

### Maintenance

| Command | Description |
|---------|-------------|
| `br health` | Preferred health check: JSONL validity, index freshness, SQLite integrity, and sync metadata |
| `br doctor` | Legacy alias for the same diagnostics surface as `br health` |
| `br backup` | Create a recoverable backup bundle with checksums in `.beads/.br_backups/` by default |
| `br restore <bundle> --verify --force` | Restore a backup bundle, always validate bundle checksums, and fail closed on post-restore integrity errors |
| `br rebuild` | Force full index rebuild from JSONL |
| `br compact` | Collapse event history into snapshots. Archives old log |
| `br sync --import-only` | Rebuild index from JSONL |
| `br sync --snapshot` | Same as compact |
| `br sync --migrate` | Convert legacy `issues.jsonl` to event-sourced `events.jsonl` |
| `br sync --export-project <name>` | Export project beads to its repo `.beads/` dir |

### Cross-Project

| Command | Description |
|---------|-------------|
| `br city ready` | Ready beads across all projects |
| `br city list` | List beads across all projects, with filters |

### Global Flags

| Flag | Description |
|------|-------------|
| `--json` | Machine-readable JSON output |
| `--actor <name>` | Override POLIS_ACTOR |
| `--db <path>` | Override beads directory |
| `--no-color` | Disable colored output |
| `-v` / `-vv` | Increase verbosity |

## Architecture

```
Write path:
  br create/update/close/claim → flock → append to events.jsonl → fsync → update watermark → upsert SQLite

Read path:
  br show/list/ready/search → check watermark → rebuild index if stale → query SQLite

Recovery:
  Corrupt index? Delete it. Next read auto-rebuilds from JSONL.
  Truncated last line? Discarded on read (crash resilience).
  Bad line in middle? Skipped, other events preserved.
  Recovery drill? Run `br backup`, then `br restore <bundle> --verify --force`.
```

See [RECOVERY.md](RECOVERY.md) for the operator runbook.

### Data Files

```
.beads/
  events.jsonl       # Source of truth (append-only JSONL)
  events.jsonl.lock  # POSIX flock for concurrent writes
  index.db           # Derived SQLite index (disposable)
  index.watermark    # Line count at last index sync
  config.yaml        # issue_prefix, projects map
  snapshots/         # Archived logs from compaction
```

### Event Types

Every mutation appends one event to `events.jsonl`:

- **create** — New bead with full initial state
- **update** — Field-level changes (only changed fields stored)
- **close** — Status=closed with reason
- **reopen** — Revert a close
- **snapshot** — Full state (produced by compaction)

### Concurrency Model

- All writes acquire an exclusive POSIX advisory lock (`flock`) on `events.jsonl.lock`
- Writes are serialized: one writer at a time, others block
- Reads never block (SQLite WAL mode)
- Index rebuilds also acquire flock to prevent concurrent rebuilds
- The JSONL file is the single source of truth; the index is always rebuildable

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `POLIS_ACTOR` | Yes (for writes) | Agent identity for event attribution |
| `BEADS_DIR` | No | Override beads directory (otherwise auto-detected from `.beads/` in parent dirs) |

## Migration from Legacy

If you have data in the old `issues.jsonl` format:

```bash
br sync --migrate
```

This converts each legacy issue into a snapshot event in `events.jsonl`, preserving all fields.

## Testing

```bash
cargo test              # 90+ tests: unit, integration, e2e
cargo test --test e2e   # End-to-end tests using the actual binary
```

Test coverage includes:
- CRUD lifecycle (create, show, list, update, close)
- Dependency blocking and unblocking
- Claim/heartbeat/unclaim with permission checks
- Concurrent writes (10 parallel processes)
- Crash recovery (truncated lines)
- Compaction and rebuild
- Legacy migration
- Human and JSON output modes

## History

- **beads_rust** by Jeffrey Emanuel — original SQLite-primary tracker
- **beads-polis fork** — added POSIX flock for concurrent writes
- **v0.1.14** — last upstream version with real SQLite (before frankensqlite)
- **beads-polis** (this) — event-sourced rewrite, JSONL-first, 3,300 lines

## Attribution

Original `beads_rust` by Jeffrey Emanuel. Licensed under MIT.
Rewrite by Polis agents (opus1, codex1) for Polis multi-agent operations.
