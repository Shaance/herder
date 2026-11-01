<task>
Fix the problems found by running the installed `crew` (master f64d054) against the real environment. Repo:
current directory (Rust, toolchain 1.88.0, build with `--offline`). Read `docs/brief-mvp.md` for the original
contract; it still holds except where this brief changes it.

Skills: load and follow `$write-rust` before editing, and keep `$make-states-unrepresentable` as a design rule
(parse once at the boundary; no new Option fields whose combinations the domain forbids). Say in the report
which skills you loaded.

Findings to fix (re-verify each; skip and say so if a premise is wrong):
1. P1, src/runs.rs `observe`: a run whose launch failed (`launch.state = failed`, e.g. the Claude trust
   refusal) is reported as `state: unknown` with evidence "executor query failed: no recorded Claude
   identity". A failed launch is terminal and has nothing to query: report `failed`, carrying the recorded
   error message and remediation as evidence. Reproduce with a stub `claude` that prints the trust message.
2. P2, src/render.rs: without `--json`, `doctor`, `ls`, `status` and `issue ls` print pretty JSON. Give each
   a short human rendering: `ls` one line per run (id, name, executor, state, attempt); `status` a few
   `key: value` lines (state, evidence, executor, attempt, run dir, report path, latest verdict); `doctor`
   one line per check (ok/missing, item, fix only when missing); `issue ls` one line per issue (id, area,
   title, resolved or open). Keep `--json` shapes byte-for-byte unchanged. Rendering stays in render.rs;
   choose the rendering from the parsed command, not by sniffing JSON keys.
3. P2, src/main.rs help: most flags and positionals have no description, and `--executor` does not list its
   values. Every flag and positional gets a one-line description; `--executor` shows claude, vibe, codex
   (clap `ValueEnum` on the domain enum is acceptable only if it keeps clap out of domain.rs; otherwise
   `value_parser` with possible values in main.rs); `<RUN>` says "run id, e.g. 20261101-120000-abcd";
   `<ID>` says "issue id, e.g. issue-0001"; repeatable flags say "(repeatable)"; top-level help names the
   env vars `CREW_STATE_DIR`, `CREW_SKILLS_DIR`, `CLAUDE_CONFIG_DIR` and lists every exit code the binary
   actually returns (README mentions exit 3: confirm in code and make help and README agree).
4. P3, src/doctor.rs: `pane_loop.fix` and `rex.fix` are set even when the check passes; make them null when
   ok, like the executor checks.
5. P3, src/doctor.rs: `rex.detail` embeds raw `rex whoami --json` output as an escaped string (including
   the tailnet login). Parse it at the boundary and keep only session_id and block_id, or drop detail.
6. P3, src/main.rs: `crew issue log --observed '--disallowedTools ate the prompt'` fails with "unexpected
   argument". Free-text values (`--title`, `--observed`, `--expected`, `--evidence`, `--note`) accept values
   that start with `-`.
Authorized paths: `src/**`, `tests/**`, `README.md`, `skill/SKILL.md`.
</task>

<workstream_contract>
CLI names, flags, JSON shapes, state files and exit codes stay as they are, except: failed launches now
observe as `failed` (finding 1) and doctor fields from findings 4 and 5. No new commands or flags.
Rejected alternative: none, no new surface.
</workstream_contract>

<implementation_authority>
Direct mode. Smallest coherent fix per finding inside the existing files and layers (domain without IO,
adapters/store, CLI/render). No new modules unless render.rs would exceed ~250 lines.
</implementation_authority>

<action_safety>
Work only in the authorized paths. Do not commit, push, install, release, or launch real Claude, Vibe or
Codex workers; tests use the existing stub harness. Do not load implementer or delegation skills.
</action_safety>

<default_follow_through_policy>
Use the most reasonable low-risk interpretation and keep going. If a finding's premise is wrong, say so and
skip it.
</default_follow_through_policy>

<verification_loop>
cargo build --offline --locked
cargo test --offline --locked
cargo clippy --offline --locked --all-targets -- -D warnings
cargo run --offline -q -- --help ; cargo run --offline -q -- dispatch --help ; cargo run --offline -q -- issue log --help
CREW_STATE_DIR=$(mktemp -d) cargo run --offline -q -- issue log --title t --observed '--x y' --expected e
cargo run --offline -q -- doctor   (real environment, read-only; paste the human output in the report)
</verification_loop>

<economy_contract>
Budget: net production change in src/ at most +180 lines (help text included). Tests: at most 3 new tests
(failed-launch state; human rendering smoke for ls/status; hyphen value), table-driven where they share setup.
No new crates, traits, or helper modules. Stop when verification passes; list further ideas in the report.
</economy_contract>

<structured_output_contract>
Report under 40 lines: outcome per finding (fixed / skipped with reason); production and test line delta;
files touched with one line each; verification results; the human `doctor` and `crew --help` output;
residual risk.
</structured_output_contract>
