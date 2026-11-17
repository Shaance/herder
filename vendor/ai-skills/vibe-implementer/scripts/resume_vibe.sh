#!/usr/bin/env bash
# Canonical detached resume of a vibe-implementer run: exact stored session id
# (never --continue), the recorded approval mode, suffixed evidence
# files, and a launch gate that requires the stream to carry that same session id.
# Usage: resume_vibe.sh <run-dir> <repo> <delta-brief> [suffix] [-- extra vibe args]
set -euo pipefail

RUN_DIR=${1:?usage: resume_vibe.sh <run-dir> <repo> <delta-brief> [suffix] [-- extra args]}
REPO=${2:?missing repo}
DELTA=${3:?missing delta brief}
shift 3
SUFFIX=2
if [[ $# -gt 0 && $1 != --* ]]; then SUFFIX=$1; shift; fi

if [[ $# -gt 0 ]]; then
	[[ $1 == -- ]] || { echo "error: unexpected argument '$1' (extra vibe args go after --)" >&2; exit 1; }
	shift
fi

SESSION_ID=$(cat "$RUN_DIR/session-id" 2>/dev/null || true)
[[ -n "$SESSION_ID" ]] || { echo "error: no session-id in $RUN_DIR" >&2; exit 1; }
[[ -s "$DELTA" ]] || { echo "error: delta brief missing or empty" >&2; exit 1; }
[[ -e "$RUN_DIR/events${SUFFIX}.jsonl" ]] && { echo "error: suffix $SUFFIX already used in $RUN_DIR" >&2; exit 1; }

# Single-writer guard: a live pid means the session already has a writer.
OLD_PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || true)
if [[ -n "$OLD_PID" ]] && kill -0 "$OLD_PID" 2>/dev/null; then
	echo "error: run.pid $OLD_PID is still alive — refusing a second writer" >&2
	exit 4
fi

MODE=$(cat "$RUN_DIR/execution-mode")
case "$MODE" in
	vibe-headless-auto) approval=(--auto-approve) ;;
	vibe-headless-smart) approval=(--smart-approve) ;;
	vibe-headless-strict) echo "error: strict mode was removed; launch a new run" >&2; exit 1 ;;
	*) echo "error: unknown execution mode: $MODE" >&2; exit 1 ;;
esac
cp "$DELTA" "$RUN_DIR/delta${SUFFIX}.md"

# Keep the recorded pid across exec, outside the coordinator's process group.
nohup perl -MPOSIX -e 'POSIX::setsid() >= 0 or die "setsid: $!"; exec @ARGV or die "exec: $!";' -- \
	vibe --workdir "$REPO" --resume "$SESSION_ID" \
	"${approval[@]}" --output streaming --output-dir "$RUN_DIR/out${SUFFIX}" \
	--prompt-file "$RUN_DIR/delta${SUFFIX}.md" "$@" \
	</dev/null >"$RUN_DIR/events${SUFFIX}.jsonl" 2>"$RUN_DIR/stderr${SUFFIX}.log" &
PID=$!
printf '%s\n' "$PID" >"$RUN_DIR/run.pid"

for _ in $(seq 1 30); do
	FIRST_ID=$(head -1 "$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null | jq -r '.sessionId // empty' 2>/dev/null || true)
	if [[ -n "$FIRST_ID" ]]; then
		[[ "$FIRST_ID" == "$SESSION_ID" ]] || { echo "error: resumed stream reports session $FIRST_ID, expected $SESSION_ID" >&2; exit 1; }
		echo "RUN_DIR=$RUN_DIR PID=$PID SESSION_ID=$SESSION_ID SUFFIX=$SUFFIX"
		exit 0
	fi
	kill -0 "$PID" 2>/dev/null || break
	sleep 1
done
if kill -0 "$PID" 2>/dev/null; then
	echo "warn: gate window elapsed with the process ALIVE; do NOT kill —" >&2
	echo "re-check with status_vibe.sh $RUN_DIR $SUFFIX" >&2
	exit 3
fi
echo "error: resume launch gate failed — vibe exited early" >&2
grep -v 'macOS Keychain' "$RUN_DIR/stderr${SUFFIX}.log" | head -c 800 >&2 || true
exit 1
