#!/usr/bin/env bash
# Brief linter: refuse a delegated-implementer launch (any executor) when
# required contract blocks are missing, instead of discovering the gap mid-run.
# Usage: check_brief.sh <brief-file> [direct|lead|review]
# review: read-only review or audit briefs need only task, action_safety,
# default_follow_through_policy and structured_output_contract.
set -euo pipefail

BRIEF=${1:?usage: check_brief.sh <brief-file> [direct|lead|review]}
MODE=${2:-direct}
[[ "$MODE" =~ ^(direct|lead|review)$ ]] || { echo "error: mode must be direct, lead or review" >&2; exit 2; }
[[ -s "$BRIEF" ]] || { echo "error: brief '$BRIEF' missing or empty" >&2; exit 1; }

if [[ "$MODE" == review ]]; then
	required=(task action_safety default_follow_through_policy structured_output_contract)
else
	required=(task workstream_contract implementation_authority action_safety
		default_follow_through_policy verification_loop economy_contract
		structured_output_contract)
fi
[[ "$MODE" == lead ]] && required+=(delegation_policy)

missing=()
for block in "${required[@]}"; do
	grep -q "<${block}>" "$BRIEF" || missing+=("$block")
done

warnings=()
[[ "$MODE" == review ]] || grep -qiE "authorized paths?" "$BRIEF" || warnings+=("no 'authorized paths' statement found")
grep -qiE "do not (commit|push)" "$BRIEF" || warnings+=("action_safety does not forbid commit/push")
if [[ "$MODE" != lead ]] && grep -q "<delegation_policy>" "$BRIEF"; then
	warnings+=("$MODE-mode brief contains <delegation_policy>; remove it")
fi

for w in "${warnings[@]:-}"; do [[ -n "$w" ]] && echo "warn: $w"; done
if (( ${#missing[@]} > 0 )); then
	echo "FAIL: missing required blocks: ${missing[*]}" >&2
	exit 1
fi

# A brief that adds or names a public surface (route, field, artifact, module,
# schema, flag) must record the existing home it rejected, or state that no new
# surface exists. Placement is then a written decision, not an analogy to a
# recent precedent. Escape hatch: "Rejected alternative: none, no new surface".
surface_words='(^|[^[:alnum:]_])new (public )?(endpoint|route|api|field|column|table|artifact|module|package|schema|type|flag|output)s?([^[:alnum:]_]|$)|(^|[^[:alnum:]_])(add|adds|added|adding|create|creates|introduce|introduces|expose|exposes)([^.]{0,80})(endpoint|route|field|artifact|module|table|column|schema|flag)s?([^[:alnum:]_]|$)'
surface_paths='(GET|POST|PUT|PATCH|DELETE) /'
surface_hit=
if [[ "$MODE" != review ]]; then
	surface_hit=$(grep -inE "$surface_words" "$BRIEF" | head -n1 || true)
	[[ -n "$surface_hit" ]] || surface_hit=$(grep -nE "$surface_paths" "$BRIEF" | head -n1 || true)
fi
if [[ -n "$surface_hit" ]] && ! grep -qi 'Rejected alternative:' "$BRIEF"; then
	echo "FAIL: brief adds or names a public surface but has no 'Rejected alternative:' line" >&2
	echo "      first match (line:text): ${surface_hit:0:160}" >&2
	echo "      Name the existing home you rejected and why, or write 'Rejected alternative: none, no new surface'." >&2
	exit 1
fi
echo "OK: brief has all required $MODE-mode blocks (${#warnings[@]} warning(s))"
