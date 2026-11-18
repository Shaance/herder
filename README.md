# herder

A Rust CLI for coordinating Claude, Vibe and Codex workers from any agent or
person. Installing from source requires Rust.

macOS is supported and tested in CI; Linux is untested.

Prerequisites: bash, jq, perl, git, and the executor CLIs you use: codex, vibe,
and/or claude.

```sh
cargo install --git https://github.com/Shaance/herder
herder doctor --json
```

**Optional: Rex.** [Rex](https://superlogical.com) is a macOS terminal in
invite-only public beta. In Rex, `--pane` opens a pane to attach to a Claude
worker or wait for a Vibe or Codex worker.

Prefer briefs outside `--repo`; files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. A direct-mode brief such as `/tmp/task.md` requires the blocks `<task>`,
`<workstream_contract>`, `<implementation_authority>`, `<action_safety>`,
`<default_follow_through_policy>`, `<verification_loop>`, `<economy_contract>`, and
`<structured_output_contract>`. Name authorized paths and rejected alternatives.

```sh
herder dispatch --executor codex --brief /tmp/task.md --repo . --goal feature-1 --path src/
# Use the printed run ID below.
herder ls --goal feature-1
herder status 20261101-120000-abcd
herder wait 20261101-120000-abcd --timeout 3600
herder report 20261101-120000-abcd
herder verify 20261101-120000-abcd --check 'cargo test --offline'
# Review evidence before publishing as coordinator.
```

All commands accept `--json`: `{ "ok": boolean, "data": object|null,
"error": string|null }`. Operational findings (failed launches, verification,
missing reports, doctor gaps) remain in `data`; invocation errors use `error`.
Exit codes: 0 success, 1 operational failure, 2 usage error. A successful status
query can report `unknown`; inspect its evidence before taking action.
Unknown observations include a `cause` in JSON and a `cause: <value>` line in `herder status`:
`query_failed`, `identity_mismatch`, `record_disagrees`, `unrecognized_status`, or `unconfirmed`; other states omit it.
Use `herder <command> --help` for examples and flags; no command prompts.

State defaults to `~/.local/state/herder`, overridden by `HERDER_STATE_DIR`.
Existing state is not moved automatically.
`runs/<id>/run.json` owns recorded run facts; scripts place evidence beside it.
`input.md` is herder's brief copy; `final.md`, `final2.md`, etc. are attempt reports.
Run writes use temporary files and rename under a per-run `run.lock` advisory lock,
including status recovery. Read commands never wait for that lock: while busy,
they show recorded state and `run busy` evidence without saving. Mutating lock
waits time out after two seconds. Launch gates retain the lock; verification
runs checks outside it, then locks, reloads and appends the original attempt's
verdict. Unrelated runs remain independent. Issue-log writes are not serialized.
Dispatch prepares `.ID.pending/` and publishes the directory only with `run.json`.
Interrupted preparation can leave a hidden staging directory; `ls` ignores it.
Broken published directories appear as individual errors in `ls`, alongside healthy runs.
Scripts are embedded in the binary. To override them, set `HERDER_SKILLS_DIR` to an
ai-skills-shaped directory containing `<owner>/scripts/<name>.sh`.
Claude profile precedence is `--profile`, `CLAUDE_CONFIG_DIR`, then explicitly
none (Claude's default). Dispatch detects trust failures without editing trust.

Status, report, stop, resume, verify and wait accept a run ID or a unique dispatch
`--name`. Existing IDs take precedence; ambiguous names list their matches, unknown names list similar ones.
Human dispatch output includes both ID and name.
`herder status <run> --tail <N>` (1–1000) shows the current launch attempt's last N event-stream lines capped at 500 characters each, adds `data.tail` in JSON, and explains missing streams or Claude's lack of a herder-recorded stream without changing status or exit codes.

`herder wait <run>...` polls every 5 seconds, returning when any run settles;
`--all` waits for every listed run. Finished, failed, stopped and blocked count as
settled (exit 0); unknown evidence or timeout exits 1. The default timeout is 3600
seconds. Human output shows state changes; final human and JSON output includes
every run's id, name, state and current report path.
`herder ls` includes models and a per-executor state tally (`data.summary` in JSON),
scoped by `--goal` when supplied. Broken entries remain errors outside the tally.
`--pane` uses Rex; Vibe/Codex panes run `herder wait` and close when it exits.
Claude panes continue to use `claude attach`. Vibe/Codex launch and resume scripts
receive no inherited `REX_SESSION`, `REX_BLOCK` or `REX_SERVER`.
Extra dispatch arguments after `--` are recorded and replayed with `--model` on
Vibe/Codex resume. They go to the executor, including Vibe arguments
past its launcher's separator. Herder Vibe runs are auto-approve only.
Launcher gate exit 3 preserves `starting` or `pending_resume`; herder returns 0.
A resume requires a non-empty delta; the delta can use `<task>`, `<verification_loop>`
and `<constraints>` without dispatch's full brief contract. The accepted attempt
and report remain current until resume succeeds (Vibe/Codex require a new PID with the same executor identity).
After correcting a recorded pending-resume error, `herder resume` retries the same
attempt using its previous identity when the executor confirms no live worker.
Running or unknown executor states block retry. Executor rejection diagnostics
still apply, including Vibe's refusal to reuse existing attempt evidence files.
`herder resume --request-id KEY` (KEY: 1-200 bytes, no control characters) makes a
resume idempotent: Herder records the request id and its attempt in `run.json`
(`request_attempts`) in the same saved mutation that marks the resume pending,
before launching. Replaying a known KEY with byte-identical `--brief` content
returns the recorded run and observation without another launch, whatever its
state; the same KEY with different bytes is rejected. A new KEY never takes over
a pending attempt reserved by another KEY, and a resume without an id cannot
rewrite an input file reserved by a KEY with different content. A stored request
means accepted by Herder, not agent receipt or success; a failed or unknown launch
is never automatically relaunched under a replayed key.
A late script identity can be recorded by status.
Initial Claude dispatch failures before acceptance remain terminal. Once Claude prints an accepted
short ID, a later launch gate failure returns exit 1 but retains that ID and its
diagnosis in `run.json`. Use status to observe that worker; do not relaunch it.
Status validates cwd and a nonempty full session identity before recording it,
then requires that same identity on every query and before stop. Gate errors
remain historical facts even when the worker later runs or finishes. Existing
failed records are unchanged and are not automatically attached to workers.
Claude resume stops a settled session if still live, then wakes the same session
with its saved options and recorded profile. No launch flags are replayed. Copies
are stopped and rejected; failed pending resumes can retry the same attempt.
Codex `status_classic.sh` inspects the newest resumed attempt itself, so herder
passes no attempt suffix and checks the reported pid against the resumed worker.
Executor queries time out after two seconds with `unknown` evidence; existing reports
remain readable. Stop waits briefly for exit and otherwise reports `stop sent` with exit 0.
Claude completion requires a terminal executor state. Codex/Vibe outcomes follow their
status script verdicts, so correctness also depends on those scripts.
Their evidence includes `stream_age_s`, the seconds since the event stream last changed,
and for a live Codex run `running_command:` naming a started command that has not completed.
A report is never proof. Verification is separate from lifecycle and approval.
Dispatch records the Git baseline commit. After rebasing a worker branch, use
`herder verify <run> --baseline <new-upstream-rev>` to verify against and persist
the new commit baseline. Invalid revisions fail without changing the run record. Verification checks committed, staged,
unstaged and untracked paths against that baseline, then runs the evidence script
and requested commands. Records without a baseline load, but scope verification fails
explicitly. A verdict belongs to its attempt; a resumed attempt starts unverified.
Legacy `starting` intents load as running, and untagged legacy verdicts belong to attempt 1.

For paired read-only reviews, write one brief containing `<task>`, `<action_safety>`,
`<default_follow_through_policy>` and `<structured_output_contract>`. Dispatch it to each
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
`--mode` selects brief linting only; the brief must enforce read-only behavior.

## Handing a conversation to another agent

When this coordinating conversation moves to another agent, write the continuation
context yourself (decisions, constraints, remaining work, explicit authorizations)
into a plain UTF-8 document, stop editing the task and repository, then:

```sh
herder handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/
```

`herder handoff` reuses dispatch's options, launch, store and `--pane`, but accepts a
plain continuation document instead of a structured implementer brief: it snapshots
the document's full bytes into the run's `input.md` behind a standard continuation
contract and sends that durable input to the worker. The generated prompt still
passes the same brief linter as dispatch. Missing, empty, whitespace-only or
non-UTF-8 documents fail before any launch and leave no run record; later edits or
deletion of the original file cannot change what the worker receives, and the
document is never printed to stdout.

This launches a continuation only: it does not transfer hidden conversation, stop
another writer, or prove the context was understood, and
the launch result is launch evidence only. Use `--pane` for the existing Claude
attach functionality when in Rex. `--mode review` instructs the receiver to stay read-only; it does not add an OS sandbox. No delegation or
publication happens unless the document or you expressly authorized it.

## Using herder from any agent

herder is a plain CLI, so any agent that can run shell commands can coordinate with it. Each host needs
herder to run with normal user access, because dispatch starts other agent CLIs that use the network and
their own config directories.

- Claude Code: allow `Bash(herder:*)` in settings, or approve herder commands when asked.
- Codex: add `prefix_rule(pattern=["herder"], decision="allow")` to `~/.codex/rules/default.rules`.
  Codex then runs herder outside its sandbox without prompting. Without the rule, read commands work in
  the `workspace-write` sandbox and writes fail with "Operation not permitted".
- Vibe: approve herder commands when asked, or run the session with the `auto-approve` agent.

The skill (`skill/SKILL.md`) is the same for every host; install it as a skill in each one.

See [architecture](docs/architecture.md) for the run lifecycle and implementation.

Development checks: `cargo build --offline --locked`, `cargo test --offline --locked`,
and `cargo clippy --offline --locked --all-targets -- -D warnings`.
Tests invoke the binary with temporary state and stub executors, never real workers.
Contract tests run the vendored launch/status scripts and require `jq`. Set
`HERDER_SKILLS_DIR` to test another ai-skills-shaped directory instead.
