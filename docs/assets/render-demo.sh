#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d "${TMPDIR%/}/herder-demo.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
export HERDER_STATE_DIR="$scratch/state" DEMO_REPO="$scratch/repo"
export PATH="$scratch/bin:$PATH" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_DATE=2026-01-01T00:00:00Z GIT_COMMITTER_DATE=2026-01-01T00:00:00Z
unset HERDER_SKILLS_DIR NO_COLOR
mkdir -p "$scratch/bin" "$DEMO_REPO/src"
ln -s "$root/target/debug/herder" "$scratch/bin/herder"
cat > "$scratch/bin/codex" <<'STUB'
#!/bin/sh
set -eu
brief=$(cat)
while [ "$#" -gt 0 ]; do
  case "$1" in -C) repo=$2; shift 2;; -o) final=$2; shift 2;; *) shift;; esac
done
echo '{"type":"thread.started","thread_id":"demo"}'
case "$brief" in *CRASH*) echo '{"type":"turn.failed"}'; exit 1;; esac
echo 'changed = true' >> "$repo/config.toml"
echo 'Task completed.' > "$final"
echo '{"type":"turn.completed"}'
STUB
chmod +x "$scratch/bin/codex"
git init -q "$DEMO_REPO"
echo 'enabled = true' > "$DEMO_REPO/config.toml"
echo '// demo' > "$DEMO_REPO/src/lib.rs"
git -C "$DEMO_REPO" add config.toml src/lib.rs
git -C "$DEMO_REPO" -c user.name=Demo -c user.email=demo@example.test -c commit.gpgsign=false commit -qm initial
cat > "$scratch/brief" <<'BRIEF'
<task>Update the module.</task><workstream_contract>Work alone.</workstream_contract>
<implementation_authority>Authorized paths: src/.</implementation_authority><action_safety>Do not commit or push.</action_safety>
<verification_loop>Run checks.</verification_loop>
<default_follow_through_policy>Finish.</default_follow_through_policy>
<economy_contract>Keep it small.</economy_contract><structured_output_contract>Report changes.</structured_output_contract>
BRIEF
herder dispatch --executor codex --brief "$scratch/brief" --repo "$DEMO_REPO" --name outside-src --path src/ > /dev/null
echo CRASH >> "$scratch/brief"
herder dispatch --executor codex --brief "$scratch/brief" --repo "$DEMO_REPO" --name crashed --path src/ > /dev/null
herder wait outside-src crashed --all --timeout 30 > /dev/null
herder status outside-src --json | jq -e '.data.observation.state == "finished"' > /dev/null
herder status crashed --json | jq -e '.data.observation.state == "failed"' > /dev/null
if herder verify outside-src --check true --json > "$scratch/verify.json"; then exit 1; fi
jq -e '.data.run.verifications[-1].checks | .[0].exit_code == 1 and
  (.[0].output | contains("config.toml")) and .[1].exit_code == 0 and .[2].exit_code == 0' "$scratch/verify.json" > /dev/null
cat > "$scratch/capture.sh" <<'CAPTURE'
#!/usr/bin/env bash
set -euo pipefail
cd "$DEMO_REPO"
printf '$ herder verify outside-src --check '\''true'\''\n'
status=0; herder verify outside-src --check 'true' || status=$?
[[ $status == 1 ]]
printf '\n$ herder ls\n'
herder ls
CAPTURE
cd "$scratch"
for theme in dark light; do
  if [[ $theme == dark ]]; then palette=dracula; background='#222530'
  else palette=github; background='#f5f6f8'; fi
  freeze --config base --execute 'bash ./capture.sh' --output "$root/docs/assets/demo-$theme.png" \
    --theme "$palette" --background "$background" --window --width 1600 \
    --padding 48,56 --margin 40 --border.radius 24 --shadow.blur 24 --shadow.y 10 \
    --font.family 'JetBrains Mono' --font.size 28 --line-height 1.3
  chmod 644 "$root/docs/assets/demo-$theme.png"
done
