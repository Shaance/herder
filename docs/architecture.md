# Architecture

herder is a Rust CLI for a coordinator running Claude, Vibe or Codex workers.
It launches and tracks workers, reads their reports, and records verification.
The coordinator chooses the executor and model, reviews results, and handles
approval and publication. See [`src/main.rs`](../src/main.rs) and
[`src/handlers.rs`](../src/handlers.rs) for the command interface and routing;
[`src/dispatch.rs`](../src/dispatch.rs) and [`src/wait.rs`](../src/wait.rs) hold
the dispatch and wait commands, and [`src/output.rs`](../src/output.rs) defines
what commands return to [`src/render.rs`](../src/render.rs).

## Run record

Each run has a herder-generated ID and a directory under
`~/.local/state/herder/runs/<id>/`. `HERDER_STATE_DIR` replaces the default root.
[`src/store.rs`](../src/store.rs) owns directory creation and record IO.

`run.json` holds durable run facts: repository, baseline commit, paths dirty at
dispatch with their staged state, brief location, executor and native identity,
launch state, attempt, resume request IDs, and verification results. The
optional name and goal help coordinators find and group runs.
[`src/domain.rs`](../src/domain.rs) defines the record and its state types.

`input.md` is the stored dispatch brief or generated handoff input. Resume
inputs use `input2.md`, `input3.md`, and so on. Reports use `final.md` for the
first accepted attempt and `final2.md`, `final3.md`, and so on for later ones.
Executor scripts write logs, identities and other evidence in the same directory.
A pending resume keeps the previous accepted attempt and report current.

Dispatch prepares a hidden `.ID.pending` directory, writes `run.json`, then
renames the directory into place before launch. Record updates use a temporary
file and rename while holding the run's advisory `run.lock`. Mutating commands
wait up to two seconds for the lock; readers try once and return recorded state
with busy evidence if it is held. `ls` skips hidden staging directories and
reports a broken published run without hiding other runs.

## Lifecycle and observation

Dispatch lints the input, records the launch intent, then starts the worker.
The native session or thread identity and, where applicable, PID are recorded
when confirmed. A launch gate can leave a run starting while later status calls
recover identity from evidence. Claude acceptance is saved before its gate.
[`src/runs.rs`](../src/runs.rs) owns launch, observation, stop and resume.

| State | Meaning |
| --- | --- |
| `starting` | Launch is recorded but not yet confirmed. |
| `running` | The executor reports active work. |
| `blocked` | The executor reports a wait, such as a Claude permission prompt. |
| `finished` | The executor reports completion. |
| `failed` | Launch or executor work failed. |
| `stopped` | Work stopped or herder confirmed a stop. |
| `unknown` | Evidence is insufficient or conflicts; inspect it before acting. |

Status reconciles the record with a fresh executor query. Unknown observations
include a cause ([`src/domain.rs`](../src/domain.rs)):

- `query_failed`: a query failed, timed out or could not be interpreted.
- `identity_mismatch`: recorded and observed process or session identity differs.
- `record_disagrees`: executor state conflicts with the recorded state.
- `unrecognized_status`: the executor returned a state herder does not recognize.
- `unconfirmed`: launch or resume evidence has not confirmed the expected state.

Executor queries have a ten-second timeout. `herder wait` polls until any run settles, or
all runs with `--all`; unknown at timeout is an error. Stop checks identity
before signaling and may report that the stop was sent without confirming exit.
Resume records a pending attempt before launch. Identity confirmation advances
the attempt, as does a confirmed Codex worker failure before it opens a thread.
An optional request ID makes replay of the same input return the recorded request.
For a scripted resume, `resume_worker_dir` records the path spelling passed to
the worker alongside the attempt reservation. Exact-directory waits use it to
match the worker's command line when that spelling resolves to the selected run.

For a pending Codex resume, Herder reads the numbered `events<N>.jsonl` stream
from the beginning and checks the first newline-terminated `thread.started`
record against the previous thread ID. Acceptance also requires a new PID and
agreement with executor files and the status PID. Without the event, a new live
worker reports `starting`, and a new failed worker advances the attempt as
`failed`. A conflicting event blocks retry.

## Executor adapters

