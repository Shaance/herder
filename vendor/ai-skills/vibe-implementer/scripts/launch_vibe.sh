#!/usr/bin/env bash
# Canonical detached headless launch for vibe-implementer runs.
# Owns the invocation flags, approval mode, evidence layout, and launch
# gate for consistent coordinator launches.
#
# Usage: launch_vibe.sh <repo> <brief-file> [run-dir] [-- extra vibe args]
# All tool calls are auto-approved without prompting.
# Prints: RUN_DIR=… PID=… SESSION_ID=… on success.
# Exit: 0 live, 1 launch failed, 3 gate elapsed with the process still alive.
set -euo pipefail

REPO=${1:?usage: launch_vibe.sh <repo> <brief-file> [run-dir] [-- extra args]}
BRIEF=${2:?missing brief file}
shift 2
RUN_DIR=
if [[ $# -gt 0 && $1 != --* ]]; then RUN_DIR=$1; shift; fi
RUN_DIR=${RUN_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/vibe-implementer.XXXXXX")}

if [[ $# -gt 0 ]]; then
	[[ $1 == -- ]] || { echo "error: unexpected argument '$1' (extra vibe args go after --)" >&2; exit 1; }
	shift
fi

[[ -d "$REPO" ]] || { echo "error: repo '$REPO' is not a directory" >&2; exit 1; }
[[ -s "$BRIEF" ]] || { echo "error: brief '$BRIEF' is missing or empty" >&2; exit 1; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }
command -v vibe >/dev/null || { echo "error: vibe CLI not on PATH" >&2; exit 1; }

[[ -e "$RUN_DIR/run.pid" ]] && { echo "error: $RUN_DIR already holds a run; continue it with resume_vibe.sh" >&2; exit 1; }
mkdir -p "$RUN_DIR"
cp "$BRIEF" "$RUN_DIR/brief.md"
printf 'vibe-headless-auto\n' >"$RUN_DIR/execution-mode"

# No --trust: repository config and hooks retain the existing trust behavior.
# Keep the recorded pid across exec, outside the coordinator's process group.
nohup perl -MPOSIX -e 'POSIX::setsid() >= 0 or die "setsid: $!"; exec @ARGV or die "exec: $!";' -- \
	vibe --workdir "$REPO" --auto-approve \
	--output streaming --output-dir "$RUN_DIR/out" \
	--prompt-file "$RUN_DIR/brief.md" "$@" \
	</dev/null >"$RUN_DIR/events.jsonl" 2>"$RUN_DIR/stderr.log" &
PID=$!
printf '%s\n' "$PID" >"$RUN_DIR/run.pid"

# Launch gate: the first streamed entry carries the session id (~2s observed).
for _ in $(seq 1 30); do
	SESSION_ID=$(head -1 "$RUN_DIR/events.jsonl" 2>/dev/null | jq -r '.sessionId // empty' 2>/dev/null || true)
	if [[ -n "$SESSION_ID" ]]; then
		printf '%s\n' "$SESSION_ID" >"$RUN_DIR/session-id"
		echo "RUN_DIR=$RUN_DIR PID=$PID SESSION_ID=$SESSION_ID"
		exit 0
	fi
	kill -0 "$PID" 2>/dev/null || break
	sleep 1
done

if kill -0 "$PID" 2>/dev/null; then
	echo "warn: gate window elapsed with the process ALIVE and no session id yet" >&2
	echo "do NOT kill; re-check with status_vibe.sh $RUN_DIR" >&2
	exit 3
fi
echo "error: launch gate failed — vibe exited before streaming a session id" >&2
echo "--- stderr head:" >&2
grep -v 'macOS Keychain' "$RUN_DIR/stderr.log" | head -c 800 >&2 || true
exit 1
