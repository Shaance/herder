---
name: crew
description: Use when coordinating other agents or asked to dispatch, track, read reports from, or verify agent runs across executors.
---

## When to use

Use crew to dispatch Claude, Vibe or Codex workers and keep their run identities,
evidence and verification results together. Any coordinating
agent or person can use the same CLI.

## The loop

1. Run `crew doctor` for the intended executor and profile.
2. Prefer briefs outside `--repo`; files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. Direct-mode briefs require
   `<task>`, `<workstream_contract>`, `<implementation_authority>`, `<action_safety>`,
   `<default_follow_through_policy>`, `<verification_loop>`, `<economy_contract>` and
   `<structured_output_contract>`. Review-mode briefs require `<task>`, `<action_safety>`,
   `<default_follow_through_policy>` and `<structured_output_contract>`. Include scope and checks.
   Name every skill the worker must load (for example `$write-rust`); workers do not reliably load them unasked.
3. Use `crew dispatch`; retain the printed run ID, name and optional goal label.
   Status, report, stop, resume, verify and wait also accept a unique dispatch
   `--name`; existing IDs take precedence; ambiguous or unknown names list matches or similar names.
   Dispatch defaults to `--mode direct`; use `--mode review` for read-only briefs.
   Vibe/Codex resume replays the recorded model and extra arguments after `--`.
Claude resume stops a settled live session, then wakes the same identity with its
saved options under the recorded profile; copies are stopped and rejected.
4. Follow progress with `crew ls` (models and executor state tallies) and `crew status`.
   Use `crew status <run> --tail <N>` (1–1000) for the current launch attempt's last N stream lines capped at 500 characters each (`data.tail` in JSON), with a message for missing streams or Claude runs.
   Vibe/Codex status evidence carries `stream_age_s` (seconds since the stream last
   changed) and, for a live Codex run, `running_command:`.
   Use `crew wait <run>...` for any settled run, or `--all` for all runs;
   `--timeout <secs>` defaults to 3600. It polls every 5 seconds, exits 0 for
   finished/failed/stopped/blocked, and exits 1 for unknown or timeout. Final output
   lists every run and report path. Example: `crew wait run-id --timeout 60`.
   When Rex is available, `--pane` opens a pane; Vibe/Codex panes use `crew wait`
   and close on exit. Read `crew report`.
5. Inspect the actual changes and run `crew verify` with scope paths and checks.
   After rebasing, use `crew verify <run> --baseline <new-upstream-rev>` to replace
   the recorded scope baseline. Invalid commit revisions leave the run unchanged.
   Resolve each review finding or record its approved deferral.
6. The coordinator handles approval and publication outside crew.

For paired read-only reviews, write one brief containing `task`, `action_safety`,
`default_follow_through_policy` and `structured_output_contract`. Dispatch it to each
executor with the same goal; select a Codex model, and repeat `--model` to review with several
Vibe models (use the review models your host instructions name):

```sh
crew dispatch --mode review --executor codex --model <codex-model> --brief /tmp/review.md --repo . --goal paired-review --name codex-review
crew dispatch --mode review --executor vibe --model <vibe-model-1> --model <vibe-model-2> --brief /tmp/review.md --repo . --goal paired-review --name vibe-review
crew wait <id1> <id2> <id3> --all
```

A repeated `--model` launches one recorded run per model, named `<name>-<model>`, and
prints every run ID. It is refused outside `--mode review` and for handoffs, because
one worktree has one writer, and when a model repeats, because both runs would share
one name. If a later launch fails, the runs already started stay listed beside the
error. Wait on all printed run IDs, then read every report.
`--mode` selects brief linting only; the brief must enforce read-only behavior.

## Handing off to another agent

When the coordinating conversation moves to another agent, write the continuation
context (decisions, constraints, remaining work, explicit authorizations) into a
plain UTF-8 file, stop editing the task and repository, then run
`crew handoff --executor claude --brief /tmp/HANDOFF.md --repo . [dispatch options]`.
Handoff reuses dispatch's launch, store and optional `--pane` (Claude attach in Rex), but
accepts a plain continuation document: its full bytes are snapshotted into the
run's `input.md` behind the standard continuation contract and linted like any
brief, so later edits to the file cannot change what the worker receives. It
launches a continuation only: it does not transfer hidden conversation, stop
another writer, or prove the context was understood, and
the launch result is launch evidence only. Invalid documents fail before launch
with no run record. `--mode review` instructs the receiver to stay read-only; it does not add an OS
sandbox. No delegation or publication unless the document or the user expressly
authorized it.

## Rules

- Choosing the executor and model is the coordinator's job; crew never selects one. Vibe runs are auto-approve only.
- A worker report is never proof. Verify the actual output independently.
- Publish only after approval; crew does not publish or grant approval.
- Treat `unknown` as unresolved evidence, not completion or permission to retry.
  Its JSON `cause` and `crew status` cause line distinguish `query_failed`, `identity_mismatch`,
  `record_disagrees`, `unrecognized_status`, and `unconfirmed`; other states omit the cause.
- Preserve recorded identities and profiles. Consult command help for stop,
  resume, dispatch's `--pane` flag and executor limitations.
- When something surprises you, file it with `gh issue create -R Shaance/crew` and continue with work that
  remains safe and understood. Do not paper over an executor disagreement.

All flag details and examples live in `crew <command> --help`.
