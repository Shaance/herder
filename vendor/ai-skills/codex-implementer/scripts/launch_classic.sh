#!/usr/bin/env bash
# Canonical detached classic launch for codex-implementer runs.
# Owns the invocation flags, evidence layout, and launch gate so no
# coordinator ever hand-rolls a launch that drops --json or skips the gate.
#
# Usage: launch_classic.sh <repo> <brief-file> [run-dir] [extra codex args...]
# Prints: RUN_DIR=… PID=… THREAD_ID=… on success; exits 1 with diagnostics
# if the stream stays empty past the gate window.
set -euo pipefail

REPO=${1:?usage: launch_classic.sh <repo> <brief-file> [run-dir] [extra args...]}
BRIEF=${2:?missing brief file}
RUN_DIR=${3:-$(mktemp -d "${TMPDIR:-/tmp}/codex-implementer.XXXXXX")}
shift $(( $# >= 3 ? 3 : 2 ))
# A caller sandbox replaces the default (codex rejects a repeated --sandbox). A Crew
# lead runs danger-full-access because a sandboxed process cannot start the sandboxed
# workers it dispatches: macOS refuses nested seatbelt (sandbox_apply: Operation not permitted).
case " $* " in
*" --sandbox "* | *" --sandbox="* | *" -s "* | *" --dangerously-bypass-approvals-and-sandbox "*) ;;
*) set -- --sandbox workspace-write "$@" ;;
esac

[[ -d "$REPO" ]] || { echo "error: repo '$REPO' is not a directory" >&2; exit 1; }
[[ -s "$BRIEF" ]] || { echo "error: brief '$BRIEF' is missing or empty" >&2; exit 1; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }

mkdir -p "$RUN_DIR"
cp "$BRIEF" "$RUN_DIR/brief.md"
printf 'classic\n' >"$RUN_DIR/execution-mode"

# Keep the recorded pid across exec, outside the coordinator's process group.
nohup perl -MPOSIX -e 'POSIX::setsid() >= 0 or die "setsid: $!"; exec @ARGV or die "exec: $!";' -- \
	codex exec -C "$REPO" --json \
	"$@" \
	-o "$RUN_DIR/final.md" - <"$RUN_DIR/brief.md" \
	>"$RUN_DIR/events.jsonl" 2>"$RUN_DIR/stderr.log" &
PID=$!
printf '%s\n' "$PID" >"$RUN_DIR/run.pid"

# Launch gate: the event stream must produce a thread id within ~90s
# (codex startup can exceed 30s when it preloads skill descriptions).
for _ in $(seq 1 90); do
	if [[ -s "$RUN_DIR/events.jsonl" ]]; then
		# read accepts only newline-terminated records; persist acceptance immediately.
		while IFS= read -r LINE; do
			THREAD_ID=$(jq -r 'select(.type=="thread.started") | .thread_id // empty' <<<"$LINE")
			if [[ -n "$THREAD_ID" ]]; then
				printf '%s\n' "$THREAD_ID" >"$RUN_DIR/thread-id"
				echo "RUN_DIR=$RUN_DIR PID=$PID THREAD_ID=$THREAD_ID"
				exit 0
			fi
		done <"$RUN_DIR/events.jsonl"
	fi
	kill -0 "$PID" 2>/dev/null || break
	sleep 1
done

if kill -0 "$PID" 2>/dev/null; then
	# Alive but slow is not dead: leave it running, tell the caller to
	# re-check with status_classic.sh instead of killing.
	echo "warn: gate window elapsed with the process ALIVE and no thread id yet" >&2
	echo "do NOT kill; re-check with status_classic.sh $RUN_DIR" >&2
	exit 3
fi
echo "error: launch gate failed — process exited before producing a thread id" >&2
echo "--- stderr head:" >&2
head -c 800 "$RUN_DIR/stderr.log" >&2 || true
exit 1
