---
name: herder
description: Use when coordinating other agents or asked to dispatch, track, read reports from, or verify agent runs across executors.
---

## When to use

Use herder to dispatch Claude, Vibe or Codex workers and keep their run identities,
evidence and verification results together. Any coordinating
agent or person can use the same CLI.

## The loop

1. Run `herder doctor` for the intended executor and profile.
2. Prefer briefs outside `--repo`; files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. Direct-mode briefs require
   `<task>`, `<workstream_contract>`, `<implementation_authority>`, `<action_safety>`,
   `<default_follow_through_policy>`, `<verification_loop>`, `<economy_contract>` and
   `<structured_output_contract>`. Review-mode briefs require `<task>`, `<action_safety>`,
   `<default_follow_through_policy>` and `<structured_output_contract>`. Include scope and checks.
   Name every skill the worker must load (for example `$write-rust`); workers do not reliably load them unasked.
3. Use `herder dispatch`; retain the printed run ID, name and optional goal label.
   Status, report, stop, resume, verify and wait also accept a unique dispatch
   `--name`; existing IDs take precedence; ambiguous or unknown names list matches or similar names.
   Dispatch defaults to `--mode direct`; use `--mode review` for read-only briefs.
   Dispatch and handoff accept `--effort <LEVEL>` for Codex and Claude. Use `low`, `medium`, `high`,
   `xhigh`, `max` or `ultra`, subject to executor and model support. The executor's configured
   default applies when both herder's `--effort` and native effort arguments are omitted. With
   the herder flag omitted, native effort arguments after `--` are forwarded and `requested_effort`
   remains empty. Vibe takes effort from the selected model alias's `thinking` setting.
   Use medium effort for bounded direct-mode Codex work orders.

   ```sh
   herder dispatch --executor codex --effort medium --brief /tmp/work-order.md --repo .
   ```

   Vibe/Codex resume replays the recorded model and extra arguments after `--`.
   Codex also replays the requested effort.
Claude resume stops a settled live session, then wakes the same identity with its
saved options under the recorded profile; copies are stopped and rejected.
4. Follow progress with `herder ls` (models, requested effort and executor state tallies) and `herder status`.
   Use `herder status <run> --tail <N>` (1–1000) for the current launch attempt's last N stream lines capped at 500 characters each (`data.tail` in JSON), with a message for missing streams or Claude runs.
   Vibe/Codex status evidence carries `stream_age_s` (seconds since the stream last
   changed) and, for a live Codex run, `running_command:`.
   Use `herder wait <run>...` for any settled run, or `--all` for all runs;
   `--timeout <secs>` defaults to 3600. It polls every 5 seconds. When the wait
   condition is met, it exits 0 if every settled run is finished, or 1 if any
   failed, stopped or blocked. A timeout also exits 1. Unknown observations can
   clear during polling. Without `--all`, another settled run can satisfy the wait. Final output
   lists every run and report path. Example: `herder wait run-id --timeout 60`.
   When Rex is available, `--pane` opens a pane; Vibe/Codex panes use `herder wait`
   and close on exit. Read `herder report`.
5. Inspect the actual changes and run `herder verify` with scope paths and checks.
   After rebasing, use `herder verify <run> --baseline <new-upstream-rev>` to replace
   the recorded scope baseline. Invalid commit revisions leave the run unchanged.
   Resolve each review finding or record its approved deferral.
6. The coordinator handles approval and publication outside herder.

For paired read-only reviews, write one brief containing `task`, `action_safety`,
`default_follow_through_policy` and `structured_output_contract`. Dispatch it to each
executor with the same goal; select a Codex model, and repeat `--model` to review with several
Vibe models (use the review models your host instructions name):

```sh
herder dispatch --mode review --executor codex --model <codex-model> --brief /tmp/review.md --repo . --goal paired-review --name codex-review
herder dispatch --mode review --executor vibe --model <vibe-model-1> --model <vibe-model-2> --brief /tmp/review.md --repo . --goal paired-review --name vibe-review
herder wait <id1> <id2> <id3> --all
```