[`src/process.rs`](../src/process.rs) embeds the shell scripts in
`vendor/ai-skills` into the binary. The shared scripts lint briefs and collect
evidence. [`src/scripts.rs`](../src/scripts.rs) runs Vibe and Codex launch,
status and resume scripts, parses their evidence, and checks native identity.
Vibe and Codex launch and resume clear inherited Rex session variables. Models
and extra executor arguments are recorded and passed again on resume.

Claude uses its CLI directly through [`src/claude.rs`](../src/claude.rs):
`claude --bg` starts or resumes a session, `claude agents` supplies status,
and `claude stop` stops it. The chosen profile is recorded and reused. Claude
resume uses the saved session options and rejects an unexpected copied session.

The launch settings register a Stop hook that calls herder with the run directory. Herder reads the latest reply from the hook payload and atomically replaces the attempt report when the reply is nonempty. During a pending resume, the hook writes the new attempt’s file so the launch gate can detect it while readers still see the previous accepted report.

Set `HERDER_SKILLS_DIR` to an ai-skills-shaped directory to replace the embedded
scripts with executable `<owner>/scripts/<name>.sh` files. The override applies
to both shared and executor scripts. Codex resume overrides must write the
numbered `events<N>.jsonl` stream used to confirm the thread. `herder doctor`
checks the required files and tools. In a Rex session, `--pane` adds a Claude
attach pane or a Vibe/Codex wait pane. Rex is optional.

## Verification

[`src/verify.rs`](../src/verify.rs) checks the Git baseline recorded at
dispatch against `HEAD`, staged and unstaged changes, and untracked files.
Renames count as both source and destination. Paths already dirty at dispatch
are excluded only while their contents, and whether and what they have staged
relative to `HEAD`, remain unchanged; records from older versions compare
contents only. A dirty path whose staged state changed is checked even when no
diff lists it. An older herder that rewrites the record drops the staged
snapshot, so later checks compare contents only. Every other changed
path must fall under an authorized `--path`; verification uses dispatch paths
unless paths are supplied to `verify`. A missing baseline fails scope checking.
`--baseline` validates and records a replacement commit for work after a rebase.

Verification also runs the shared evidence collector and each `--check` through
`sh -c` in the repository. The Rust scope check owns path enforcement; the
collector records a worktree summary. All checks must exit zero for a pass.
The verdict is attached to the attempt that was current when checks began;
checks run without the run lock, then the record is reloaded and updated.
Protected verification (`--accept-with`, [`src/guard.rs`](../src/guard.rs)) is
the exception: it keeps the run lock through the host check until the receipt
is saved, repeats the scope audit only after a successful host check, and saves
its first verdict as an immutable receipt.
Verification refuses starting, running and blocked workers, but can run with
unknown evidence. It neither freezes the worktree nor approves the result.

With `dispatch --check`, the dispatch process keeps the check commands and original
checkout, baseline and attempt in memory. It owns the post-launch deadline
and starts verification only after that attempt finishes. Codex also requires
`turn.completed` from the dispatched attempt's own event stream. The check processes
run outside the run lock. Herder checks that the attempt is still current between
commands and under the lock before saving a verdict. The remaining deadline
bounds lock acquisition for the verdict. Plain observation never runs
verification commands.

## Handoff

`herder handoff` accepts a nonempty UTF-8 continuation document. It adds a standard
contract, stores the document's bytes in `input.md`, lints the result and uses
the dispatch launch path. Review mode instructs the receiver to stay read-only.
For Codex, dispatch also records the read-only sandbox flag in `extra`, which the
launcher replays on resume. Invalid input fails
before launch. The coordinator supplies the context and stops its own edits;
launch alone cannot transfer hidden conversation or prove understanding. See
[`src/handoff.rs`](../src/handoff.rs) and [`src/dispatch.rs`](../src/dispatch.rs).

## Design rules

The local MCP companion exposes the same checkout and run records to several clients. The host owns routing and write permission. The CLI owns request replay and verification. [MCP delegation decision](decisions/0002-mcp-delegation.md)

- When parsing input, resolving identity, or committing a result establishes a fact, pass that fact to its consumer in the type instead of re-deriving it, re-reading it, or calling `expect`. Add a type only when it deletes an existing re-check, re-read or `expect`. The rule covers facts herder itself established; checks of external state that can change, such as executor files and the process table, stay where the state is used.
