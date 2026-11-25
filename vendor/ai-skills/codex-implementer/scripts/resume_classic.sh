#!/usr/bin/env bash
# Canonical detached resume of a classic run: global flags BEFORE `resume`
# (the arg-order mistake this script exists to prevent), enforced --json,
# suffixed evidence files, and the same launch gate as launch_classic.sh.
# Usage: resume_classic.sh <run-dir> <repo> <delta-brief> [suffix] [-- extra codex args]
# Extra args (e.g. --model, -c key=value, --add-dir) go before `resume`, the
# same slot launch_classic.sh uses, so a resumed attempt keeps attempt 1's route.
set -euo pipefail

RUN_DIR=${1:?usage: resume_classic.sh <run-dir> <repo> <delta-brief> [suffix] [-- extra args]}
REPO=${2:?missing repo}
DELTA=${3:?missing delta brief}
shift 3
SUFFIX=2
if [[ $# -gt 0 && $1 != --* ]]; then SUFFIX=$1; shift; fi
if [[ $# -gt 0 ]]; then
	[[ $1 == -- ]] || { echo "error: unexpected argument '$1' (extra codex args go after --)" >&2; exit 1; }
	shift
fi
# Same sandbox rule as launch_classic.sh: a replayed caller sandbox replaces the default.
case " $* " in
*" --sandbox "* | *" --sandbox="* | *" -s "* | *" --dangerously-bypass-approvals-and-sandbox "*) ;;
*) set -- --sandbox workspace-write "$@" ;;
esac

THREAD_ID=$(cat "$RUN_DIR/thread-id" 2>/dev/null || true)
[[ -n "$THREAD_ID" ]] || { echo "error: no thread-id in $RUN_DIR" >&2; exit 1; }

# Single-writer guard: a live pid in run.pid means the thread already has an
# active writer; a second resume would die on thread-store conflict and
# clobber this suffix's evidence files.
OLD_PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || true)
if [[ -n "$OLD_PID" ]] && kill -0 "$OLD_PID" 2>/dev/null; then
	echo "error: run.pid $OLD_PID is still alive — refusing a second writer" >&2
	exit 4
fi
[[ -s "$DELTA" ]] || { echo "error: delta brief missing or empty" >&2; exit 1; }
cp "$DELTA" "$RUN_DIR/delta${SUFFIX}.md"

# Keep the recorded pid across exec, outside the coordinator's process group.
# The wrapper records its own pid, which exec keeps, before Codex starts, so a
# launcher killed right after the fork cannot leave a live worker unrecorded.
nohup perl -MPOSIX -e 'my $pid_file = shift; POSIX::setsid() >= 0 or die "setsid: $!"; open(my $f, ">", $pid_file) or die "run.pid: $!"; print $f "$$\n" or die "run.pid: $!"; close($f) or die "run.pid: $!"; exec @ARGV or die "exec: $!";' -- "$RUN_DIR/run.pid" \
	codex exec -C "$REPO" --json \
	"$@" \
	-o "$RUN_DIR/final${SUFFIX}.md" \
	resume "$THREAD_ID" - <"$RUN_DIR/delta${SUFFIX}.md" \
	>"$RUN_DIR/events${SUFFIX}.jsonl" 2>"$RUN_DIR/stderr${SUFFIX}.log" &
PID=$!

for _ in $(seq 1 90); do
	[[ -s "$RUN_DIR/events${SUFFIX}.jsonl" ]] && {
		echo "RUN_DIR=$RUN_DIR PID=$PID THREAD_ID=$THREAD_ID SUFFIX=$SUFFIX"
		exit 0
	}
	kill -0 "$PID" 2>/dev/null || break
	sleep 1
done
if kill -0 "$PID" 2>/dev/null; then
	echo "warn: gate window elapsed with the process ALIVE; do NOT kill —" >&2
	echo "re-check with status_classic.sh $RUN_DIR" >&2
	exit 3
fi
echo "error: resume launch gate failed — process exited early" >&2
head -c 800 "$RUN_DIR/stderr${SUFFIX}.log" >&2 || true
exit 1
