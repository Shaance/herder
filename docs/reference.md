# herder reference

This page describes the CLI's behavior and operational limits. For the run record and implementation design, see [architecture](architecture.md).

## JSON output and exit codes

Every command accepts `--json` and returns an envelope: `{ "ok": boolean, "data": object|null, "error": string|null }`. Operational findings, including failed launches, verification failures, missing reports and doctor gaps, remain in `data`; invocation errors use `error`. Exit codes are 0 for success, 1 for operational failure and 2 for usage error. No herder command prompts. Use `herder <command> --help` for flags and examples.

## Run state and unknown causes

A successful status query can report `unknown`; inspect its evidence before acting. Unknown observations include a `cause` in JSON and a `cause: <value>` line in human `herder status` output. The causes are `query_failed`, `identity_mismatch`, `record_disagrees`, `unrecognized_status` and `unconfirmed`; other states omit the cause.

Executor queries time out after two seconds with `unknown` evidence. Existing reports remain readable. Claude completion requires a terminal executor state. Codex and Vibe outcomes follow their status script verdicts, so correctness also depends on those scripts. Their evidence includes `stream_age_s`, the seconds since the event stream last changed. For a live Codex run, `running_command:` names a started command that has not completed.

## Run record, storage and locking

State defaults to `~/.local/state/herder`; `HERDER_STATE_DIR` overrides it. Existing state is not moved automatically. `runs/<id>/run.json` owns recorded run facts, and scripts place evidence beside it. `input.md` is herder's brief copy; `final.md`, `final2.md` and so on are attempt reports.

Run writes use temporary files and rename under a per-run `run.lock` advisory lock, including status recovery. Read commands never wait for that lock: while it is busy, they show recorded state and `run busy` evidence without saving. Mutating lock waits time out after two seconds. Launch gates retain the lock. Verification runs checks outside it, then locks, reloads and appends the original attempt's verdict. Unrelated runs remain independent.

Dispatch prepares `.ID.pending/` and publishes the directory only with `run.json`. Interrupted preparation can leave a hidden staging directory, which `ls` ignores. Broken published directories appear as individual errors in `ls` alongside healthy runs.

Scripts are embedded in the binary. Set `HERDER_SKILLS_DIR` to an ai-skills-shaped directory containing `<owner>/scripts/<name>.sh` to override them.

## Names and run IDs

`status`, `report`, `stop`, `resume`, `verify` and `wait` accept a run ID or a unique dispatch `--name`. Existing IDs take precedence. Ambiguous names list their matches. Unknown names list similar names if any exist; otherwise herder suggests `herder ls`. Human dispatch output includes both ID and name.

## Status and tail

`herder status <run> --tail <N>` shows the current launch attempt's last N event-stream lines, where N is 1 to 1000. Lines are capped at 500 characters each. JSON adds `data.tail`. Missing streams and Claude's lack of a herder-recorded stream are explained without changing status or exit codes.

## Wait

`herder wait <run>...` polls every five seconds and returns when any run settles; `--all` waits for every listed run. Finished, failed, stopped and blocked count as settled. The command exits 0 when its wait condition is met. An unknown observation can clear during polling and does not itself end the wait. Without `--all`, a settled run can satisfy the wait while another remains unknown. If the wait condition is not met at timeout, the command exits 1. The default timeout is 3600 seconds. Human output shows state changes; final human and JSON output includes every run's ID, name, state and current report path.

## List runs

`herder ls` includes models and a per-executor state tally, available as `data.summary` in JSON. `--goal` scopes the list and tally to the supplied goal. Broken entries remain errors outside the tally.

## Dispatch and executor arguments

Prefer briefs outside `--repo`. Files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. A direct-mode brief requires `<task>`, `<workstream_contract>`, `<implementation_authority>`, `<action_safety>`, `<default_follow_through_policy>`, `<verification_loop>`, `<economy_contract>` and `<structured_output_contract>` blocks. Name authorized paths and rejected alternatives in the brief.

Extra dispatch arguments after `--` go to the executor, including Vibe arguments past its launcher's separator. They are recorded and replayed with `--model` on Vibe and Codex resume. Herder Vibe runs are auto-approve only. Launcher gate exit 3 preserves `starting` or `pending_resume`; herder returns 0.

## Resume, retry and request IDs

Resume requires a non-empty delta brief. The delta can use `<task>`, `<verification_loop>` and `<constraints>` without dispatch's full brief contract. The accepted attempt and report remain current until resume succeeds. Vibe and Codex require a new PID with the same executor identity.

After correcting a recorded pending-resume error, `herder resume` retries the same attempt using its previous identity when the executor confirms no live worker. Running or unknown executor states block retry. Executor rejection diagnostics still apply, including Vibe's refusal to reuse existing attempt evidence files. A late script identity can be recorded by status.

`herder resume --request-id KEY` makes a resume idempotent. KEY is 1 to 200 bytes with no control characters. Before launching, herder records the request ID and attempt in `run.json` under `request_attempts`, in the same saved mutation that marks the resume pending. Replaying a known KEY with byte-identical `--brief` content returns the recorded run and observation without another launch, whatever its state. The same KEY with different bytes is rejected.

A new KEY never takes over a pending attempt reserved by another KEY. A resume without an ID cannot rewrite an input file reserved by a KEY with different content. A stored request means herder accepted it, not that the agent received it or succeeded. A failed or unknown launch is never automatically relaunched under a replayed key.

Codex `status_classic.sh` inspects the newest resumed attempt itself. Herder passes no attempt suffix and checks the reported PID against the resumed worker.

