# herder reference

This page describes the CLI's behavior and operational limits. For the run record and implementation design, see [architecture](architecture.md).

## JSON output and exit codes

Every command accepts `--json` and returns an envelope: `{ "ok": boolean, "data": object|null, "error": string|null }`. Operational findings, including failed launches, verification failures, missing reports and doctor gaps, remain in `data`; invocation errors use `error`. Exit codes are 0 for success, 1 for operational failure and 2 for usage error. No herder command prompts. Use `herder <command> --help` for flags and examples.

## Run state and unknown causes

A successful status query can report `unknown`; inspect its evidence before acting. Unknown observations include a `cause` in JSON and a `cause: <value>` line in human `herder status` output. The causes are `query_failed`, `identity_mismatch`, `record_disagrees`, `unrecognized_status` and `unconfirmed`; other states omit the cause.

Executor queries time out after ten seconds with `unknown` evidence. Existing reports remain readable. Claude completion requires a terminal executor state. Codex and Vibe outcomes follow their status script verdicts, so correctness also depends on those scripts. Their evidence includes `stream_age_s`, the seconds since the event stream last changed. For a live Codex run, `running_command:` names a started command that has not completed.

## Run record, storage and locking

State defaults to `~/.local/state/herder`; `HERDER_STATE_DIR` overrides it. Existing state is not moved automatically. `runs/<id>/run.json` owns recorded run facts, and scripts place evidence beside it. `input.md` is herder's brief copy; `final.md`, `final2.md` and so on are attempt reports.

Run writes use temporary files and rename under a per-run `run.lock` advisory lock, including status recovery. Read commands never wait for that lock: while it is busy, they show recorded state and `run busy` evidence without saving. Mutating lock waits time out after two seconds. Launch gates retain the lock. Verification runs checks outside it, then locks, reloads and appends the original attempt's verdict. Unrelated runs remain independent.

Dispatch prepares `.ID.pending/` and publishes the directory only with `run.json`. Interrupted preparation can leave a hidden staging directory, which `ls` ignores. Broken published directories appear as individual errors in `ls` alongside healthy runs.

On a single `dispatch`, supply `--request-id KEY` to make retries reuse the first run ID. Herder stores an immutable receipt in `requests/` before publishing the run or starting the worker. All clients must share the same state directory for this guarantee. If the process stops between reservation and publication, the receipt still identifies the reserved run. Inspect the state directory before deciding whether to issue a fresh work order with a new key.

Scripts are embedded in the binary. Set `HERDER_SKILLS_DIR` to an ai-skills-shaped directory containing `<owner>/scripts/<name>.sh` to override them.

## Names and run IDs

`status`, `report`, `stop`, `resume`, `verify` and `wait` accept a run ID or a unique dispatch `--name`. Existing IDs take precedence. Ambiguous names list their matches. Unknown names list similar names if any exist; otherwise herder suggests `herder ls`. Human dispatch output includes both ID and name.

## Status and tail

For a client bound to one checkout, `herder context --repo PATH --json` returns the canonical configured directory and the Git worktree root, branch, HEAD, and Herder state directory. A linked worktree has its own root even if another worktree points at the same commit. `checkout_state` distinguishes an attached branch, detached HEAD, unborn branch and non-Git directory.

Pass `--repo ROOT` to `status`, `report`, or `wait` to observe an exact run ID in that checkout. An absent or foreign ID returns the same `data.outcome` of `not_found`. Scoped `ls` filters by checkout before observing workers. If a record cannot be attributed, the listing indicates that it may be incomplete.

`herder status <run> --tail <N>` shows the current launch attempt's last N event-stream lines, where N is 1 to 1000. Lines are capped at 500 characters each. JSON adds `data.tail`. Missing streams and Claude's lack of a herder-recorded stream are explained without changing status or exit codes.

## Wait

