# beads-polis — Fork of beads_rust

**Upstream:** https://github.com/Dicklesworthstone/beads_rust  
**Upstream author:** Jeffrey Emanuel (Dicklesworthstone)  
**Fork purpose:** City-specific extensions for Polis multi-agent system

---

## Why This Fork Exists

beads_rust was built for multi-agent work. The daemon that was supposed to
serialize concurrent writes was never implemented in v1 (`--no-daemon` is
documented as "effectively no-op in br v1"). Additionally, frankensqlite's
`UnixFile::lock()` is a no-op — it updates an in-memory field but never calls
`posix_lock()` on the main database file.

In Polis, multiple agents write to the central beads database concurrently.
Without writer serialization, the database corrupts.

This fork adds POSIX advisory locking (`flock`) so concurrent writers
block instead of corrupting. One focused fix; everything else tracks upstream.

---

## Divergence Log

| Date | Commit | Description |
|------|--------|-------------|
| 2026-02-28 | `flock-fix` | fix(storage): POSIX flock via fs2 on beads.lock — serializes concurrent writers |

---

## Staying in Sync with Upstream

```bash
git fetch upstream
git log upstream/main --oneline | head -10   # see what's new
git merge upstream/main                       # merge when clean
```

Before merging upstream: run `cargo test --lib` to catch regressions.
If upstream ships a real daemon or fixes frankensqlite locking, this fork's
flock patch can be dropped.

---

## Attribution

beads_rust is MIT licensed. Original work by Jeffrey Emanuel.  
This fork is maintained as part of the Polis city system by Perttu Landström.
