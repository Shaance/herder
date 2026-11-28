# Local Herder MCP companion

`herder-mcp` lets a coding agent commission a read-only Codex review, wait for the worker, and read its report over stdio. The operator can also enable implementation with an independent acceptance check. The host sets the checkout, Herder executable, model, and reasoning effort. Clients configured with the same checkout and Herder state directory can share run IDs.

## Install

Starting with v0.4.0, Homebrew installs both binaries on macOS. The GitHub release's signed, notarized `herder-macos-universal.tar.gz` also contains both.

```sh
brew install shaance/tap/herder
```

To get both binaries from crates.io, install the two packages separately. Install matching versions of the CLI and companion; herder-mcp reads the CLI's `observed_attempt` field and reports an older CLI as unavailable. Checkout inspection, bounded waits and opt-in implementation require v0.5.0 or later.

```sh
cargo install --locked herder
cargo install --locked herder-mcp
```

To install from source, run these commands from the root of the Herder checkout.

```sh
cargo install --locked --path .
cargo install --locked --path mcp
```

## Register a checkout

Run `git rev-parse --show-toplevel` in the checkout to review. Use that same absolute root as `--repo` in both clients. Set the absolute paths to both executables from `$(brew --prefix herder)/bin` for Homebrew, or Cargo's installation directory (usually `~/.cargo/bin`) for Cargo. Using one directory keeps an older CLI elsewhere on `PATH` from being selected.

Keep machine-specific configuration local. To ignore untracked copies of `/.codex/config.toml` and `/.mcp.json`, add those paths to the file shown by `git rev-parse --git-path info/exclude` in the checkout.

