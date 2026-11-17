#!/usr/bin/env bash
# Liveness and outcome readout for a run launched by launch_vibe.sh or
# resume_vibe.sh. Authority order: pid, then live stream growth, then
# out*/export.json. On exit it also writes final*.md from the last assistant
# message and checks the effective model route against the expected thinking level.
# Usage: status_vibe.sh <run-dir> [suffix]
#   VIBE_IMPLEMENTER_THINKING  expected thinking level (default: high)
set -euo pipefail

RUN_DIR=${1:?usage: status_vibe.sh <run-dir> [suffix]}
SUFFIX=${2:-}
EVENTS="$RUN_DIR/events${SUFFIX}.jsonl"
EXPORT="$RUN_DIR/out${SUFFIX}/export.json"
FINAL="$RUN_DIR/final${SUFFIX}.md"
EXPECT=${VIBE_IMPLEMENTER_THINKING:-high}

PID=$(cat "$RUN_DIR/run.pid" 2>/dev/null || echo "")
if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null \
	&& [[ " $(ps -p "$PID" -o args= 2>/dev/null) " == *" $RUN_DIR/out${SUFFIX} "* ]]; then
	STATE=alive
else
	STATE=exited
fi
EV=$(wc -c <"$EVENTS" 2>/dev/null | tr -d ' ' || echo 0)
# Seconds since the stream last changed; one readout replaces two growth polls.
AGE=$(perl -e 'my @s = stat shift or exit 1; print time - $s[9]' "$EVENTS" 2>/dev/null || echo none)
if [[ ! -s "$RUN_DIR/session-id" ]]; then
	LATE_ID=$(head -1 "$RUN_DIR/events.jsonl" 2>/dev/null | jq -r '.sessionId // empty' 2>/dev/null || true)
	[[ -n "$LATE_ID" ]] && printf '%s\n' "$LATE_ID" >"$RUN_DIR/session-id"
fi
DENIED=$(jq -r 'select(.type=="callback" and .detail.kind=="approval") | .title' "$EVENTS" 2>/dev/null | sort | uniq -c | tr '\n' ';' || true)

echo "state=$STATE pid=${PID:-none} session=$(cat "$RUN_DIR/session-id" 2>/dev/null || echo none) events_bytes=$EV stream_age_s=$AGE"
[[ -n "$DENIED" ]] && echo "denied approvals (inspect the brief and Vibe configuration): $DENIED"

if [[ "$STATE" == alive ]]; then
	echo "verdict: RUNNING — do not kill; re-check stream growth before calling it stalled"
	exit 0
fi
if [[ ! -s "$EXPORT" ]]; then
	echo "verdict: DIED — no export.json; inspect stderr and the worktree before retrying"
	grep -v 'macOS Keychain' "$RUN_DIR/stderr${SUFFIX}.log" 2>/dev/null | tail -5 || true
	exit 0
fi

jq -rs '[.[] | select(.type=="message" and .role=="assistant")] | last
	| [.content[]? | select(.type=="text") | .text] | join("\n")' "$EVENTS" >"$FINAL" 2>/dev/null || true

OUTCOME=$(jq -r '.outcome' "$EXPORT")
ROUTE=$(jq -r '.config.active_model as $a | .config.models[$a] // {} | "\($a) name=\(.name) thinking=\(.thinking)"' "$EXPORT")
THINKING=$(jq -r '.config.active_model as $a | .config.models[$a].thinking // "unknown"' "$EXPORT")
MODE=$(cat "$RUN_DIR/execution-mode" 2>/dev/null || echo unknown)
echo "outcome=$OUTCOME route=$ROUTE mode=$MODE configured_default_agent=$(jq -r '.config.default_agent' "$EXPORT") steps=$(jq -r '.steps' "$EXPORT") tokens=$(jq -r '.usage.total_tokens' "$EXPORT") cost_usd=$(jq -r '.cost_usd' "$EXPORT")"
[[ "$THINKING" == "$EXPECT" ]] || echo "ROUTE MISMATCH: effective thinking=$THINKING, expected $EXPECT; do not report this run as $EXPECT"
echo "final report: $FINAL ($(wc -c <"$FINAL" | tr -d ' ') bytes)"

case $OUTCOME in
finished) echo "verdict: COMPLETED — read the final report and the diff" ;;
turn_limit | token_limit | price_limit | deadline | terminated | length)
	echo "verdict: STOPPED ($OUTCOME) — resumable; inspect the tree, then resume_vibe.sh with a delta" ;;
*)
	echo "verdict: FAILED ($OUTCOME) — $(jq -r '.error.message // "no error message"' "$EXPORT" | head -c 300)" ;;
esac
