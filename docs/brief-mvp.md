<task>
Build the MVP of `crew`, a Rust CLI that lets any coordinating agent (Claude Code, Codex, Vibe, or a person in a
Rex terminal pane) dispatch work to Claude, Vibe or Codex workers, track them, read their reports, verify their
output and log workflow problems. Read `docs/design.md` first: it is the frozen product design. Where it and this
brief differ, this brief wins (the design predates the Rust decision).

Repo: current directory, a fresh `cargo init` (binary `crew`), toolchain 1.88.0. Dependencies are already
fetched: clap (derive), serde, serde_json, anyhow; dev: tempfile. Build and test with `--offline`.
Authorized paths: `src/**`, `tests/**`, `skill/SKILL.md`, `README.md`, `Cargo.toml` (only to add `[[test]]`
or features of the existing deps; no new crates), `Cargo.lock`.

Executors crew wraps (read their scripts and help yourself; re-verify every claim here and adapt if one is wrong):
- Vibe: `$CREW_SKILLS_DIR/vibe-implementer/scripts/launch_vibe.sh <repo> <brief> [run-dir] [--strict] [--allow CMD]... [-- vibe args]`
  prints `RUN_DIR=… PID=… SESSION_ID=…`, exit 0 live / 1 failed / 3 gate elapsed but alive.
  `status_vibe.sh <run-dir> [suffix]` prints `state=… pid=…`, `outcome=…`, `verdict: …` lines and writes `final.md`
  (or `final<N>.md` for resume suffix N) once `out*/export.json` exists. `resume_vibe.sh <run-dir> <repo> <delta> [suffix]`.
- Codex: `$CREW_SKILLS_DIR/codex-implementer/scripts/launch_classic.sh <repo> <brief> [run-dir] [extra codex args]`
  prints `RUN_DIR=… PID=… THREAD_ID=…`. `status_classic.sh <run-dir>`, `resume_classic.sh <run-dir> <repo> <delta>`.
  Codex writes `final.md` in the run dir.
- Both launchers accept crew's own run dir as the third argument, so evidence lands in crew's run dir.
- Brief lint: `$CREW_SKILLS_DIR/implementer-core/scripts/check_brief.sh <brief> direct`;
  scope check: `collect_evidence.sh <run-dir> <repo> <authorized-path>...` (exit 2 = touched file outside paths).
- Claude Code background sessions: `claude --bg "<prompt>" --model <m> -n <name> [--add-dir D]... [--permission-mode M]`,
  run with cwd = repo and env `CLAUDE_CONFIG_DIR` = the run's profile. Success prints
  `backgrounded · <short-id> · <name>`. Known failure modes observed today:
  (a) untrusted folder prints `Workspace not trusted. Run \`claude\` in <path> once and accept the trust prompt, then retry.`
  (b) if the prompt is not the first positional argument, variadic flags such as `--disallowedTools` swallow it and the
  output says `(idle — send a prompt to start)`: treat that as a failed launch.
  Status: `claude agents --json --all` returns an array of objects with `id` (short), `sessionId`, `name`, `cwd`,
  `kind`, `status` (`busy`/`idle`), `state` (e.g. `working`), `pid`, `startedAt`. Stop: `claude stop <id>`.
  Check `claude --help` for a non-interactive way to send a follow-up to an existing background session; if none
  exists, `crew resume` on a Claude run fails with a clear message naming the manual alternative (`claude attach <id>`).
  Claude workers do not write a report file on their own: crew appends to the prompt an instruction to write the
  final report to `<run-dir>/final.md`, and passes `--add-dir <run-dir>` so they can.
- Rex: `rex split -b <block> --focus=false -- <cmd...>` opens a pane; `$REX_BLOCK` is the caller's pane.
</task>

<workstream_contract>
Commands (clap subcommands; each `--help` has a one-line purpose and at least one example; every command supports
`--json` with a stable shape; no interactive prompts; exit 0 success, 1 operational failure, 2 usage error):

- `crew doctor [--executor claude|vibe|codex]`: report, per executor, whether its executable/scripts exist, the
  resolved Claude profile (`--profile` flag > `CLAUDE_CONFIG_DIR` env > none, printed explicitly), `rex` reachability,
  and the exact fix for each gap. Do not read or edit Claude's internal trust files; trust is detected at dispatch.
- `crew dispatch --executor E --brief FILE --repo DIR [--model M] [--goal G] [--name N] [--profile DIR] [--pane]
  [-- extra executor args]`: lint the brief with check_brief.sh (Claude briefs too), mint a run id
  (`YYYYMMDD-HHMMSS-<4 hex>`), write `run.json` BEFORE spawning (state `starting`), launch, record the native id,
  print the run id. On a launch failure, record `failed` with the executor's message and the remediation (trust
  message → "open `claude` in <repo> under profile <p> once and accept trust"). `--pane` opens a Rex pane showing
  the run (Claude: `env CLAUDE_CONFIG_DIR=<p> claude attach <id>`; Vibe/Codex: a loop over the status script).
- `crew ls [--goal G] [--json]` and `crew status RUN`: re-query the executor every time; never cache derived state.
  States: starting, running, blocked, finished, failed, stopped, unknown. If crew's record and the executor
  disagree or the executor cannot be queried, print `unknown` with both facts.
- `crew report RUN`: print the current attempt's report path and content (`final.md`, or `final<N>.md` after
  resume N). Missing report → exit 1 with "no report yet" and the run state.