`herder wait <run>...` polls every five seconds and returns when any run settles; `--all` waits for every listed run. Finished, failed, stopped and blocked count as settled. When the wait condition is met, the command exits 0 if every settled run at return is finished and 1 if any is failed, stopped or blocked. An unknown observation can clear during polling and does not itself end the wait. Without `--all`, a settled run can satisfy the wait while another remains unknown. If the wait condition is not met at timeout, the command exits 1. The default timeout is 3600 seconds. Human output shows state changes; final human and JSON output includes every run's ID, name, state and current report path.

`herder wait --run-dir PATH` observes one exact run. Supply its canonical absolute `STATE_DIR/runs/RUN_ID` path without positional targets or `--repo`. The run directory supplies the store and ID. Codex and Vibe observations in this mode use the embedded status adapters, regardless of `HERDER_SKILLS_DIR`.

While a Claude worker is running, human `wait` output shows the report path on stderr when a poll finds a readable report for the observed attempt. The wait continues tracking the worker. For a pending resume, this notice can point to the new attempt's report while `herder report` still reads the last accepted attempt.

On timeout, human stderr reports the last available progress for each unsettled attempt. JSON adds `data.runs[].progress` for those runs, with `attempt`, `kind` and, when available, `updated_at`, `age_seconds` or an unknown-state `cause`. A script executor can report `session_opened` when the session or thread opening is the only recorded activity. Timestamps and ages describe the event stream or Claude report file's last modification. Claude can also report that the executor says it is working without a timestamp. Wait omits command contents and event text from these diagnostics. An old or silent stream leaves the executor's current state intact.

### Terminal resume

