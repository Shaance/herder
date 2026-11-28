#!/usr/bin/env bash
# Usage: launch_agy.sh <repo> <brief-file> <run-dir> --model <slug> [--mode plan]
# Exit: 0 identity confirmed, 1 launch failed, 3 gate elapsed with the process still alive.
set -euo pipefail

REPO=${1:?missing repo}
BRIEF=${2:?missing brief file}
RUN_DIR=${3:?missing run directory}
[[ ${4:-} == --model && -n ${5:-} ]] || { echo "error: agy requires --model <slug>" >&2; exit 1; }
MODEL=$5
shift 5

FLAGS=(--dangerously-skip-permissions)
if [[ $# -eq 2 && $1 == --mode && $2 == plan ]]; then
	FLAGS=(--mode plan "${FLAGS[@]}")
elif [[ $# -ne 0 ]]; then
	echo "error: unexpected agy launch arguments" >&2
	exit 1
fi

[[ -d "$REPO" ]] || { echo "error: repo '$REPO' is not a directory" >&2; exit 1; }
[[ -s "$BRIEF" ]] || { echo "error: brief '$BRIEF' is missing or empty" >&2; exit 1; }
[[ "$RUN_DIR" == /* ]] || RUN_DIR="$PWD/$RUN_DIR"
[[ ! -e "$RUN_DIR/run.pid" ]] || { echo "error: $RUN_DIR already holds a run" >&2; exit 1; }
# Preserve the brief's trailing newlines across command substitution.
PROMPT=$(cat "$BRIEF" && printf '.')
cd "$REPO"

command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }
command -v agy >/dev/null || { echo "error: agy CLI not on PATH" >&2; exit 1; }

SETTINGS=${HOME:-}/.gemini/antigravity-cli/settings.json
# jq accepts non-JSON values such as NaN. Require a valid settings document.
if [[ -z ${HOME:-} ]] || ! perl -MJSON::PP -0777 -e '
	my $settings = decode_json(<STDIN>);
	exit !(ref($settings) eq "HASH" && ($settings->{modelProvider} // "") eq "gemini");
' 2>/dev/null <"$SETTINGS"; then
	echo 'error: set "modelProvider": "gemini" in ~/.gemini/antigravity-cli/settings.json to use API-key mode' >&2
	exit 1
fi

mkdir -p "$RUN_DIR"

# Retain the PID across exec and give the detached worker its own session.
nohup perl -MPOSIX -e 'POSIX::setsid() >= 0 or die "setsid: $!"; exec @ARGV or die "exec: $!";' -- \
	agy --log-file "$RUN_DIR/agy.log" --model "$MODEL" --output-format stream-json \
	"${FLAGS[@]}" -p "${PROMPT%.}" </dev/null >"$RUN_DIR/events.jsonl" 2>"$RUN_DIR/stderr.log" &
PID=$!
printf '%s\n' "$PID" >"$RUN_DIR/run.pid"

for ((elapsed = 0; elapsed <= 30; elapsed++)); do
	ALIVE=false
	if kill -0 "$PID" 2>/dev/null; then ALIVE=true; fi
	CONVERSATION_ID=$(head -n 1 "$RUN_DIR/events.jsonl" 2>/dev/null \
		| jq -r 'select(.event == "init") | .conversation_id | strings' 2>/dev/null || true)
	if [[ -n "$CONVERSATION_ID" ]]; then
		printf '%s\n' "$CONVERSATION_ID" >"$RUN_DIR/conversation-id"
		echo "RUN_DIR=$RUN_DIR PID=$PID CONVERSATION_ID=$CONVERSATION_ID"
		exit 0
	fi
	"$ALIVE" || break
	[[ $elapsed -eq 30 ]] || sleep 1
done

if "$ALIVE"; then
	echo "warn: gate window elapsed with agy alive; re-check with status_agy.sh $RUN_DIR" >&2
	exit 3
fi
echo "error: launch gate failed; agy exited before streaming a conversation id" >&2
head -c 800 "$RUN_DIR/stderr.log" >&2
exit 1