A repeated `--model` launches one recorded run per model, named `<name>-<model>`, and
prints every run ID. It is refused outside `--mode review` and for handoffs, because
one worktree has one writer, and when a model repeats, because both runs would share
one name. If a later launch fails, the runs already started stay listed beside the
error. Wait on all printed run IDs, then read every report.
Herder launches Codex review runs read-only and replays the saved sandbox flag when
resuming reviews recorded by this version. Older review records without a saved sandbox
flag resume with the launcher's default. Start a fresh review when you need read-only
enforcement for an older run. Claude and Vibe review runs rely on the brief's read-only
instruction alone. Vibe runs with `--auto-approve`.

## Handing off to another agent

When the coordinating conversation moves to another agent, write the continuation
context (decisions, constraints, remaining work, explicit authorizations) into a
plain UTF-8 file, stop editing the task and repository, then run
`herder handoff --executor claude --brief /tmp/HANDOFF.md --repo . [dispatch options]`.
Handoff reuses dispatch's launch, store and optional `--pane` (Claude attach in Rex), but
accepts a plain continuation document: its full bytes are snapshotted into the
run's `input.md` behind the standard continuation contract and linted like any
brief, so later edits to the file cannot change what the worker receives. It
launches a continuation only: it does not transfer hidden conversation, stop
another writer, or prove the context was understood, and
the launch result is launch evidence only. Invalid documents fail before launch
with no run record. `--mode review` instructs the receiver to stay read-only and
applies the same Codex sandbox as review dispatch. Delegate or publish only when
the document or the user expressly authorizes it.

## Rules

- Choosing the executor and model is the coordinator's job; herder never selects one. Vibe runs are auto-approve only.
- A worker report is never proof. Verify the actual output independently.
- Publish only after approval; herder does not publish or grant approval.
- Treat `unknown` as unresolved evidence, not completion or permission to retry.
  Its JSON `cause` and `herder status` cause line distinguish `query_failed`, `identity_mismatch`,
  `record_disagrees`, `unrecognized_status`, and `unconfirmed`; other states omit the cause.
- Preserve recorded identities and profiles. Consult command help for stop,
  resume, dispatch's `--pane` flag and executor limitations.
- When something surprises you, file it with `gh issue create -R Shaance/herder` only if the issue text contains no private setup details
  (local paths, hostnames, account or model routing). Otherwise, record the surprise in a local note outside any repository
  and mention the note in your own report or handoff. Continue with work that remains safe and understood. Do not paper over an executor disagreement.

All flag details and examples live in `herder <command> --help`.

## Codex runs from a background coordinator

Run the following as one background shell command, for example in a Claude Code background Bash call.

```sh
herder wait "$run" --timeout 3600; herder report "$run"
```

The host wakes the coordinator when the command exits, and the output ends with the final report if available.
Use `;` because `wait` exits 1 if the run failed, stopped or blocked, or the wait timed out. Using `&&` would skip the report.
Never write `sleep` or `kill -0` waiter loops. After context compaction, run `herder ls` and start a waiter only for runs that are `starting` or `running` and have no live waiter. An older waiter ends at its own timeout.

`herder wait` lists each report path. `herder report <run>` prints the path and then the contents of the current attempt's `final.md` (`final2.md` after one resume, and so on)
from `$HERDER_STATE_DIR/runs/<id>/`. The state directory defaults to `~/.local/state/herder`. For Codex, the launcher writes the run's last agent message there with `codex exec -o`.

Put Codex options after `--`. Herder replays them on resume.

- Attach an image with `-- --image=/abs/path/shot.png`. Use the `=` form. The short `-i <file>` accepts several values and consumes `resume <thread> -`, so the resume silently starts a new session.
  Keep attachments outside `--repo`, like briefs: new files inside it outside the authorized paths fail verify scope.
- Select a named Codex config profile with `-- -p <name>`. Codex layers settings from `$CODEX_HOME/<name>.config.toml`, such as the model and `model_reasoning_effort`, over its base configuration. Keep model IDs in that host file and out of briefs and skills. Codex silently ignores a missing profile and runs on its defaults, so check the file exists.
- The Codex `workspace-write` sandbox blocks network access, so checks that bind local ports, such as test servers or emulators, can fail.
  Prefer running those checks yourself with `herder verify <run> --check '<cmd>'`, which runs outside the worker sandbox.
  If the worker must run them, `-- -c sandbox_workspace_write.network_access=true` opens all network access for that run, including connections beyond loopback. Use it only in direct mode and say so in the brief.
