#!/usr/bin/env bash
set -eu

while [[ "$1" != -o ]]; do
    shift
done
report=$2
run=$(dirname "$report")
printf '%s\n' "$run" >> "$HERDER_STATE_DIR/worker-launches"
cat > "$run/worker-stdin"

case "$HERDER_TEST_WORKER" in
    fail)
        echo 'worker refused startup' >&2
        exit 1
        ;;
    hold)
        for _ in $(seq 1 20); do
            [[ ! -f "$HERDER_STATE_DIR/release-worker" ]] || break
            sleep 1
        done
        [[ -f "$HERDER_STATE_DIR/release-worker" ]]
        ;;
    implement)
        mkdir -p src
        printf 'fixture change\n' > src/implemented.txt
        ;;
esac

printf 'A finding from the recorded review.\n' > "$report"
printf '{"type":"thread.started","thread_id":"fixture-thread"}\n'
if [[ "$HERDER_TEST_WORKER" == partial ]]; then
    printf '{"type":"turn.failed"}\n'
elif [[ "$HERDER_TEST_WORKER" != incomplete ]]; then
    printf '{"type":"turn.completed"}\n'
fi
