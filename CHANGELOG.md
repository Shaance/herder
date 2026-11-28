# Changelog

All notable changes to herder are listed here. herder follows [Semantic Versioning](https://semver.org/); before 1.0, a minor version can change behavior that scripts depend on.

## Unreleased

- Add `agy` as a CLI executor for Antigravity direct and plan-mode review runs in Gemini API-key mode.
- `herder stop` also stops commands started by Vibe, Codex, and agy workers. After a grace period, surviving commands receive KILL while the worker receives TERM only. Herder reports `stopped` once the worker and recorded survivors are gone. Repeating stop signals survivors again, and a failed process listing reports `unknown`.
- Updated crates.io keywords and categories and the Homebrew description so searches for Claude Code, Codex, MCP, or code review can find herder.
- The skill says to run dispatch in the host's background mode, never with a shell `&` or `nohup`. A non-interactive shell starts `&` children with SIGINT ignored, so the worker and every `--check` command can inherit it, and tests that need Ctrl-C fail. It also says to give the host command a time limit above `herder wait --timeout`.

## 0.7.0 (2026-11-28)

- Dispatch stages exactly the brief bytes it linted. Editing or deleting the brief during a dispatch no longer changes or fails later models of a review. Only a regular, nonempty brief file is read; anything else goes to the linter unread, as before. Custom brief linters receive a private snapshot path outside the checkout and the state directory. On keyed dispatches and handoffs, a lint failure is now reported before staging or Git errors.
- Dispatch and `herder verify` refuse a Git path that is not valid UTF-8 instead of handling it under a lossy spelling: dispatch stops before staging and verify fails the scope check, naming the path. This is reported before checkout pins and protected dispatch's clean-checkout check.
- CLI JSON adds `observed_attempt` beside `observation` on single-run replies and readable `ls` rows. herder-mcp now reads it instead of re-deriving the attempt; install matching versions of Herder and herder-mcp.
- `herder status` no longer accepts, and `herder resume` no longer retries, a Vibe resume whose event stream reports a different session; it reports `unknown` with cause `identity_mismatch`. Before, after the launcher rejected the mismatch, a later status could advance the attempt.
- herder-mcp now relays the CLI's reason when protected verification returns `superseded`, such as another work order taking the checkout, instead of always saying the attempt changed.
- `herder verify` no longer passes scope when a worker changes what is staged for a file that was already dirty and outside its paths, for example by staging its own version and restoring the working copy, or by unstaging the user's edit. Dispatch records each dirty path's staged state relative to `HEAD` as `dispatch_staged` in `run.json` and the run JSON, for dirty checkouts only. A rebase that restores the file's contents and staged state still passes `verify --baseline`. Runs dispatched by older versions keep the content-only check.
- `herder verify` without `--check` now reuses a user check even when its command text matches a built-in label such as `collect_evidence.sh`; before, the check was dropped and reuse could fall back to an older verification. Records written before the collector row existed keep the old ambiguity for a command in that position.

## 0.6.1 (2026-11-27)

- Interactive `herder wait` now supports terminal restoration for a confirmed Claude worker. The restored wait observes the same recorded worker.
- `herder wait` shows the report path when a running Claude worker saves a readable report. The wait continues tracking the worker.

## 0.6.0 (2026-11-27)

- Terminals that support the Program Resume Protocol (OSC 7577) can reopen a foreground `herder wait` for one Codex or Vibe run after a restart. The monitor reconnects to the original run, even if you started the wait by name or changed your working directory. Rex users can opt in with a trust rule.
- Use `herder wait --run-dir PATH` to open a monitor for an exact run directory. It uses Herder's embedded status scripts and works independently of the current state-directory setting.
- When a terminal starts Herder without `jq` on its `PATH`, the bundled status checks preserve the saved report and explain the missing dependency.

## 0.5.0 (2026-11-27)

- Inspect the registered worktree, worker model, effort and write settings with MCP `checkout` before spending worker quota. Use `wait` for up to 30 seconds at a time and reopen a report by run ID. The CLI exposes the same checkout through `herder context` and supports repository-scoped observations with `--repo`.
- Pin a review to its intended worktree and retry a dispatch with the same `request_id` to recover the original run ID.
- Opt in to MCP implementation in a dedicated clean worktree. The operator chooses the path prefixes to audit and the executable that checks the result. `verify` records the worker state, Git path audit and acceptance result for the run.
- The optional Herder skill explains when to use MCP or the CLI. The setup guide covers personal and team installation and shows how to share a run between Claude Code, Codex and the CLI.
- MCP clients now see the companion's name and package version when they connect.

## 0.4.0 (2026-11-26)

### Added

- **Local MCP reviews.** Claude Code and Codex can commission a read-only review and share its recorded status or report through `herder-mcp`.
- **Homebrew install.** `brew install shaance/tap/herder` installs both signed, notarized universal executables from one archive. Each release tag updates the tap.
- **Separate crates.io installs.** `cargo install --locked herder` installs the CLI on its own. Install `herder-mcp` separately for the MCP tools.

## 0.3.0 (2026-11-25)

### Added

- **`herder wait` reports the last available progress when it times out.** Human diagnostics go to stderr and JSON adds per-run progress for unsettled attempts.
- **`herder dispatch --check` waits for one worker to finish and verifies its dispatched attempt.** The coordinator supplies the commands at launch, and `--check-timeout` bounds post-launch supervision and verification.

- **`--effort <LEVEL>` on `dispatch` and `handoff`** sets reasoning effort for Codex and Claude runs (`low`, `medium`, `high`, `xhigh`, `max`, `ultra`). Codex receives it on every launch and resume; Claude receives it at launch and keeps it on resume. Vibe rejects the flag with exit 2, and so does a dispatch that also sets native effort after `--`. The level is saved as `requested_effort` in `run.json` and shown by `status` and `ls`.

### Changed

- **New Claude runs capture the report from the last reply.** Only the repository needs to be trusted. Claude runs recorded by earlier versions resume without a report: their saved options lack the hook, and the resume prompt no longer names a report file. Dispatch them afresh instead of resuming.
- **Codex review runs are read-only.** `--mode review` on a Codex dispatch or handoff saves `--sandbox read-only` with the executor arguments and replays it on every resume. Arguments that would widen the sandbox (`--sandbox` or `sandbox_mode` with another value, bypass or auto-approve flags) exit 2 before a run is recorded. Review runs recorded by earlier versions without a saved sandbox flag resume with the launcher's default sandbox.
- **`herder ls` has an `EFFORT` column** after `MODEL`, and piped run rows end with an effort field (`-` when unset). Scripts that split piped `ls` rows by position need updating.

### Fixed

- Codex resumes check the worker's `thread.started` event before accepting its thread identity.
- Claude dispatch and status still identify the session when `claude agents` prints warnings to stderr.
- Claude sessions that are just starting now show as running, so `herder wait` no longer exits early with a blocked result. Unrecognised idle states report as unknown.
- The skill's background recipe now attempts the report and returns `herder wait`'s exit status.
- The skill and dispatch help no longer claim that `-i` swallows Codex resume arguments.
- While a Codex resume waits for its thread to open, `resume`, `ls` and `status` show the new attempt number beside its state instead of the previous attempt's.
- A resumed Codex worker records its own PID before it starts, so a launcher killed right after starting it can no longer leave a live worker unrecorded for a later resume to duplicate.

## 0.2.0 (2026-11-18)

### Changed

- **`herder wait` exits 1 when a run did not finish.** It exits 0 only when every run that satisfied the wait is `finished`. A failed, stopped or blocked run now exits 1, as a timeout already did, so `herder wait X && next` stops after a failure. Scripts that relied on exit 0 for any settled run need updating.
- **`herder verify` reuses earlier checks.** Without `--check`, verify reruns the commands from this run's latest verification. If the run has none, it takes them from the most recent run with the same goal and repository. Human output names the run the checks came from. An explicit `--check` still wins.
- **Parallel Claude dispatches start one at a time.** Claude Code 2.1.327 can kill one of two `claude --bg` sessions started at the same moment. herder now holds a lock from launch until the session has started, so a second dispatch waits about a second. This is a workaround for the Claude bug and will be removed once it is fixed.
- The verify scope row reads `scope  failed` instead of `scope  baseline scope  failed`.

### Fixed

- **Resuming a Claude run right after it finished** always failed with "Claude resume created a copy". herder now waits for the stopped Claude process to exit before resuming.
- **Executor queries timed out under load.** The deadline is now 10 seconds instead of 2, which also stops `status` reporting `unknown` and Claude launches failing on a busy machine.
- `dispatch --pane` on a deferred Vibe or Codex launch opens the wait pane instead of failing with "pane requires native identity".
- The Claude trust remediation is only shown for Claude runs.
- `status` reads the event stream from the directory it loaded the run from, so a moved `HERDER_STATE_DIR` keeps working.
- `doctor` with a non-UTF-8 `--profile` reports an error with exit 1 instead of panicking.
- The brief lint no longer rejects read-only review briefs that mention new types or fields.

### Internal

- Domain facts are carried in types across doctor, verify, render, run storage and launch, removing re-checks and unreachable branches. JSON output, human output, exit codes and `run.json` are unchanged, checked against 240 recorded outputs.
- Tests wait on conditions instead of fixed sleeps and pass with three suites running at once.
- Releases publish to crates.io from the release workflow with Trusted Publishing. GitHub Actions moved to v7 and the MSRV check runs on Linux.

## 0.1.0 (2026-11-18)

First public release.
