#!/usr/bin/env bash
# Usage: status_agy.sh <run-dir>
set -euo pipefail

RUN_DIR=${1:?usage: status_agy.sh <run-dir>}
EVENTS="$RUN_DIR/events.jsonl"
IDENTITY="$RUN_DIR/conversation-id"
FINAL="$RUN_DIR/final.md"

PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || true)
if [[ "$PID" =~ ^[1-9][0-9]*$ ]] && kill -0 "$PID" 2>/dev/null \
	&& [[ " $(ps -p "$PID" -o args= 2>/dev/null) " == *" $RUN_DIR/agy.log "* ]]; then
	STATE=alive
else
	STATE=exited
fi

if [[ ! -s "$IDENTITY" ]]; then
	LATE_ID=$(head -n 1 "$EVENTS" 2>/dev/null \
		| jq -r 'select(.event == "init") | .conversation_id | strings' 2>/dev/null || true)
	[[ -z "$LATE_ID" ]] || printf '%s\n' "$LATE_ID" >"$IDENTITY"
fi
AGE=$(perl -e 'my @s = stat shift or exit 1; print time - $s[9]' "$EVENTS" 2>/dev/null || echo none)
echo "state=$STATE pid=${PID:-none} conversation=$(cat "$IDENTITY" 2>/dev/null || echo none) stream_age_s=$AGE"

if [[ "$STATE" == alive ]]; then
	echo "verdict: RUNNING"
	exit 0
fi

RESULT=$(tail -n 1 "$EVENTS" 2>/dev/null | jq -ce 'select(.event == "result") | .result' 2>/dev/null || true)
if [[ -z "$RESULT" ]]; then
	echo "verdict: DIED (no result event)"
	tail -n 5 "$RUN_DIR/stderr.log" 2>/dev/null || true
	exit 0
fi

DENIED=$(jq -r '.denied_actions // [] | map(.display_name) | join(", ")' <<<"$RESULT")
STATUS=$(jq -r '.status' <<<"$RESULT")
ERROR=$(jq -r '.error // empty' <<<"$RESULT")
if jq -e '(.denied_actions // []) | length > 0' <<<"$RESULT" >/dev/null; then
	echo "verdict: FAILED (denied actions: $DENIED)"
elif [[ "$STATUS" == ERROR && "$ERROR" == interrupted ]]; then
	echo "verdict: STOPPED (interrupted)"
elif [[ "$STATUS" == ERROR ]]; then
	echo "verdict: FAILED (${ERROR:-no error message})"
elif [[ "$STATUS" == SUCCESS ]] && jq -e '.response | strings | length > 0' <<<"$RESULT" >/dev/null; then
	TEMP=$(mktemp "$FINAL.XXXXXX")
	trap 'rm -f "$TEMP"' EXIT
	jq -jr '.response' <<<"$RESULT" >"$TEMP"
	mv -f "$TEMP" "$FINAL"
	echo "verdict: COMPLETED"
else
	echo "verdict: FAILED (result status $STATUS or empty response)"
fi
