<task>
Apply exactly these three review fixes on branch codex/mvp (current HEAD). Each was confirmed by the coordinator against the code.

1. P2 contract: `src/handlers.rs:90` rejects `--profile` with a non-Claude executor via `ensure!`, which exits 1. The contract says usage errors exit 2. Return exit 2 the same way the verify path-precheck in handlers.rs already does.
2. P2 representable invalid state: script-adapter functions in `src/scripts.rs` take `Executor`, so Claude can reach them and is rejected at runtime (`unreachable!` at :17, `bail!` at :32). The same file branches on `executor == Executor::Vibe` five times (:36, :50, :64, :87, :105) to choose per-executor constants (skill dir, script prefix, identity file, launch-output key, status suffix handling). Make Claude unrepresentable in this module with a type that only admits Vibe and Codex, carrying or returning its constants, so the `unreachable!`, the `bail!` and the repeated if/else branches are deleted. Callers convert once at the boundary where the native identity or target is already known.
3. P3 duplicate rule: `src/verify.rs:11` repeats the "at least one --path" check that handlers.rs enforces first with exit 2. Delete the duplicate.
</task>

<verification_loop>
cargo build --offline --locked
cargo test --offline --locked
cargo clippy --offline --locked --all-targets -- -D warnings
grep -n "unreachable!\|Executor::Vibe" src/scripts.rs   (expect no unreachable!, and no repeated executor equality branches)
</verification_loop>

<economy_contract>
Net production line delta must be zero or negative. Add at most one table row or test for fix 1 (exit 2 on `dispatch --executor vibe --profile X`). No new files, traits or generics. Rejected review items, do not apply: removing the attempt-overflow check (harmless), removing doctor's `watch` check (stock macOS lacks watch and --pane needs it).
</economy_contract>

<constraints>
All previous scope limits, interfaces, and invariants remain unchanged. Do not commit.
</constraints>

<structured_output_contract>
Report under 20 lines: each fix with file:line, production lines before/after (`wc -l src/*.rs` total), test results.
</structured_output_contract>