## Claude identity, profiles and resume

Claude profile precedence is `--profile`, `CLAUDE_CONFIG_DIR`, then explicitly none, which uses Claude's default. Dispatch detects trust failures without editing trust.

Initial Claude dispatch failures before acceptance remain terminal. Once Claude prints an accepted short ID, a later launch gate failure returns exit 1 but retains that ID and its diagnosis in `run.json`. Use status to observe that worker; do not relaunch it. Status validates the working directory and a nonempty full session identity before recording it, then requires the same identity on every query and before stop. Gate errors remain historical facts even when the worker later runs or finishes. Existing failed records are unchanged and are not automatically attached to workers.

Claude resume stops a settled session if it is still live, then wakes the same session with its saved options and recorded profile. No launch flags are replayed. Herder rejects unexpected copies and attempts to stop copies whose IDs it can parse. It reports unparseable IDs and stop failures; a copy can require manual cleanup. Failed pending resumes can retry the same attempt.

## Stop

Stop requires a running or blocked worker with a matching recorded identity. Claude uses `claude stop`; Vibe and Codex receive TERM only after the PID's command matches the run. Stop waits briefly for exit. If exit is not confirmed, it reports `stop sent` with exit 0; use status to confirm.

## Verify, scope and baseline

A report is never proof. Verification is separate from lifecycle and approval. Dispatch records the Git baseline commit. Verification checks committed, staged, unstaged and untracked paths against that baseline, then runs the evidence script and requested commands. Records without a baseline load, but scope verification fails explicitly. A verdict belongs to its attempt; a resumed attempt starts unverified.

Pass one or more `--path` options to verify to use those paths for that check in place of the paths recorded at dispatch. If dispatch recorded no paths, verify requires at least one `--path`. Verification rejects starting, running and blocked workers; it can run when the observation is unknown, so inspect the evidence first.

After rebasing a worker branch, use `herder verify <run> --baseline <new-upstream-rev>` to verify against and persist the new commit baseline. Invalid revisions fail without changing the run record. Review the evidence before publishing as coordinator.

## Paired reviews

For paired read-only reviews, write one brief containing `<task>`, `<action_safety>`, `<default_follow_through_policy>` and `<structured_output_contract>`. Dispatch it to each executor with the same goal; select a Codex model, and repeat `--model` to review with several Vibe models. Use the review models your host instructions name.

```sh
herder dispatch --mode review --executor codex --model <codex-model> --brief /tmp/review.md --repo . --goal paired-review --name codex-review
herder dispatch --mode review --executor vibe --model <vibe-model-1> --model <vibe-model-2> --brief /tmp/review.md --repo . --goal paired-review --name vibe-review
herder wait <id1> <id2> <id3> --all
```

Repeated `--model` launches one recorded run per model and prints every run ID. With `--name`, each run is named `<name>-<model>`; otherwise it uses its generated ID. Repeating `--model` is allowed only with `--mode review`, is refused for handoffs, and requires distinct model values. This limits each direct-mode dispatch to one worker in the worktree. If a later launch fails, runs already started stay listed beside the error. Wait on all printed run IDs, then read every report. For dispatch, `--mode` selects brief linting only; the brief must enforce read-only behavior.

## Handoff

When a coordinating conversation moves to another agent, write the continuation context yourself, including decisions, constraints, remaining work and explicit authorizations, in a plain UTF-8 document. Stop editing the task and repository, then run:

```sh
herder handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/
```

Handoff reuses dispatch's options, launch, store and `--pane`, but accepts a plain continuation document instead of a structured implementer brief. It snapshots the document's full bytes into the run's `input.md` behind a standard continuation contract and sends that durable input to the worker. The generated prompt still passes the same brief linter as dispatch. Missing, empty, whitespace-only or non-UTF-8 documents fail before launch and leave no run record. Later edits or deletion of the original cannot change what the worker receives, and the document is never printed to stdout.

Handoff launches a continuation only: it does not transfer hidden conversation, stop another writer or prove the context was understood. The launch result is launch evidence only. Use `--pane` for Claude attach in Rex. `--mode review` instructs the receiver to stay read-only; it does not add an OS sandbox. The generated prompt tells the receiver not to delegate or publish unless the continuation document or the user expressly authorized it; herder does not enforce this as an OS restriction.

## Rex panes and environment

[Rex](https://superlogical.com) is a macOS terminal in invite-only public beta. `--pane` opens a pane for a Claude attach or for waiting on a Vibe or Codex worker. Vibe and Codex panes run `herder wait` and close when it exits; Claude panes use `claude attach`. Vibe and Codex launch and resume scripts receive no inherited `REX_SESSION`, `REX_BLOCK` or `REX_SERVER`.

## Legacy records

Legacy `starting` intents load as running. Untagged legacy verdicts belong to attempt 1.

## Agent permissions

herder is a plain CLI, so any agent that can run shell commands can coordinate with it. Each host needs herder to run with normal user access because dispatch starts other agent CLIs that use the network and their own config directories.

Claude Code can allow `Bash(herder:*)` in settings or ask for command approval. Codex can add `prefix_rule(pattern=["herder"], decision="allow")` to `~/.codex/rules/default.rules`; Codex then runs herder outside its sandbox without prompting. Without the rule, read commands work in the `workspace-write` sandbox and writes fail with "Operation not permitted". Vibe can ask for approval or run the session with the `auto-approve` agent. The [skill](../skill/SKILL.md) is the same for every host; install it as a skill in each one.
