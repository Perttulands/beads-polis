# PRD: Beads for Polis

**Date:** 2026-03-12
**Status:** Draft
**Author:** opus1 (synthesized from 5-perspective agent review)
**Audience:** Any agent implementing or maintaining beads in Polis

---

## What This Is

Beads is the work tracking system for Polis. Every intentional piece of work — features, bugs, tasks, decisions — is a bead. No bead means it doesn't exist.

This document describes what beads must be to serve a multi-agent AI operating system where the human checks in every 4 hours. It is prescriptive. If the current implementation contradicts this document, fix the implementation.

---

## Design Principles

1. **JSONL is the database. SQLite is the cache.** The append-only JSONL log is the source of truth. The SQLite file is a derived, disposable index rebuilt automatically when stale. If they disagree, JSONL wins. Always.

2. **Boring technology only.** Standard rusqlite with bundled SQLite. POSIX flock for concurrency. No experimental database engines, no custom SQLite forks, no reimplementations of solved problems. The most battle-tested option wins every time.

3. **Self-healing without a human.** If the SQLite index corrupts, the system detects it and rebuilds from JSONL. If a claim goes stale, the system releases it. If a write fails, the data is safe in the log. No failure mode should require a human to run a manual command.

4. **Simplicity over features.** 400 beads across 15 projects is a tiny dataset. Full JSONL replay takes <100ms. Don't optimize for scale you don't have. Don't add abstractions for hypothetical future problems. The right amount of code is the minimum that works.

---

## Architecture

```
┌──────────────────────────────────────────────┐
│                   br CLI                      │
├──────────┬──────────────────┬────────────────┤
│  Write   │      Read        │   Maintenance  │
│          │                  │                │
│ flock    │ check watermark  │ auto-compact   │
│ append   │ rebuild if stale │ auto-snapshot  │
│ fsync    │ query SQLite     │ auto-rebuild   │
│ unlock   │ return results   │                │
│ update   │                  │                │
│ index    │                  │                │
├──────────┴──────────────────┴────────────────┤
│        events.jsonl  (source of truth)        │
│        index.db      (derived, disposable)    │
└──────────────────────────────────────────────┘
```

### Write Path

Every mutation follows this sequence:

1. Acquire exclusive `flock` on `events.jsonl.lock`
2. Append one JSON event line to `events.jsonl`
3. `fsync` the JSONL file
4. Update SQLite index (using `PRAGMA journal_mode=WAL` so concurrent readers are not blocked)
5. Update `index.watermark` with the new line count and `fsync`
6. Release flock

The watermark update happens inside the lock so readers never see a JSONL that is ahead of the watermark without the writer still holding the lock. This eliminates the race where a reader triggers a rebuild while the writer is mid-update.

Total lock hold time: <5ms for a typical write (JSONL append + SQLite insert + two fsyncs). With 3-5 concurrent agents, contention is negligible.

The event is a complete, self-describing record:

```json
{"op":"create","ts":"2026-03-12T18:00:00Z","actor":"athena","bead":{"id":"pol-abc1","title":"Fix relay timeout","status":"open","priority":1,"type":"bug","project":"relay",...}}
{"op":"update","ts":"2026-03-12T19:00:00Z","actor":"athena","id":"pol-abc1","fields":{"status":"in_progress","assignee":"athena","claimed_at":"...","claim_deadline":"..."}}
{"op":"close","ts":"2026-03-12T20:00:00Z","actor":"athena","id":"pol-abc1","reason":"Fixed in commit abc123"}
```

The current state of any bead is computed by replaying its events in order. This is event sourcing.

### Read Path

Every read operation starts by checking the index freshness:

```
1. Read stored index.watermark
2. If watermark < actual JSONL line count OR index.db missing OR integrity_check fails:
   → Acquire flock
   → Rebuild index from JSONL to index.db.tmp
   → Update watermark
   → Atomic rename index.db.tmp → index.db
   → Release flock
3. Query SQLite index
4. Return results
```

The rebuild acquires the flock to prevent races with concurrent writers or other rebuilding readers. The atomic rename ensures readers never see a partially-built index. The index is never trusted — it is always verified.

### Concurrency Model

**POSIX flock on the JSONL lock file.** That's it.