Choose a worker model and an effort it supports. The worker needs `codex`, `jq`, Bash, Perl and Git on the inherited `PATH`. Both clients need access to the same Herder state directory. The default is `~/.local/state/herder`. For a custom directory, [set it explicitly for both clients](#use-a-custom-state-directory). Keep `TMPDIR` outside the checkout.

The [optional Herder skill](../README.md#add-the-herder-skill) gives the agent guidance on when to use these tools or the CLI.

## Codex

Add this server to `.codex/config.toml` at the Git root. Start Codex from that checkout, trust the project when prompted, and check `/mcp` for the `herder` server. Codex skips project configuration in untrusted checkouts.

```toml
[mcp_servers.herder]
command = "/path/to/bin/herder-mcp"
args = ["--herder", "/path/to/bin/herder", "--repo", "/absolute/git/root", "--model", "gpt-6.2-sol", "--effort", "high"]
```

## Claude Code

Save the following project configuration as `.mcp.json` at the same Git root. Start Claude Code from the checkout and approve the `herder` server when prompted.

```json
{
  "mcpServers": {
    "herder": {
      "type": "stdio",
      "command": "/path/to/bin/herder-mcp",
      "args": ["--herder", "/path/to/bin/herder", "--repo", "/absolute/git/root", "--model", "gpt-6.2-sol", "--effort", "high"]
    }
  }
}
```

For a one-off session, pass the same Codex `mcp_servers.herder` keys with `codex -c`, or save the Claude configuration outside the checkout and run `claude --strict-mcp-config --mcp-config /absolute/path/to/herder-mcp.json`.

If Codex skips the configuration for a linked worktree, check its trust notice. The notice may name the primary checkout. You can also use the session-local `-c` settings.

## Use a custom state directory

In Codex's `.codex/config.toml`, add this table for the registered server.

```toml
[mcp_servers.herder.env]
HERDER_STATE_DIR = "/absolute/path/to/herder-state"
```

In Claude Code's `.mcp.json`, add `"env": {"HERDER_STATE_DIR": "/absolute/path/to/herder-state"}` inside the `mcpServers.herder` object. Use the same absolute path in both configurations and reconnect both clients. Set `HERDER_STATE_DIR` to that path in the environment where you run the Herder CLI as well.

## Check your registration

Paste this prompt into your coding agent after connecting Herder.

> Use Herder’s `checkout` once. Compare its configured directory and Git root with `git rev-parse --show-toplevel` from my current directory. Show whether both match and explain how to correct the registration if either differs. Report the branch or checkout state, full HEAD if available, and the configured worker model and effort. Tell me whether implementation is enabled. If it is, list the prefixes Herder audits after the run and say whether they block writes. Stop after inspection.

If your client has no shell, include the intended Git root in the prompt. An empty `write_paths` array means implementation is disabled. A nonempty array lists the prefixes for the Git change audit.

For example, you might see “Both paths match `/Users/me/project`. Branch `main` at `6af89e4823ec7f871133cd8fc6a1cc8d69ba8dd8`. Worker `gpt-6.2-sol` at `high` effort. Implementation disabled.”

This inspection uses zero Herder worker quota. Your client model turn can still consume usage. If `checkout` is missing, check that the registered paths point to v0.5.0 or later for both executables, then reconnect the client.

If either reported path differs, set `--repo` to the intended Git root in the client configuration, reconnect, and run the inspection again.

## Review and recovery

Call `checkout` when starting in a new worktree. Compare its `repo` with the result of `git rev-parse --show-toplevel` in the directory where you intend the worker to run. Two linked worktrees can have the same HEAD. Supplying the intended root as `expected_checkout.repo` on `delegate` catches a registration that points to the other one. Optionally include `branch` or the full `head` commit to pin the review more precisely.

- `delegate` takes a coordination goal, fresh run name, task, acceptance criteria, and optional constraints. It launches one sandboxed Codex review and uses Codex quota. The coordinator needs permission to spend quota for the goal. Save the run ID even if the launch reports an error.
- `status` takes a run ID, or the original goal and name together after a lost reply. It returns the observed attempt, worker state, and evidence. An ambiguous or incomplete lookup lists known candidate IDs for inspection.
- `wait` takes a run ID and an optional `timeout_seconds` from 1 to 30 (default 15). It returns the attempt and worker state. On `timed_out`, you can call it again with the same run ID. On `settled`, inspect the state and read any available report, including when the worker failed. If the configured Herder executable stalls, the companion allows two additional seconds for its response before returning `herder_unavailable`.
- `report` takes a run ID and returns any available text with the attempt that owns it, including partial reports from failed workers. `observed_attempt` appears when a pending resume belongs to a different attempt. A missing report is a normal result.

Use a fresh `request_id` on `delegate` if your host might retry after a disconnect. To recover, repeat the same request, including its key. Herder returns the original run ID. Changing the work order under the same key produces `request_conflict`. Without a key, recover through `status` with the original goal and name. A missing match can mean Herder is still publishing the run. Concurrent unkeyed dispatches can start two workers.

## Enable a writer for one worktree

Start with a clean Git worktree dedicated to one work order at a time. Give parallel work orders separate worktrees. Herder returns `checkout_busy` while a previous guarded work order still owns the checkout and awaits completion or verification. Keep the Herder state directory and the acceptance program outside the worker's writable directories. Add both of the following settings to the server arguments in the operator's trusted configuration.

```json
["--write-path", "src", "--write-check", "/absolute/path/to/check-result"]
```

Repeat `--write-path` to allow additional prefixes in the Git change audit. Codex has workspace write access during the run. Herder audits Git-visible changes after the run finishes, and edits stay in the worktree if verification fails. The check must be an executable file. Herder runs it in the worktree with no arguments. Exit zero means the result meets your check. A check that runs worker-edited project code should provide its own isolation because it inherits the coordinator's permissions.

1. Call `checkout` and compare its root and full HEAD with the worktree you intend to use.
2. Call `implement` with a fresh `request_id`, goal, name, task, acceptance criteria, relative `paths` within the approved prefixes, and `expected_checkout` containing `repo` and `head`. Keep the same key and work order if the connection drops.
3. Call `wait` with the returned ID. Once the worker finishes, call `verify` with that ID. The result includes the worker state, Git path audit, host check and overall verdict. `report` gives the worker's account of what changed.

Herder skips the host check if the initial path audit fails, and audits again after a successful host check. The first completed acceptance verdict belongs to the work order. Calling `verify` again retrieves that verdict and its original `verified_at` timestamp even if someone has since changed the files. Use the Herder executable and state directory shown by `checkout` to run `herder status <run-id>` for the worker state and protected acceptance verdict. Add `--json` to inspect the saved checks under `.data.run.protected_verification`. For follow-up work, prepare a clean checkout and dispatch a fresh keyed order. The default read-only registration exposes neither `implement` nor `verify`.

## Tests

From a full repository checkout, build the root CLI first, then run the companion's stdio tests. They use the embedded Herder scripts and real linter, with a stub Codex worker and a disposable state directory.

```sh
cargo build --locked
cargo test --locked --manifest-path mcp/Cargo.toml
```

Licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
