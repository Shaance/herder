#!/usr/bin/env bash
# Post-run evidence collector: one readout of what a delegated run actually
# did to the tree, including mechanical scope-violation detection. Run the
# executor's own status script first; this script only reads the tree.
# Usage: collect_evidence.sh <run-dir> <repo> [authorized-path-prefix...]
set -euo pipefail

RUN_DIR=${1:?usage: collect_evidence.sh <run-dir> <repo> [authorized-prefix...]}
REPO=${2:?missing repo}
shift 2 || true

echo "--- final.md: $([[ -s "$RUN_DIR/final.md" ]] && wc -c <"$RUN_DIR/final.md" | tr -d ' ' || echo 0) bytes"

cd "$REPO"
echo "--- worktree status (incl. untracked):"
git status --short --untracked-files=all | head -40
echo "--- diff stat:"
git diff --stat | tail -5

if (( $# > 0 )); then
	echo "--- scope check against: $*"
	# --no-renames lists a rename as its deleted source and its added
	# destination, so both sides are scope-checked; one path per line.
	violations=$({ git diff --name-only --no-renames HEAD; git ls-files --others --exclude-standard; } | sort -u | while IFS= read -r f; do
		ok=false
		for p in "$@"; do [[ "$f" == "$p"* ]] && ok=true && break; done
		$ok || echo "$f"
	done)
	if [[ -n "$violations" ]]; then
		echo "SCOPE VIOLATIONS (touched outside authorized paths):"
		echo "$violations"
		exit 2
	fi
	echo "scope: clean — every touched file is inside the authorized paths"
fi
