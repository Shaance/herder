---
name: crew
description: Use when coordinating other agents in Rex or asked to dispatch, track, read reports from, or verify agent runs across executors.
---

## When to use

Use crew to dispatch Claude, Vibe or Codex workers and keep their run identities,
evidence and verification results together. Any coordinating
agent or person can use the same CLI.

## The loop

1. Run `crew doctor` for the intended executor and profile.
2. Keep briefs outside `--repo`; an untracked brief inside it fails verify scope. Use
   implementer-core's direct-mode contract blocks, scope and checks. Name every skill the worker
   must load (for example `$write-rust`); workers do not reliably load them unasked.
3. Use `crew dispatch`; retain the printed run ID, name and optional goal label.
   Status, report, stop, resume, verify and wait also accept a unique dispatch
   `--name`; existing IDs take precedence; ambiguous or unknown names list matches or similar names.
   Dispatch defaults to `--mode direct`; use `--mode review` for read-only briefs.
   Vibe/Codex resume replays the recorded model and extra arguments after `--`.
4. Follow progress with `crew ls` (models and executor state tallies) and `crew status`.
   Use `crew wait <run>...` for any settled run, or `--all` for all runs;
   `--timeout <secs>` defaults to 3600. It polls every 5 seconds, exits 0 for
   finished/failed/stopped/blocked, and exits 1 for unknown or timeout. Final output
   lists every run and report path. Example: `crew wait run-id --timeout 60`.
   Vibe/Codex `--pane` uses wait and closes on exit. Read `crew report`.
5. Inspect the actual changes and run `crew verify` with scope paths and checks.
   After rebasing, use `crew verify <run> --baseline <new-upstream-rev>` to replace
   the recorded scope baseline. Invalid commit revisions leave the run unchanged.
   Resolve each review finding or record its approved deferral.
6. The coordinator handles approval and publication outside crew.

For paired read-only reviews, write one brief containing `task`, `action_safety`,
`default_follow_through_policy` and `structured_output_contract`. Dispatch it twice
with the same goal; select a Codex model and let Vibe use its configured default:

```sh
crew dispatch --mode review --executor codex --model <codex-model> --brief /tmp/review.md --repo . --goal paired-review --name codex-review
crew dispatch --mode review --executor vibe --brief /tmp/review.md --repo . --goal paired-review --name vibe-review
crew wait <id1> <id2> --all
```

Use the two printed run IDs for `<id1>` and `<id2>`, then read both reports.
`--mode` selects brief linting only; the brief must enforce read-only behavior.

## Rules

- Route work per AGENT_DEFAULTS; crew never selects an executor. Vibe runs are auto-approve only.
- A worker report is never proof. Verify the actual output independently.
- Publish only after approval; crew does not publish or grant approval.
- Treat `unknown` as unresolved evidence, not completion or permission to retry.
- Preserve recorded identities and profiles. Consult command help for stop,
  resume, dispatch's `--pane` flag and executor limitations.
- When something surprises you, file it with `gh issue create -R Shaance/crew` and continue with work that
  remains safe and understood. Do not paper over an executor disagreement.

All flag details and examples live in `crew <command> --help`.
