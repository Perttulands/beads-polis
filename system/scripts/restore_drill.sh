#!/usr/bin/env bash
# Run a disposable br backup/restore drill and save a markdown transcript.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYSTEM_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$SYSTEM_DIR/.." && pwd)"

MODE="manual"
TRANSCRIPT_PATH=""
ARTIFACT_DIR=""
KEEP_WORKSPACE=0
DRILL_ACTOR="${DRILL_ACTOR:-restore-drill}"

usage() {
    cat <<'EOF'
Usage: restore_drill.sh [options]

Options:
  --mode <manual|ci>        Label the transcript (default: manual)
  --transcript <path>       Markdown transcript output path
  --artifact-dir <path>     Directory for raw command logs and bundle copy
  --keep-workspace          Preserve the disposable workspace for inspection
  -h, --help                Show this help

Environment:
  BR_BINARY                 Path to the br binary to use
  DRILL_ACTOR               Actor name for write operations (default: restore-drill)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            MODE="$2"
            shift 2
            ;;
        --transcript)
            TRANSCRIPT_PATH="$2"
            shift 2
            ;;
        --artifact-dir)
            ARTIFACT_DIR="$2"
            shift 2
            ;;
        --keep-workspace)
            KEEP_WORKSPACE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$TRANSCRIPT_PATH" ]]; then
    TRANSCRIPT_PATH="$SYSTEM_DIR/target/restore-drill/restore-drill-${MODE}.md"
fi

if [[ -z "$ARTIFACT_DIR" ]]; then
    ARTIFACT_DIR="$SYSTEM_DIR/target/restore-drill/${MODE}"
fi

mkdir -p "$(dirname "$TRANSCRIPT_PATH")" "$ARTIFACT_DIR"

if [[ -n "${BR_BINARY:-}" ]]; then
    BR_CMD="$BR_BINARY"
else
    (
        cd "$SYSTEM_DIR"
        cargo build --release >/dev/null
    )
    BR_CMD="$SYSTEM_DIR/target/release/br"
fi

if [[ ! -x "$BR_CMD" ]]; then
    echo "br binary not executable: $BR_CMD" >&2
    exit 1
fi

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/br-restore-drill.XXXXXX")"
WORKSPACE="$TEMP_ROOT/workspace"
BUNDLE_DIR="$TEMP_ROOT/backup-bundle"
mkdir -p "$WORKSPACE"

cleanup() {
    if [[ "$KEEP_WORKSPACE" -eq 0 ]]; then
        rm -rf "$TEMP_ROOT"
    fi
}
trap cleanup EXIT

RUN_COUNT=0
RESTORE_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")"
STARTED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat >"$TRANSCRIPT_PATH" <<EOF
# br Restore Drill Transcript

- Mode: $MODE
- Started at: $STARTED_AT
- Repo: $REPO_ROOT
- Commit: $RESTORE_COMMIT
- Binary: $BR_CMD
- Runbook: [RECOVERY.md](../RECOVERY.md)

## Summary

This drill creates a disposable workspace, captures a backup bundle, mutates the workspace,
restores the bundle with \`--verify --force\`, and confirms that the restored JSONL matches the bundled copy.

EOF

log_step() {
    local title="$1"
    local command="$2"
    local stdout_file="$3"
    local stderr_file="$4"
    local status="$5"

    {
        echo "## $title"
        echo
        echo '```bash'
        echo "$command"
        echo '```'
        echo
        echo "- Exit code: $status"
        echo
        echo "### stdout"
        echo
        echo '```text'
        cat "$stdout_file"
        echo '```'
        echo
        if [[ -s "$stderr_file" ]]; then
            echo "### stderr"
            echo
            echo '```text'
            cat "$stderr_file"
            echo '```'
            echo
        fi
    } >>"$TRANSCRIPT_PATH"
}

run_br() {
    local label="$1"
    shift
    RUN_COUNT=$((RUN_COUNT + 1))
    local prefix
    prefix="$(printf '%02d_%s' "$RUN_COUNT" "$label")"
    local stdout_file="$ARTIFACT_DIR/${prefix}.stdout"
    local stderr_file="$ARTIFACT_DIR/${prefix}.stderr"
    local cmd_display
    cmd_display="cd \"$WORKSPACE\" && $*"

    set +e
    (
        cd "$WORKSPACE"
        env -u BEADS_DIR HOME="$WORKSPACE" POLIS_ACTOR="$DRILL_ACTOR" "$BR_CMD" "$@"
    ) >"$stdout_file" 2>"$stderr_file"
    local status=$?
    set -e

    log_step "$label" "$cmd_display" "$stdout_file" "$stderr_file" "$status"
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi
    return "$status"
}

run_shell_check() {
    local label="$1"
    local command="$2"
    RUN_COUNT=$((RUN_COUNT + 1))
    local prefix
    prefix="$(printf '%02d_%s' "$RUN_COUNT" "$label")"
    local stdout_file="$ARTIFACT_DIR/${prefix}.stdout"
    local stderr_file="$ARTIFACT_DIR/${prefix}.stderr"

    set +e
    bash -lc "$command" >"$stdout_file" 2>"$stderr_file"
    local status=$?
    set -e

    log_step "$label" "$command" "$stdout_file" "$stderr_file" "$status"
    if [[ "$status" -ne 0 ]]; then
        return "$status"
    fi
    return "$status"
}

run_br "init" init
run_br "create_baseline" create "Restore drill baseline" -p 1 -t task
run_br "backup" backup --output "$BUNDLE_DIR"
run_br "create_mutation" create "Post-backup mutation" -p 2 -t task
run_shell_check \
    "verify_mutation_present" \
    "grep -q 'Post-backup mutation' '$WORKSPACE/.beads/issues.jsonl'"
run_br "restore_verify" restore "$BUNDLE_DIR" --verify --force
run_shell_check \
    "compare_restored_jsonl" \
    "cmp -s '$WORKSPACE/.beads/issues.jsonl' '$BUNDLE_DIR/jsonl/issues.jsonl'"
run_br "health_json" health --json

cp -R "$BUNDLE_DIR" "$ARTIFACT_DIR/bundle"

{
    echo "## Result"
    echo
    echo "- Restore drill: PASS"
    echo "- Bundle copy: \`$ARTIFACT_DIR/bundle\`"
    echo "- Disposable workspace: \`$WORKSPACE\`"
    if [[ "$KEEP_WORKSPACE" -eq 0 ]]; then
        echo "- Workspace cleanup: automatic"
    else
        echo "- Workspace cleanup: preserved for inspection"
    fi
    echo
} >>"$TRANSCRIPT_PATH"

printf 'restore drill transcript: %s\n' "$TRANSCRIPT_PATH"
