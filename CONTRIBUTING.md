# Contributing to herder

## Setup and checks

Use the Rust toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml). The minimum supported Rust version is 1.89, as declared in [Cargo.toml](Cargo.toml). CI runs the test suite on macOS. Tests require `jq` and `ps` on your `PATH`.

Run the same checks as the CI `check` job:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The CI `msrv` job runs `cargo +1.89 check --all-targets --locked` on Linux.

Tests use stub executors and temporary state. Tests must never invoke real Claude, Codex or Vibe executors or use the network.

## Changes and pull requests

- Follow the [Design rules](docs/architecture.md#design-rules). Preserve the JSON envelopes, exit codes and `run.json` contracts, or document deliberate changes in the [reference](docs/reference.md).
- Make changes to the scripts in `vendor/ai-skills/` upstream, then refresh the copies as described in [SOURCE.md](vendor/ai-skills/SOURCE.md).
- Add a line under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md) for every user-visible change.
- Keep pull requests focused and open an issue before starting a larger change. Pull requests require passing `check` and `msrv` jobs and are squash-merged.
- Report security issues privately as described in [SECURITY.md](SECURITY.md).

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in herder by you, as defined in the Apache-2.0 license, shall be dual licensed as [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), without any additional terms or conditions.