- `crew stop RUN`: stop the exact recorded identity (Vibe/Codex: `kill -TERM` the recorded pid only after
  confirming it is still that run; Claude: `claude stop <id>` with the run's profile).
- `crew resume RUN --brief DELTA`: exact-identity resume through the executor's resume script; increments `attempt`.
- `crew verify RUN [--path P]... [--check CMD]...`: run collect_evidence.sh with the authorized paths (default: the
  ones recorded at dispatch if given via `--path` there, else require `--path`), then each `--check` in the repo via
  `sh -c`; record a verdict object (`pass`/`fail`, each check's exit code, timestamp) on the run. Verification is a
  fact on the run, not a state.
- `crew issue log --title T --observed O --expected E [--evidence X]... [--area A] [--run RUN]`, `crew issue ls
  [--open] [--json]`, `crew issue resolve ID --note N`. Area values: `crew`, `executor:<name>`, `skill:<name>`, free text allowed.

State: `$CREW_STATE_DIR` (default `~/.local/state/ai-skills/crew`). `runs/<run-id>/run.json` is the only authority
for a run's recorded facts (id, executor, model, profile, repo, brief copy path, goal, name, native ids, pid, attempt,
authorized paths, verify verdicts, created_at, launch error). Executor evidence lives in the same run dir.
`issues.jsonl` is the only authority for issues (append-only events; `ls` folds them). Writes are atomic
(write temp file, rename). `CREW_SKILLS_DIR` (default `~/.ai-skills/skills`) locates the executor scripts; both
env overrides exist so tests can run against stub executables, and are the only test hooks allowed.

`skill/SKILL.md`: under 60 lines, frontmatter `name: crew` and a `description` that triggers when an agent
coordinates other agents in Rex or is asked to dispatch/track agent runs. Sections: when to use, the loop
(doctor → brief → dispatch → ls/report → verify → approval via board → coordinator publishes), rules (a worker
report is never proof; publish only after approval; routing per AGENT_DEFAULTS), and "when something surprises
you, `crew issue log` it and continue". All flag detail stays in `--help`. Must not mention Claude-only tools.

`README.md`: install (`cargo install --path . --locked`), one end-to-end example, state locations. Under 80 lines.

Non-goals: board integration beyond storing an optional `--goal` string; dashboards; scheduling; auto-retry;
executor auto-selection; cost accounting; parsing `claude logs`; publishing commands.
Rejected alternative: extending `board.py` (Python, decisions not runs) was rejected by the user's Rust decision
and because board records approvals, not executor lifecycles; `skill-feedback` was rejected as the issue store
because it requires user confirmation per record. New surfaces: the `crew` binary, its state dir, and the
`skill/SKILL.md` file, all named in docs/design.md.
</workstream_contract>

<implementation_authority>
Direct mode. You own the internal design inside this contract. Suggested split by job, each file under ~250
lines: `main.rs` (clap + dispatch to handlers), `store.rs` (run records and issues), `claude.rs`, `vibe.rs`,
`codex.rs` (one adapter each). Vibe and Codex adapters shell out to the existing scripts; never re-implement
their flags, gates or report extraction. Use `std::process::Command` with argument vectors, never a shell
string, except for user-supplied `--check` commands.
</implementation_authority>

<action_safety>
Work only in the authorized paths. Do not commit, push, install, release, mutate external systems, expose
secrets, or perform destructive cleanup. Do not launch real Claude, Vibe or Codex workers from tests or
verification; tests use stub executables on a temp PATH and a temp `CREW_SKILLS_DIR`/`CREW_STATE_DIR`.
Do not load implementer or delegation skills; you are the implementer.
</action_safety>

<default_follow_through_policy>
Use the most reasonable low-risk interpretation and keep going. Ask only when missing information changes
correctness, safety, scope, or an irreversible action. If an executor fact above is wrong, follow reality and
say so in the report.
</default_follow_through_policy>

<verification_loop>
cargo build --offline --locked
cargo test --offline --locked
cargo clippy --offline --locked --all-targets -- -D warnings
cargo run --offline -q -- --help ; cargo run --offline -q -- dispatch --help ; cargo run --offline -q -- issue --help
CREW_STATE_DIR=$(mktemp -d) cargo run --offline -q -- issue log --title t --observed o --expected e --json
CREW_STATE_DIR=<same> cargo run --offline -q -- issue ls --json
cargo run --offline -q -- doctor --json   (real environment, read-only; report whatever it finds)
</verification_loop>

<economy_contract>
Budget (changed lines): production 900-1300 in src/; tests at most 1.2x production; README+SKILL under 140.
At 80% of production budget, stop adding features and finish what exists. No new crates. No traits or generic
executor abstraction unless it removes duplication across all three adapters (an enum + match is fine). No retry
loops, caches, lock files or background threads: concurrency between two crew writers on the same run is out of
scope; atomic rename is enough. Behavior-matrix tests through the real binary (`CARGO_BIN_EXE_crew`) against
stub executables: dispatch success and each failure mode (trust, idle-no-prompt, lint failure) for Claude;
dispatch+status+report for Vibe and Codex via stub scripts; resume picks final<N>.md; stop; verify pass/fail;
issue log/ls/resolve fold; `unknown` when the executor can't be queried. One test per behavior; table-drive
variants. Stop when verification passes; list further ideas in the report instead of building them.
</economy_contract>

<structured_output_contract>
Report under 40 lines: outcome; budget table expected vs actual (production, tests, docs); files with one-line
purpose each; new-artifact ledger (each type/module and the rejected alternative); executor facts from this brief
that turned out wrong; verification command results; deliberate omissions; residual risk.
</structured_output_contract>