- Writers serialize behind the flock. Hold time is <1ms (one append + fsync).
- Readers use their own SQLite connection. If the index is stale, they rebuild it (which briefly reads the JSONL but doesn't need the write lock).
- No SQLite-level concurrency management needed. SQLite is read-only from the application's perspective — writes go to JSONL first.
- No CRDTs, no distributed consensus, no message queues. All writers are on the same machine. A mutex is the correct primitive.

### Storage Layout

One canonical location for all beads:

```
/home/polis/.polis/beads/
├── events.jsonl          # append-only event log (source of truth)
├── events.jsonl.lock     # flock target
├── index.db              # SQLite index (derived, disposable, git-ignored)
├── index.watermark       # last-processed JSONL line number
├── snapshots/            # periodic full-state snapshots (backup)
│   ├── 2026-03-12.jsonl
│   └── 2026-03-11.jsonl
└── config.yaml           # prefix, project registry
```

**One database, not fifteen.** 400 beads don't need 15 SQLite files. Project is a field on the bead, not a separate database. Per-project `.beads/` directories in tool repos are derived exports for git tracking, not authoritative stores.

For git-tracked project repos:

```
/home/polis/tools/gate/.beads/
└── beads.jsonl           # periodic export: current state WHERE project='gate'
```

These are generated artifacts. The canonical data lives in one place.

### Resilience

Three layers of defense:

**Layer 1 — Append-only with fsync.** A crash mid-write produces at most a truncated last line. Detected on next read (JSON parse fails on last line), discarded. No previous data at risk.

**Layer 2 — Atomic index rebuilds.** Full rebuilds write to `index.db.tmp`, then `rename()` (atomic on Linux). Incremental updates use SQLite transactions — if they corrupt, the next read triggers a full rebuild.

**Layer 3 — Periodic snapshots.** Daily compaction writes a full-state JSONL snapshot. Keep the last 7. Total storage: ~200KB per snapshot. This is the backup.

### Compaction

When `events.jsonl` exceeds 10,000 lines or 5MB:

1. Acquire flock
2. Compute current state of all beads by replaying events
3. Write one `snapshot` event per bead to `events.jsonl.new`
4. Atomic rename `events.jsonl.new` → `events.jsonl`
5. Archive the old file to `snapshots/`
6. Release flock

After compaction, the JSONL has exactly N lines (one per bead). Replay is instant.

---

## Data Model

A bead has these fields:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique ID with project prefix (e.g., `pol-abc1`, `gate-xyz2`) |
| `title` | string | yes | Short description of the work |
| `description` | string | no | Full context, acceptance criteria, design notes |
| `status` | enum | yes | `open`, `in_progress`, `closed`, `deferred` |
| `priority` | int | yes | 0=critical, 1=high, 2=medium, 3=low, 4=backlog |
| `type` | enum | yes | `epic`, `feature`, `bug`, `task`, `chore` |
| `project` | string | yes | Which project this belongs to (e.g., `gate`, `relay`, `city`) |
| `assignee` | string | no | Who is working on it |
| `parent` | string | no | Parent bead ID (for epic → child relationships) |
| `dependencies` | [string] | no | IDs this bead is blocked by |
| `labels` | [string] | no | Free-form tags |
| `created_at` | timestamp | yes | When created |
| `updated_at` | timestamp | yes | Last modification |
| `closed_at` | timestamp | no | When closed |
| `close_reason` | string | no | Why it was closed |
| `claimed_at` | timestamp | no | When an agent claimed it |
| `claim_deadline` | timestamp | no | Auto-release claim after this time |
| `last_heartbeat` | timestamp | no | Last time the assignee signaled activity |

### Agent Claim Semantics

Claiming work is the most contention-prone operation. The rules:

1. **Actor identity** is read from the `POLIS_ACTOR` environment variable, set by the OS supervisor or session launcher. The CLI fails with a clear error if `POLIS_ACTOR` is unset. Agents cannot self-report arbitrary identities — the environment is the authority.
2. `br claim <id>` sets `status=in_progress`, `assignee=$POLIS_ACTOR`, `claimed_at=now`, `claim_deadline=now+1h` (default, configurable with `--lock-for`).
3. If the bead is already claimed by another agent, the command fails with a structured error naming the current holder and their deadline.
4. If `claim_deadline` has passed and no heartbeat was received, any agent may re-claim. The system treats the previous claim as abandoned.
5. `br heartbeat <id>` extends the claim deadline by another hour. Agents doing long work call this periodically.
6. `br unclaim <id>` releases the claim, sets status back to `open`.
7. Only the current assignee or the human operator (`POLIS_ACTOR=operator`) can close or unclaim a bead that is in_progress. Other agents get a structured error.

This prevents the "agent dies mid-task, bead stuck forever" failure mode without requiring a human to intervene.

---

## CLI Interface

Eight core commands. Every command supports `--json` output.

```
br create <title> [-p priority] [-t type] [--project name] [--dep id] [--parent id]
br show <id>
br list [--project name] [--status S] [--priority P] [--type T]
br update <id> [--status S] [--priority P] [--title T] [--add-dep id] [--rm-dep id]
br close <id> --reason "text"
br ready [--project name]
br search <query>
br sync [--export-project gate] [--snapshot]
```

Agent workflow commands:

```
br claim <id> [--lock-for duration]
br heartbeat <id>
br unclaim <id>
```

Maintenance commands (run automatically, exposed for debugging):

```
br doctor              # check JSONL integrity, index freshness, stale claims
br rebuild             # force index rebuild from JSONL
br compact             # force compaction if threshold not yet reached
br city ready          # aggregated ready across all registered projects
br city list           # aggregated list across all registered projects
```

### Observability

- **Logging:** Non-fatal self-healing events (`index_rebuild_triggered`, `truncated_line_discarded`, `stale_claim_released`) are written to stderr and to a rotating log at `~/.polis/beads/beads.log`. These are warnings, not errors — the system handled them.
- **Thrash detection:** If `br` detects more than 5 index rebuilds within a 1-minute window, it emits `{"alert":"index_thrashing","rebuilds":N,"window":"60s"}` to stdout (JSON mode) or a warning to stderr (human mode). This signals a systematic problem (e.g., a writer that always corrupts the index) rather than normal self-healing.
- **`br doctor` output:** Reports JSONL line count, index watermark, integrity_check result, stale claims, and last rebuild timestamp. Machine-readable with `--json`.

### What does NOT belong in the CLI

- Migration commands — JSONL is forward-compatible by design
- Backup commands — snapshots happen automatically
- Recovery commands — recovery happens automatically on corruption detection

---

## Project Registration

City root `config.yaml` lists all projects:

```yaml
issue_prefix: pol
projects:
  gate: /home/polis/tools/gate
  relay: /home/polis/tools/relay
  work: /home/polis/tools/work
  senate: /home/polis/tools/senate
  argus: /home/polis/tools/argus
  chiron: /home/polis/tools/chiron
  loop: /home/polis/tools/learning-loop
  oathkeeper: /home/polis/tools/oathkeeper
  cmd: /home/polis/projects/polis-command
  orbit: /home/polis/projects/polis-orbit-ui
```

`br city ready` queries the single canonical database and filters by project. No multi-database aggregation needed.

`br sync --export-project gate` writes a derived `beads.jsonl` to `/home/polis/tools/gate/.beads/` for git tracking. This is a convenience, not a data path.

---

## Integration Contract

All downstream tools (`work`, `gate`, `senate`, `oathkeeper`, `argus`, `relay`) call `br` as a CLI subprocess. The contract:

| Command | Exit code 0 | Exit code non-zero | stdout (--json) |
|---------|------------|-------------------|-----------------|
| `br create` | Created successfully | Validation error | `{"id":"...","title":"..."}` |
| `br close` | Closed successfully | Not found / already closed | `{"id":"...","status":"closed"}` |
| `br claim` | Claimed successfully | Already claimed / not found | `{"id":"...","assignee":"...","claim_deadline":"..."}` |
| `br ready` | Results returned | Database error | `[{"id":"...","title":"...","priority":1,...},...]` |
| `br show` | Found | Not found | `{"id":"...","title":"...","status":"...",...}` |

**Error format** (stderr, JSON):
```json
{"error":"already_claimed","holder":"athena","deadline":"2026-03-12T20:00:00Z","bead":"pol-abc1"}
```

Structured errors so agents can make decisions, not just fail.

---

## Non-Requirements

Things this system explicitly does not need:

- **Web UI.** Polis-orbit-ui reads beads via CLI. The tracker doesn't need its own UI.
- **Multi-machine sync.** All agents are on one machine. Git push/pull of JSONL handles the rare case of working from another machine.
- **Full-text search engine.** `grep` on 400 JSONL lines is fast enough. SQLite FTS is available if needed later.
- **Concurrent writers on different machines.** Not a Polis requirement. If it becomes one, add a merge strategy to JSONL (last-write-wins per bead ID). Don't build it now.
- **MVCC / concurrent write transactions.** flock serialization is correct and sufficient for same-machine access. Don't reach for a harder solution to a problem you don't have.
- **Plugin system / extensibility framework.** Beads is a focused tool. Features go in beads, not in plugins.
- **Backward compatibility with frankensqlite databases.** JSONL is the migration path. Any frankensqlite-era `.db` file is discarded and rebuilt from JSONL.

---

## Migration From Current State

### Phase 1: Stabilize (Day 1)

1. Replace the current `br` binary (v0.1.19, frankensqlite) with upstream v0.1.14 (rusqlite).
2. Rebuild the SQLite index from JSONL: `br sync --import-only`.
3. Verify: `sqlite3 .beads/beads.db "PRAGMA integrity_check"` returns `ok`.
4. Verify: all existing commands work (`create`, `close`, `list`, `ready`, `show`, `update`, `dep`, `search`).
5. Run the system for 24 hours. Confirm zero corruption incidents.

### Phase 2: Invert the Architecture (Week 1)

1. Fork v0.1.14 as the new beads-polis base.
2. Apply the POSIX flock patch (the one good thing from the current fork).
3. Move writes to JSONL-first: every mutation appends to JSONL before touching SQLite.
4. Add watermark-based staleness detection: reads auto-rebuild the index when JSONL is ahead.
5. Add corruption detection: if any SQLite operation returns `SQLITE_CORRUPT`, delete the index and rebuild.
6. Remove all frankensqlite-specific code paths and workarounds.

### Phase 3: Agent-Optimized Features (Week 2)

1. Add `claim_deadline` and `last_heartbeat` fields to the data model.
2. Implement `br claim`, `br heartbeat`, `br unclaim`.
3. Add automatic stale-claim release (deadline passed + no heartbeat → status reverts to open).
4. Add `br doctor` with automatic fix capabilities.
5. Add periodic snapshot automation.

### Phase 4: City-Wide Intelligence (Week 2-3)

1. Move canonical storage to `/home/polis/.polis/beads/`.
2. Implement `br city ready` and `br city list` as filters on the single database.
3. Implement `br sync --export-project <name>` for per-repo git tracking.
4. Update all agent configs and skills to use the new location.

---

## Testing Strategy

1. **Unit tests:** 100% coverage on event sourcing replay logic. Given an array of events, the computed bead state must be exactly correct. Edge cases: out-of-order timestamps, duplicate events, truncated last line.

2. **Concurrency tests:** A harness that spawns 20 parallel processes running `br create`, `br update`, and `br claim` simultaneously for 30 seconds. The resulting JSONL must contain valid JSON on every line, and the rebuilt SQLite index must pass `PRAGMA integrity_check`.

3. **Crash tests:** A script that sends `SIGKILL` to `br` processes at random points during writes. Verify that the next `br` invocation detects any truncated JSONL line, discards it, rebuilds the index, and returns correct results. No human intervention allowed.

4. **Watermark race test:** Two processes — one writing, one reading — running concurrently for 60 seconds. The reader must never see an inconsistent state (watermark ahead of JSONL, or JSONL ahead of watermark without a rebuild).

---

## Success Criteria

The beads system is done when:

1. **Zero corruption incidents in 30 days of multi-agent operation.**
2. **`sqlite3 beads.db "PRAGMA integrity_check"` always returns `ok`** — the database is readable by standard SQLite tools, not just by `br`.
3. **Any agent can recover from any failure without human help.** Corrupt index? Auto-rebuilt. Stale claim? Auto-released. Truncated JSONL line? Auto-discarded.
4. **`br ready` returns results in <200ms** even after days of continuous agent writes.
5. **The WAL file never exceeds 1MB.** Checkpointing works.
6. **The entire system is under 2,000 lines of Rust** (excluding tests). If it's bigger, something is wrong.

---

## Dependencies

- `rusqlite` 0.31+ with `bundled` feature (real SQLite, not frankensqlite)
- `serde` + `serde_json` (JSONL serialization)
- `clap` (CLI parsing)
- `chrono` (timestamps)
- `fs2` (POSIX flock)

Five crate dependencies. No async runtime. No ORM. No migration framework.

---

## What This Replaces

- beads-polis v0.1.19 (frankensqlite-based fork with flock patch)
- The `br` binary at `/home/polis/.local/bin/br`
- All frankensqlite-related workarounds, recovery scripts, and corrupt DB archives
- The "database is malformed" error message as a regular occurrence in Polis operations

---

_This PRD was synthesized from five independent analyses: integration mapping, production reliability audit, alternative systems research, architecture design, and agent workflow assessment. Every recommendation is grounded in observed failure modes from 12 days of multi-agent operation._
