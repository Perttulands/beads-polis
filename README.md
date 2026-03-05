# beads-polis

Polis-maintained fork of `beads_rust` for city workflows.

This repo exists because Polis needs a stable, local-first issue tracker (`br`)
with behavior that works under multi-agent concurrency and Polis operational
constraints.

## Upstream

- Upstream project: `beads_rust`
- Upstream author: Jeffrey Emanuel (`Dicklesworthstone`)
- Upstream repo: https://github.com/Dicklesworthstone/beads_rust

Fork rationale and divergence notes are tracked in [FORK.md](FORK.md).

## Repo Layout

- `system/` - active Rust project (`br` CLI), tests, packaging, docs
- `FORK.md` - fork policy and divergence log
- `TESTING.md` - Polis testing notes
- `polis/` - reserved for Polis-specific overlays (currently minimal)

## Build And Test

```bash
cd system
cargo build
cargo test
```

Run the CLI locally:

```bash
cd system
./target/debug/br --help
```

## Using `br` In Polis

Typical workflow:

```bash
br ready
br create "Title" -p 1 -t task -l <project>
br close <id> --reason "what changed"
br sync --flush-only
```

`br` is non-invasive: it does not run git commands for you.
After sync, commit `.beads/` changes explicitly.

## Fork Maintenance

1. Keep Polis changes focused and documented in `FORK.md`.
2. Periodically fetch/merge upstream changes.
3. Re-run tests in `system/` after merges.
4. Prefer compatibility with existing Polis `br` workflows.

## Attribution

Original `beads_rust` is MIT licensed.
This fork is maintained by Perttu Landstrom for Polis.
