# Local Herder MCP companion

`herder-mcp` exposes three tools over stdio for independent read-only reviews of one checkout. The host sets the checkout, Herder executable, Codex worker model, and reasoning effort. Both hosts can retrieve the same run when they launch the companion with the same checkout and Herder state directory.

## Install

Starting with v0.4.0, Homebrew installs both binaries on macOS. The GitHub release's signed, notarized `herder-macos-universal.tar.gz` also contains both.

```sh
brew install shaance/tap/herder
```

To get both binaries from crates.io, install the two packages separately. If you already have the v0.4.0 CLI, install only `herder-mcp`.

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

Choose a worker model and an effort it supports. The worker needs `codex`, `jq`, Bash, Perl and Git on the inherited `PATH`. Both clients need access to the same Herder state directory, either the default `~/.local/state/herder` or an inherited absolute `HERDER_STATE_DIR`. Keep `TMPDIR` outside the checkout.

## Codex

In the trusted checkout, add this server to `.codex/config.toml` at the Git root. Start Codex from that checkout and check `/mcp` for the `herder` server.

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

## Review and recovery

- `delegate` takes a coordination goal, fresh run name, task, acceptance criteria, and optional constraints. It launches one sandboxed Codex review and uses Codex quota. The coordinator needs permission to spend quota for the goal. Save the run ID even if the launch reports an error.
- `status` takes a run ID, or the original goal and name together after a lost reply. It returns the observed attempt, worker state, and evidence. An ambiguous or incomplete lookup lists known candidate IDs for inspection.
- `report` takes a run ID and returns any available text with the attempt that owns it, including partial reports from failed workers. `observed_attempt` appears when a pending resume belongs to a different attempt. A missing report is a normal result.

When a dispatch reply is missing or uncertain, check `status` with the supplied goal and name. A missing match can mean Herder is still publishing the record. Duplicate-name checking is advisory because concurrent callers can race before publication. Keep dispatch disabled in any host that automatically replays it after transport failure.

## Tests

From a full repository checkout, build the root CLI first, then run the companion's stdio tests. They use the embedded Herder scripts and real linter, with a stub Codex worker and a disposable state directory.

```sh
cargo build --locked
cargo test --locked --manifest-path mcp/Cargo.toml
```

Licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
