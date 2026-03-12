# Testing — beads-v2

Test suite for the event-sourced beads-v2 rewrite.

---

## Test Summary

**90 tests total** across 10 test files, 3 ignored (timing-sensitive CI tests).

| File | Tests | What it covers |
|---|---|---|
| `src/lib.rs` (unit) | 15 | CLI parsing, engine open/ID generation, format helpers |
| `tests/cmd_core.rs` | 13 | CRUD lifecycle, filters, search, dependency blocking |
| `tests/cmd_agent.rs` | 10 | Claim/heartbeat/unclaim, permission checks, deadline expiry |
| `tests/cmd_maintenance.rs` | 10 | Doctor diagnostics, rebuild, compact, CLI e2e |
| `tests/claim_semantics.rs` | 8 | Claim invariants: one-winner, permission, expiry, heartbeat |
| `tests/event_replay.rs` | 13 | Event sourcing: create/update/close/reopen replay, idempotency |
| `tests/e2e.rs` | 17 | Full binary e2e: CRUD, deps, claims, search, doctor, compact, migration, concurrency |
| `tests/concurrency.rs` | 2 | JSONL flock serialization, index integrity under contention |
| `tests/crash_recovery.rs` | 3 | Truncated line discard, append-after-recovery |
| `tests/watermark_race.rs` | 2 | Concurrent reader/writer watermark consistency |

---

## Test Categories

### Unit Tests (15)
- CLI argument parsing for all commands and flag combinations
- Engine initialization (fresh dir, existing events, ID generation)
- Error and help text formatting

### Integration Tests (33)
- Full Engine-level CRUD: create, show, list, update, close
- Filter combinations: status, project, priority, type, labels
- Dependency blocking: blocked beads excluded from ready queue
- Claim lifecycle: claim → heartbeat → unclaim, conflict detection
- Permission enforcement: only holder can modify claimed beads
- Deadline expiry: expired claims can be re-claimed

### Event Replay Tests (13)
- Each event type produces correct bead state
- Multiple updates accumulate correctly
- Close/reopen cycles preserve state
- Duplicate events are idempotent
- Serialization roundtrip fidelity
- Truncated last line detection

### End-to-End Tests (17)
- Full binary execution via `std::process::Command`
- Both human and JSON output modes
- Error cases: missing actor, not found, permission denied
- Legacy migration (`issues.jsonl` → `events.jsonl`)
- City commands (cross-project ready/list)
- **Concurrent creates**: 10 parallel processes writing simultaneously
- Doctor health check, rebuild, compact

### Concurrency Tests (4, 1 ignored)
- 10 parallel JSONL writers produce valid, complete output
- Index integrity after concurrent writes
- Reader/writer watermark race detection

### Crash Recovery Tests (3, 1 ignored)
- Truncated last line discarded on read
- Append succeeds after truncation recovery

---

## Running Tests

```bash
cd beads-v2

# All tests
cargo test

# Only e2e (requires built binary)
cargo test --test e2e

# Only unit tests
cargo test --lib

# Specific test file
cargo test --test cmd_core
cargo test --test concurrency

# With output
cargo test -- --nocapture
```

---

## Ignored Tests

3 tests are `#[ignore]`d — they require longer timeouts or specific
concurrency conditions that are flaky in CI:

- `concurrent_eventlog_writes_with_index_integrity` — 10-writer contention with index rebuild
- `eventlog_crash_recovery` — simulates mid-write crash via process kill
- `eventlog_watermark_race_with_index` — 5-second sustained reader/writer race

Run them explicitly with:

```bash
cargo test -- --ignored
```

---

## What the Tests Verify for Multi-Agent Use

The test suite specifically targets Polis multi-agent concerns:

1. **Write serialization** — flock prevents concurrent JSONL corruption
2. **Index rebuild safety** — flock + double-check prevents concurrent rebuilds
3. **Claim exclusivity** — only one agent can hold a bead at a time
4. **Permission enforcement** — only the claim holder can modify/close
5. **Deadline-based expiry** — stale claims auto-release via doctor
6. **Crash resilience** — truncated writes are discarded, not propagated
7. **Actor attribution** — every event records who did it and when

---

## Gate Status

`gate check` runs tests, clippy, truthsayer, and UBS. Current status:

- **Tests**: PASS (90 tests, 0 failures)
- **Clippy**: PASS (0 warnings)
- **Truthsayer**: PASS
- **UBS**: Reports false positives for `panic!` in `#[cfg(test)]` code — this is idiomatic Rust test code. UBS does not distinguish test modules from production code.
