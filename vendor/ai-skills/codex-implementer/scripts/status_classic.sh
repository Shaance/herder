#!/usr/bin/env bash
# Liveness readout for a classic run launched by launch_classic.sh.
# Authority order: pid, then live stream growth, then artifacts.
# Usage: status_classic.sh <run-dir>
set -euo pipefail

RUN_DIR=${1:?usage: status_classic.sh <run-dir>}
PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || echo "")

# Recover acceptance if the thread started after the launch gate elapsed.
if [[ ! -s "$RUN_DIR/thread-id" && -s "$RUN_DIR/events.jsonl" ]]; then
	while IFS= read -r LINE; do
		THREAD_ID=$(jq -r 'select(.type=="thread.started") | .thread_id // empty' <<<"$LINE")
		if [[ -n "$THREAD_ID" ]]; then
			printf '%s\n' "$THREAD_ID" >"$RUN_DIR/thread-id"
			break
		fi
	done <"$RUN_DIR/events.jsonl"
fi

# Resumes share run.pid and create eventsN.jsonl before emitting events.
SUFFIX=
ATTEMPT=1
for EVENTS in "$RUN_DIR"/events*.jsonl; do
	[[ -f "$EVENTS" ]] || continue
	N=${EVENTS##*/events}
	N=${N%.jsonl}
	if [[ "$N" =~ ^[0-9]+$ ]] && (( 10#$N > ATTEMPT )); then
		ATTEMPT=$((10#$N))
		SUFFIX=$N
	fi
done

if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null \
	&& [[ " $(ps -p "$PID" -o args= 2>/dev/null) " == *" $RUN_DIR/final${SUFFIX}.md "* ]]; then
	STATE=alive
else
	STATE=exited
fi

EV=$(wc -c <"$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null || echo 0)
SE=$(wc -c <"$RUN_DIR/stderr${SUFFIX}.log" 2>/dev/null || echo 0)
# Seconds since the stream last changed; one readout replaces two growth polls.
AGE=$(perl -e 'my @s = stat shift or exit 1; print time - $s[9]' "$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null || echo none)
# A started command_execution item without its completed item is still running.
# Events carry no timestamps, so its duration is not known here.
RUNNING=
if [[ "$STATE" == alive ]]; then
	RUNNING=$(jq -Rrs 'split("\n") | map(fromjson? | select(.item.type == "command_execution"))
		| reduce .[] as $e ({}; if $e.type == "item.started" then .[$e.item.id] = $e.item.command
			elif $e.type == "item.completed" then del(.[$e.item.id]) else . end)
		| to_entries | last | .value // empty' "$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null | head -c 300 | tr '\n' ' ' || true)
fi
FINAL=absent
[[ -s "$RUN_DIR/final${SUFFIX}.md" ]] && FINAL=present
LAST_EVENT=$(tail -1 "$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null \
	| jq -r '.type + " " + (.item.type // "")' 2>/dev/null || echo "unparsed")
# Ignore an in-flight partial record and use the latest turn in this attempt.
LAST_TURN=$(jq -Rrs 'split("\n")[:-1] | map(fromjson? | select(.type |
	IN("turn.started", "turn.completed", "turn.failed"))) | last | .type // empty' \
	"$RUN_DIR/events${SUFFIX}.jsonl" 2>/dev/null || true)

echo "state=$STATE pid=${PID:-none} events_bytes=$EV stderr_bytes=$SE final=$FINAL last_event='$LAST_EVENT' stream_age_s=$AGE"
[[ -n "$RUNNING" ]] && echo "running_command: $RUNNING"
if [[ "$LAST_TURN" == turn.failed ]]; then
	echo "verdict: FAILED — inspect the turn.failed event and stderr.log before retrying"
elif [[ "$STATE" == alive && "$FINAL" == absent ]]; then
	echo "verdict: RUNNING — do not kill; re-check growth before calling it stalled"
elif [[ "$STATE" == exited && "$FINAL" == present ]]; then
	echo "verdict: COMPLETED — read final.md and the diff"
elif [[ "$STATE" == exited && "$FINAL" == absent ]]; then
	echo "verdict: DIED — inspect stderr.log tail and the worktree before retrying"
fi