A foreground, interactive wait for one run can report a restart command to a terminal that supports the [Program Resume Protocol (OSC 7577)](https://www.superlogical.com/rex/docs/build/program-resume). Herder resolves the ID, unique name, or `--run-dir` first. The restart command uses the selected run's canonical directory and starts a fresh wait with the default timeout. The terminal decides whether to replay it.

For Claude, Herder reports after a fresh status check matches a running worker's recorded short ID, full session ID, and checkout under its saved profile. It withdraws the record if that identity becomes invalid or the worker enters a pending resume, and can publish it again after a fresh match to the same worker. The restored terminal needs `claude` on `PATH`. For Codex and Vibe, reporting uses the embedded status scripts and the restored terminal needs `jq` on `PATH`. An exact-directory wait for either scripted executor reports a missing `jq` before polling.

Reporting applies to human output with the default timeout. Scoped waits, `--all`, multiple runs, and positional Codex or Vibe waits using `HERDER_SKILLS_DIR` continue as ordinary waits. Herder clears the record on completion or Ctrl-C and refreshes it when continued in the foreground. A hangup or termination leaves it available to the terminal. Herder keeps the protocol token out of child commands.

## List runs

`herder ls` shows the effort requested through herder's `--effort` in `EFFORT` immediately after `MODEL`. A dash indicates that the herder flag was omitted. JSON includes `run.requested_effort` and a per-executor state tally in `data.summary`. `--goal` scopes the list and tally to the supplied goal. Broken entries remain errors outside the tally.

## Dispatch and executor arguments

Prefer briefs outside `--repo`. Files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. A direct-mode brief requires `<task>`, `<workstream_contract>`, `<implementation_authority>`, `<action_safety>`, `<default_follow_through_policy>`, `<verification_loop>`, `<economy_contract>` and `<structured_output_contract>` blocks. Name authorized paths and rejected alternatives in the brief.

Extra dispatch arguments after `--` go to the executor, including Vibe arguments past its launcher's separator. They are recorded and replayed with `--model` on Vibe and Codex resume. Herder Vibe runs are auto-approve only. Launcher gate exit 3 leaves the launch pending for observation. Herder returns 0 unless immediate Codex resume observation detects an identity conflict, which exits 1.

Dispatch and handoff accept `--effort <LEVEL>` for Codex and Claude. Accepted levels are `low`, `medium`, `high`, `xhigh`, `max` and `ultra`. The executor determines which levels a model supports. Its configured default applies when both herder's `--effort` and native effort arguments are omitted. With the herder flag omitted, native effort arguments after `--` are forwarded and `requested_effort` remains empty. Vibe uses the selected model alias's `thinking` setting in `config.toml`. Herder rejects a request that also sets native effort after `--`. Codex receives the requested effort on each launch and resume. Claude receives it at launch and resumes with the saved session options.

## Dispatch with verification

```sh
herder dispatch --executor codex --repo . --brief /tmp/task.md --path src/ --check 'cargo test --locked'
```

Repeat `--check` in execution order to arm coordinator-supplied shell commands for the dispatched attempt. After the launch returns, the same foreground invocation waits for the worker to finish, checks the recorded dispatch scope and runs the evidence collector and supplied commands from `--repo` with stdin closed. The coordinator can run this invocation as a background job. Use `--name` to look up the already recorded worker while dispatch is still running. With no `--path`, the scope check allows no new changes. Repeated `--model` cannot be combined with `--check`. A `--pane` remains a separate view of the worker.

`--check-timeout <SECONDS>` gives the supervisor and checks one deadline after the normal launch returns. It defaults to 3600 seconds. On timeout during a command, herder terminates the command's process group. Checks must run in the foreground and return an exit status. Keep the supervisor alive to enforce the deadline. The timeout leaves the worker under its executor's control. Launch failure, failed or blocked worker, timeout, scope violation, failed check or a superseded attempt all make checked dispatch exit 1. Exit 0 requires a finished worker and passing verification. For Codex, herder also requires the latest complete turn event in the dispatched attempt's stream tail to be `turn.completed`.

Checked dispatch retains the normal launch output and adds `data.verification` in JSON with the original `attempt`, `verdict`, and scope and command results. A skipped verification has a null verdict and a brief message. Human verification results go to stderr. The verdict uses the normal attempt-scoped verification record. Wait, status and ls only observe; resuming a worker starts an attempt without an armed check. Supply read-only, repeatable commands from the coordinator's own trusted instructions. A concurrent resume rejects a stale verification result. A check that has already started can still have side effects.

## Resume, retry and request IDs

Resume requires a non-empty delta brief. The delta can use `<task>`, `<verification_loop>` and `<constraints>` without dispatch's full brief contract. The previous report stays current while the resume is pending. Vibe and Codex require a new PID with the same executor identity.

For Codex, a new PID and a matching `thread.started` record in `events<N>.jsonl` confirm the resume. Before that event, a live candidate shows as `starting`. If the new worker dies before opening a thread, Herder records the attempt as `failed` so the next resume can use the next attempt number. The old PID or a reported completion without the event leaves the resume `unconfirmed`. A conflicting thread ID reports `identity_mismatch`.

After correcting a recorded pending-resume error, `herder resume` retries the same attempt using its previous identity when the executor confirms no live worker. Running or unknown executor states block retry. Executor rejection diagnostics still apply, including Vibe's refusal to reuse existing attempt evidence files. A conflicting Codex thread event blocks retry.

`herder resume --request-id KEY` makes a resume idempotent. KEY is 1 to 200 bytes with no control characters. Before launching, herder records the request ID and attempt in `run.json` under `request_attempts`, in the same saved mutation that marks the resume pending. Replaying a known KEY with byte-identical `--brief` content returns the recorded run and observation without another launch, whatever its state. The same KEY with different bytes is rejected.

A new KEY never takes over a pending attempt reserved by another KEY. A resume without an ID cannot rewrite an input file reserved by a KEY with different content. A stored request means herder accepted it, not that the agent received it or succeeded. A failed or unknown launch is never automatically relaunched under a replayed key.

Codex `status_classic.sh` inspects the newest resumed attempt itself. Herder passes no attempt suffix and checks the reported PID against the resumed worker. Codex resume scripts supplied through `HERDER_SKILLS_DIR` must write the numbered `events<N>.jsonl` stream with the worker's `thread.started` event.

## Claude identity, profiles and resume

Claude profile precedence is `--profile`, `CLAUDE_CONFIG_DIR`, then explicitly none, which uses Claude's default. Dispatch detects trust failures without editing trust.

For new Claude runs, the profile needs to trust the repository only. At the end of each reply, the Stop hook saves the latest nonempty reply as the report for that attempt. Resuming the session reuses this hook.

Herder starts Claude background sessions one at a time across processes that share a state directory.
Parallel Claude dispatches wait for each other's launch gate to finish.
Herder coordinates launches using `claude-launch.lock` in the state directory.
This works around a Claude Code 2.1.327 bug where simultaneous `claude --bg` launches can kill one of the sessions during startup.

Initial Claude dispatch failures before acceptance remain terminal. Once Claude prints an accepted short ID, a later launch gate failure returns exit 1 but retains that ID and its diagnosis in `run.json`. Use status to observe that worker; do not relaunch it. Status validates the working directory and a nonempty full session identity before recording it, then requires the same identity on every query and before stop. Gate errors remain historical facts even when the worker later runs or finishes. Existing failed records are unchanged and are not automatically attached to workers.

Claude resume stops a settled session if it is still live, then wakes the same session with its saved options and recorded profile. No launch flags are replayed. Herder rejects unexpected copies and attempts to stop copies whose IDs it can parse. It reports unparseable IDs and stop failures; a copy can require manual cleanup. Failed pending resumes can retry the same attempt.

## Stop

Stop requires a running or blocked worker with a matching recorded identity. Claude uses `claude stop`; Vibe and Codex receive TERM only after the PID's command matches the run. Stop waits briefly for exit. If exit is not confirmed, it reports `stop sent` with exit 0; use status to confirm.

If Herder refuses to stop a live Codex candidate because its thread ID conflicts, inspect the process, stop it manually, and dispatch a fresh run.

## Verify, scope and baseline

For a protected direct Codex work order, include `--request-id KEY --expected-head FULL_SHA --accept-with /absolute/check` and one or more `--path` values on `dispatch`. Herder requires a clean Git worktree and binds the original attempt to that baseline and executable. Codex has workspace write access. Herder uses the prefixes to audit Git-visible changes after execution and leaves edits in the worktree if verification fails. After the worker finishes, run `herder verify ID --repo ROOT --path ALLOWED_PREFIX --accept-with /absolute/check --json`. Supply the path prefixes that the host currently authorizes. Herder verifies eligibility before it runs the checker. It records the first completed verdict, including the attempt, scope audit and host check. Later calls return that receipt even if the files change. Run `herder status ID` to see the worker state and protected acceptance verdict, or add `--json` for the saved check details.

A report is never proof. Verification is separate from lifecycle and approval. Dispatch records the Git baseline commit. Verification checks committed, staged, unstaged and untracked paths against that baseline, then runs the evidence script and requested commands. Records without a baseline load, but scope verification fails explicitly. A verdict belongs to its attempt; a resumed attempt starts unverified.

Pass one or more `--path` options to verify to use those paths for that check in place of the paths recorded at dispatch. If dispatch recorded no paths, verify requires at least one `--path`. Verification rejects starting, running and blocked workers. It remains available when the observation is `unknown`, so inspect the evidence first.

Explicit `--check` commands take precedence. If omitted, verify reuses commands from the latest verification of this run that recorded any. If this run has no commands and has a goal, verify looks for the most recent verification with commands from another run in the same repository and goal. If no commands are found, verify runs only its built-in checks. Human output names the source run when commands are reused.

After rebasing a worker branch, use `herder verify <run> --baseline <new-upstream-rev>` to verify against and persist the new commit baseline. Invalid revisions fail without changing the run record. Review the evidence before publishing as coordinator.

## Paired reviews

For paired read-only reviews, write one brief containing `<task>`, `<action_safety>`, `<default_follow_through_policy>` and `<structured_output_contract>`. Dispatch it to each executor with the same goal; select a Codex model, and repeat `--model` to review with several Vibe models. Use the review models your host instructions name.

```sh
herder dispatch --mode review --executor codex --model <codex-model> --brief /tmp/review.md --repo . --goal paired-review --name codex-review
herder dispatch --mode review --executor vibe --model <vibe-model-1> --model <vibe-model-2> --brief /tmp/review.md --repo . --goal paired-review --name vibe-review
herder wait <id1> <id2> <id3> --all
```

Repeated `--model` launches one recorded run per model and prints every run ID. A supplied `--effort` applies to every run. With `--name`, each run is named `<name>-<model>`; otherwise it uses its generated ID. Repeating `--model` is allowed only with `--mode review`, is refused for handoffs, and requires distinct model values. This limits each direct-mode dispatch to one worker in the worktree. If a later launch fails, runs already started stay listed beside the error. Wait on all printed run IDs, then read every report.

Herder sandboxes Codex review dispatches and handoffs read-only. For reviews recorded by this version, herder saves the read-only sandbox flag in the executor arguments and replays it on each resume. Older review records without a saved sandbox flag resume with the launcher's default. Start a fresh review when you need read-only enforcement for an older run. Herder accepts a single explicit read-only sandbox and rejects sandbox config overrides and bypass flags before recording the run. Claude and Vibe review runs rely on the brief's read-only instruction alone. Vibe runs with `--auto-approve`.

## Handoff

When a coordinating conversation moves to another agent, write the continuation context yourself, including decisions, constraints, remaining work and explicit authorizations, in a plain UTF-8 document. Stop editing the task and repository, then run:

```sh
herder handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/
```

Handoff reuses dispatch's options, launch, store and `--pane`, but accepts a plain continuation document instead of a structured implementer brief. It snapshots the document's full bytes into the run's `input.md` behind a standard continuation contract and sends that durable input to the worker. The generated prompt still passes the same brief linter as dispatch. Missing, empty, whitespace-only or non-UTF-8 documents fail before launch and leave no run record. Later edits or deletion of the original cannot change what the worker receives, and the document is never printed to stdout.

Handoff launches a continuation only: it does not transfer hidden conversation, stop another writer or prove the context was understood. The launch result is launch evidence only. Use `--pane` for Claude attach in Rex. `--mode review` instructs the receiver to stay read-only and applies the same Codex sandbox as review dispatch. The generated prompt tells the receiver not to delegate or publish unless the continuation document or the user expressly authorized it; herder does not enforce this as an OS restriction.

## Rex panes and environment

[Rex](https://superlogical.com) is a macOS terminal in invite-only public beta. `--pane` opens a Claude attach pane or starts `herder wait` in a new pane for a Vibe or Codex worker. The initial wait pane closes when the wait exits. A restored wait can return to a shell and leave its output visible. Vibe and Codex launch and resume scripts receive no inherited `REX_SESSION`, `REX_BLOCK` or `REX_SERVER`.

To opt in to restoring eligible waits after a Rex server restart, add a trust rule to your [Rex configuration](https://www.superlogical.com/rex/docs/customize/resume-rules).

```lua
rex.set("resume.trust.herder_wait", {
  command = { "/absolute/path/to/herder", "wait", "--run-dir",
              rex.match.under("~/.local/state/herder/runs") },
})
```

Replace the executable with the exact path Herder reports. Run `rex block call -b MONITOR_BLOCK program_resume_records` to see the reported command for a running monitor, or use `realpath "$(command -v herder)"` for a direct installation. If this points into a versioned Homebrew Cellar, refresh the rule after upgrading Herder. With a custom `HERDER_STATE_DIR`, use the canonical absolute path to its `runs` subdirectory. Check the active rules with `rex config check`.

The rule authorizes only the observer command. Rex uses its usual shell when no rule matches.

## Legacy records

Legacy `starting` intents load as running. Untagged legacy verdicts belong to attempt 1.

## Agent permissions

herder is a plain CLI, so any agent that can run shell commands can coordinate with it. Each host needs herder to run with normal user access because dispatch starts other agent CLIs that use the network and their own config directories.

Claude Code can allow `Bash(herder:*)` in settings or ask for command approval. Codex can add `prefix_rule(pattern=["herder"], decision="allow")` to `~/.codex/rules/default.rules`; Codex then runs herder outside its sandbox without prompting. Without the rule, read commands work in the `workspace-write` sandbox and writes fail with "Operation not permitted". Vibe can ask for approval or run the session with the `auto-approve` agent. The [skill](../skill/SKILL.md) is the same for every host; install it as a skill in each one.
