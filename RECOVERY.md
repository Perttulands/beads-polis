# br Recovery Runbook

This runbook is the supported D7 recovery path for `beads-polis`.

## Quick Checks

```bash
br health
br health --json
```

Use `br doctor` only as a compatibility alias. `br health` is the stable operator-facing command.

## Create a Backup

```bash
br backup
```

Default location:

```text
.beads/.br_backups/<timestamp>/
```

Bundle contents:

- `manifest.json` with SHA-256 checksums
- current SQLite database
- current JSONL export
- `metadata.json`
- `config.yaml`
- `.br_history/` files when present

Custom output path:

```bash
br backup --output /tmp/br-backup-2026-03-12
```

## Restore a Backup

Dry rule:

- restoring normally refuses to overwrite existing files
- use `--force` for a real recovery restore

Verified restore:

```bash
br restore /path/to/backup-bundle --verify --force
```

Restore always validates the backup bundle manifest and checksums before copying files.

`--verify` adds:

- SQLite `PRAGMA integrity_check` after copy
- JSONL parse validation after copy

## Recommended Manual Drill

1. `br health`
2. `br backup`
3. make a disposable change in a throwaway workspace copy
4. `br restore <bundle> --verify --force`
5. `br health --json`
6. confirm the restored JSONL matches the bundle copy

Automation helper:

```bash
cd system
scripts/restore_drill.sh --mode manual --transcript ../restore-drills/2026-03-12-manual.md
```

## When to Use Which Path

- Index stale or disposable DB problem: prefer `br health`, then `br sync --import-only` or rebuild the DB from JSONL.
- Workspace-level recovery or pre-migration safety snapshot: use `br backup`.
- Suspected corruption or failed migration rollback: use `br restore --verify --force`.

## Notes

- `br backup` writes under `.beads/.br_backups/` by default, and new workspaces ignore that directory in `.beads/.gitignore`.
- A restore drill should be completed before any schema or storage migration that raises `br` criticality.
- CI runs the same drill helper and uploads the transcript/log bundle as the `br-restore-drill` artifact.
