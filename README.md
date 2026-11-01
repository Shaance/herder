# crew

A Rust CLI for coordinating Claude, Vibe and Codex workers from any agent or
person in a Rex terminal. Requires Rust 1.88+, the executor CLIs, and the shared
ai-skills scripts. Vibe/Codex scripts also require Bash and jq.

```sh
cargo install --path . --locked
crew doctor --json
```

Write `task.md` with implementer-core's direct-mode contract blocks: `task`,
`workstream_contract`, `implementation_authority`, `action_safety`,
`default_follow_through_policy`, `verification_loop`, `economy_contract`, and
`structured_output_contract`. Name authorized paths and rejected alternatives.

```sh
crew dispatch --executor codex --brief task.md --repo . --goal feature-1 --path src/
# Use the printed run ID below; check it until the worker finishes.
crew ls --goal feature-1
crew status 20261101-120000-abcd
crew report 20261101-120000-abcd
crew verify 20261101-120000-abcd --check 'cargo test --offline'
# Review evidence, obtain approval through board, then publish as coordinator.
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
Writes use temporary files and rename; concurrent writers are outside the MVP.
`CREW_SKILLS_DIR` defaults to `~/.ai-skills/skills`.
Claude profile precedence is `--profile`, `CLAUDE_CONFIG_DIR`, then explicitly
none (Claude's default). Dispatch detects trust failures without editing trust.

`--pane` uses Rex; Vibe/Codex displays require `watch` with `-x` support.
Extra dispatch arguments after `--` go to the executor, including Vibe arguments
past its launcher's separator; launcher policy flags are not crew flags.
Launcher gate exit 3 preserves `starting`; crew returns 0, not 3, for that outcome.
A late script identity can be recorded by status.
Claude resume requires manual `claude attach <id>` with the recorded profile.
The installed Codex status script reads only attempt 1: after a resumed process
exits, crew reports `unknown` and still reads the correct numbered report.
Fixing that script belongs upstream; crew does not infer completion from a report.
A report is never proof. Verification is separate from lifecycle and approval.

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
Tests invoke the binary with temporary state, scripts and PATH stubs, never workers.
