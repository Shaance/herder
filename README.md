# crew

A Rust CLI for coordinating Claude, Vibe and Codex workers from any agent or
person in a Rex terminal. Requires Rust 1.88+, the executor CLIs, and the shared
ai-skills scripts. Vibe/Codex scripts also require Bash and jq.

```sh
cargo install --path . --locked
crew doctor --json
```

Keep briefs outside `--repo`; an untracked brief inside it fails verify scope. Write `/tmp/task.md` with implementer-core's direct-mode contract blocks: `task`,
`workstream_contract`, `implementation_authority`, `action_safety`,
`default_follow_through_policy`, `verification_loop`, `economy_contract`, and
`structured_output_contract`. Name authorized paths and rejected alternatives.

```sh
crew dispatch --executor codex --brief /tmp/task.md --repo . --goal feature-1 --path src/
# Use the printed run ID below; check it until the worker finishes.
crew ls --goal feature-1
crew status 20261101-120000-abcd
crew report 20261101-120000-abcd
crew verify 20261101-120000-abcd --check 'cargo test --offline'
# Review evidence before publishing as coordinator.
crew issue log --title 'Unexpected result' --observed 'What happened' --expected 'What should happen'
crew issue ls --open
crew issue resolve issue-0001 --note 'Resolution and evidence'
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
`issues.jsonl` contains append-only events, folded by `issue ls`.
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

`--pane` uses Rex; Vibe/Codex displays require `watch` with `-x` support.
Extra dispatch arguments after `--` go to the executor, including Vibe arguments
past its launcher's separator. Crew Vibe runs are auto-approve only.
Launcher gate exit 3 preserves `starting` or `pending_resume`; crew returns 0.
A resume requires a non-empty delta; the delta can use `<task>`, `<verification_loop>`
and `<constraints>` without dispatch's full brief contract. The accepted attempt
and report remain current until a different PID with the same executor identity is recorded.
After correcting a recorded pending-resume error, `crew resume` retries the same
attempt using its previous identity when the executor confirms no live worker.
Running or unknown executor states block retry. Executor rejection diagnostics
still apply, including Vibe's refusal to reuse existing attempt evidence files.
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
Dispatch records the Git baseline commit. Verification checks committed, staged,
unstaged and untracked paths against that baseline, then runs the evidence script
and requested commands. Records without a baseline load, but scope verification fails
explicitly. A verdict belongs to its attempt; a resumed attempt starts unverified.
Legacy `starting` intents load as running, and untagged legacy verdicts belong to attempt 1.

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
