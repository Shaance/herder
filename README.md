# crew

A Rust CLI for coordinating Claude, Vibe and Codex workers from any agent or
person in a Rex terminal. Requires Rust 1.88+, the executor CLIs, and the shared
ai-skills scripts. Vibe/Codex scripts also require Bash and jq.

```sh
cargo install --path . --locked
ln -s "$PWD/skill" ~/.ai-skills/skills/crew && bash ~/.ai-skills/setup.sh   # install the skill
crew doctor --json
```

Prefer briefs outside `--repo`; files already dirty at dispatch are excluded from verify scope only while their contents remain unchanged. Write `/tmp/task.md` with implementer-core's direct-mode contract blocks: `task`,
`workstream_contract`, `implementation_authority`, `action_safety`,
`default_follow_through_policy`, `verification_loop`, `economy_contract`, and
`structured_output_contract`. Name authorized paths and rejected alternatives.

```sh
crew dispatch --executor codex --brief /tmp/task.md --repo . --goal feature-1 --path src/
# Use the printed run ID below.
crew ls --goal feature-1
crew status 20261101-120000-abcd
crew wait 20261101-120000-abcd --timeout 3600
crew report 20261101-120000-abcd
crew verify 20261101-120000-abcd --check 'cargo test --offline'
# Review evidence before publishing as coordinator.
```

All commands accept `--json`: `{ "ok": boolean, "data": object|null,
"error": string|null }`. Operational findings (failed launches, verification,
missing reports, doctor gaps) remain in `data`; invocation errors use `error`.
Exit codes: 0 success, 1 operational failure, 2 usage error. A successful status
query can report `unknown`; inspect its evidence before taking action.
Use `crew <command> --help` for examples and flags; no command prompts.

