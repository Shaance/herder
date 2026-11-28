#!/usr/bin/env bash
# Usage: status_agy.sh <run-dir>
set -euo pipefail

RUN_DIR=${1:?usage: status_agy.sh <run-dir>}
EVENTS="$RUN_DIR/events.jsonl"
IDENTITY="$RUN_DIR/conversation-id"
FINAL="$RUN_DIR/final.md"

PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || true)
STATE=exited
if [[ "$PID" =~ ^[1-9][0-9]*$ ]] && kill -0 "$PID" 2>/dev/null; then
	if ! ARGS=$(ps -p "$PID" -o args= 2>/dev/null); then
		STATE=unknown
	elif [[ " $ARGS " == *" $RUN_DIR/agy.log "* ]]; then
		STATE=alive
	fi
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
elif [[ "$STATE" == unknown ]]; then
	echo "verdict: UNKNOWN (ps failed to read process arguments)"
	exit 0
fi

LAST_LINE=$(tail -n 1 "$EVENTS" 2>/dev/null || true)
VERDICT=$(jq -Rr '
	fromjson? | objects | select(.event == "result") | .result
	| (.denied_actions // []) as $denied
	| if ($denied | length) > 0 then
		"FAILED (denied actions: \($denied | map(.display_name) | join(", ") | sub("\n+$"; "")))"
	  elif .status == "ERROR" then
		"ERROR", (.error // "")
	  elif .status == "SUCCESS" and ((.response | strings | length) // 0) > 0 then
		"COMPLETED"
	  else "FAILED (result status \(.status) or empty response)" end
' <<<"$LAST_LINE")
if [[ -z "$VERDICT" ]]; then
	echo "verdict: DIED (no result event)"
	tail -n 5 "$RUN_DIR/stderr.log" 2>/dev/null || true
	exit 0
fi

case "$VERDICT" in
ERROR$'\n'interrupted)
	VERDICT='STOPPED (interrupted)';;
ERROR*)
	ERROR=${VERDICT#ERROR}
	ERROR=${ERROR#$'\n'}
	VERDICT="FAILED (${ERROR:-no error message})";;
COMPLETED)
	TEMP=$(mktemp "$FINAL.XXXXXX")
	trap 'rm -f "$TEMP"' EXIT
	jq -jr '.result.response' <<<"$LAST_LINE" >"$TEMP"
	mv -f "$TEMP" "$FINAL";;
esac
echo "verdict: $VERDICT"