State defaults to `~/.local/state/ai-skills/crew`, overridden by `CREW_STATE_DIR`.
`runs/<id>/run.json` owns recorded run facts; scripts place evidence beside it.
`input.md` is crew's brief copy; `final.md`, `final2.md`, etc. are attempt reports.
Run writes use temporary files and rename under a per-run `run.lock` advisory lock,
including status recovery. Read commands never wait for that lock: while busy,
they show recorded state and `run busy` evidence without saving. Mutating lock
waits time out after two seconds. Launch gates retain the lock; verification
runs checks outside it, then locks, reloads and appends the original attempt's
verdict. Unrelated runs remain independent. Issue-log writes are not serialized.
Dispatch prepares `.ID.pending/` and publishes the directory only with `run.json`.
Interrupted preparation can leave a hidden staging directory; `ls` ignores it.
Broken published directories appear as individual errors in `ls`, alongside healthy runs.
`CREW_SKILLS_DIR` defaults to `~/.ai-skills/skills`.
Claude profile precedence is `--profile`, `CLAUDE_CONFIG_DIR`, then explicitly
none (Claude's default). Dispatch detects trust failures without editing trust.

Status, report, stop, resume, verify and wait accept a run ID or a unique dispatch
`--name`. Existing IDs take precedence; ambiguous names list their matches, unknown names list similar ones.
Human dispatch output includes both ID and name.

`crew wait <run>...` polls every 5 seconds, returning when any run settles;
`--all` waits for every listed run. Finished, failed, stopped and blocked count as
settled (exit 0); unknown evidence or timeout exits 1. The default timeout is 3600
seconds. Human output shows state changes; final human and JSON output includes
every run's id, name, state and current report path.
`crew ls` includes models and a per-executor state tally (`data.summary` in JSON),
scoped by `--goal` when supplied. Broken entries remain errors outside the tally.
`--pane` uses Rex; Vibe/Codex panes run `crew wait` and close when it exits.
Claude panes continue to use `claude attach`. Vibe/Codex launch and resume scripts
receive no inherited `REX_SESSION`, `REX_BLOCK` or `REX_SERVER`.
Extra dispatch arguments after `--` are recorded and replayed with `--model` on
Vibe/Codex resume. They go to the executor, including Vibe arguments
past its launcher's separator. Crew Vibe runs are auto-approve only.
Launcher gate exit 3 preserves `starting` or `pending_resume`; crew returns 0.
A resume requires a non-empty delta; the delta can use `<task>`, `<verification_loop>`
and `<constraints>` without dispatch's full brief contract. The accepted attempt
and report remain current until a different PID with the same executor identity is recorded.
After correcting a recorded pending-resume error, `crew resume` retries the same
attempt using its previous identity when the executor confirms no live worker.
Running or unknown executor states block retry. Executor rejection diagnostics
still apply, including Vibe's refusal to reuse existing attempt evidence files.
`crew resume --request-id KEY` (KEY: 1-200 bytes, no control characters) makes a
resume idempotent: Crew records the request id and its attempt in `run.json`
(`request_attempts`) in the same saved mutation that marks the resume pending,
before launching. Replaying a known KEY with byte-identical `--brief` content
returns the recorded run and observation without another launch, whatever its
state; the same KEY with different bytes is rejected. A new KEY never takes over
a pending attempt reserved by another KEY, and a resume without an id cannot
rewrite an input file reserved by a KEY with different content. A stored request
means accepted by Crew, not agent receipt or success; a failed or unknown launch
is never automatically relaunched under a replayed key.
A late script identity can be recorded by status.
Claude failures before acceptance remain terminal. Once Claude prints an accepted
short ID, a later launch gate failure returns exit 1 but retains that ID and its
diagnosis in `run.json`. Use status to observe that worker; do not relaunch it.
Status validates cwd and a nonempty full session identity before recording it,
then requires that same identity on every query and before stop. Gate errors
remain historical facts even when the worker later runs or finishes. Existing
failed records are unchanged and are not automatically attached to workers.
Claude resume requires manual `claude attach <id>` with the recorded profile.
Codex `status_classic.sh` inspects the newest resumed attempt itself, so crew
passes no attempt suffix and checks the reported pid against the resumed worker.
Executor queries time out after two seconds with `unknown` evidence; existing reports
remain readable. Stop waits briefly for exit and otherwise reports `stop sent` with exit 0.
Claude completion requires a terminal executor state. Codex/Vibe outcomes follow their
status script verdicts, so correctness also depends on the installed scripts.
A report is never proof. Verification is separate from lifecycle and approval.
Dispatch records the Git baseline commit. After rebasing a worker branch, use
`crew verify <run> --baseline <new-upstream-rev>` to verify against and persist
the new commit baseline. Invalid revisions fail without changing the run record. Verification checks committed, staged,
unstaged and untracked paths against that baseline, then runs the evidence script
and requested commands. Records without a baseline load, but scope verification fails
explicitly. A verdict belongs to its attempt; a resumed attempt starts unverified.
Legacy `starting` intents load as running, and untagged legacy verdicts belong to attempt 1.

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

## Handing a conversation to another agent

When this coordinating conversation moves to another agent, write the continuation
context yourself (decisions, constraints, remaining work, explicit authorizations)
into a plain UTF-8 document, stop editing the task and repository, then:

```sh
crew handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/
```

`crew handoff` reuses dispatch's options, launch, store and `--pane`, but accepts a
plain continuation document instead of a structured implementer brief: it snapshots
the document's full bytes into the run's `input.md` behind a standard continuation
contract and sends that durable input to the worker. The generated prompt still
passes the same brief linter as dispatch. Missing, empty, whitespace-only or
non-UTF-8 documents fail before any launch and leave no run record; later edits or
deletion of the original file cannot change what the worker receives, and the
document is never printed to stdout.

This launches a continuation only: it does not transfer hidden conversation, stop
another writer, change Board task owners, or prove the context was understood, and
the launch result is launch evidence only. Use `--pane` for the existing Claude
attach functionality when in Rex. `--mode review` instructs the receiver to stay read-only; it does not add an OS sandbox. No delegation or
publication happens unless the document or you expressly authorized it.

## Using crew from any agent

crew is a plain CLI, so any agent that can run shell commands can coordinate with it. Each host needs
crew to run with normal user access, because dispatch starts other agent CLIs that use the network and
their own config directories.

- Claude Code: allow `Bash(crew:*)` in settings, or approve crew commands when asked.
- Codex: add `prefix_rule(pattern=["crew"], decision="allow")` to `~/.codex/rules/default.rules`.
  Codex then runs crew outside its sandbox without prompting. Without the rule, read commands work in
  the `workspace-write` sandbox and writes fail with "Operation not permitted".
- Vibe: approve crew commands when asked, or run the session with the `auto-approve` agent.

The skill (`skill/SKILL.md`) is the same for every host; install it as a skill in each one.

Development checks: `cargo build --offline --locked`, `cargo test --offline --locked`,
and `cargo clippy --offline --locked --all-targets -- -D warnings`.
Tests invoke the binary with temporary state and stub executors, never real workers.
Contract tests also run the installed ai-skills launch/status scripts and require `jq`.
CI must supply that script checkout, using `CREW_SKILLS_DIR` to select its `skills/`
directory; missing scripts fail the contract tests rather than silently skipping them.
