---
name: crew
description: Use when coordinating other agents in Rex or asked to dispatch, track, read reports from, or verify agent runs across executors.
---

## When to use

Use crew to dispatch Claude, Vibe or Codex workers and keep their run identities,
evidence, verification results and workflow issues together. Any coordinating
agent or person can use the same CLI.

## The loop

1. Run `crew doctor` for the intended executor and profile.
2. Write a direct-mode brief outside the worker repository with implementer-core's contract blocks, authorized
   paths, verification steps and publication limits. Name every skill the worker
   must load (for example `$write-rust`); workers do not reliably load them unasked.
3. Use `crew dispatch`; retain the printed run ID and optional board goal string.
4. Follow progress with `crew ls` and `crew status`; read `crew report`.
5. Inspect the actual changes and run `crew verify` with scope paths and checks.
   Resolve each review finding or record its approved deferral.
6. Obtain approval via board. The coordinator publishes after approval.

## Rules

- Route work according to AGENT_DEFAULTS; crew never selects an executor.
- A worker report is never proof. Verify the actual output independently.
- Publish only after approval; crew does not publish or grant approval.
- Treat `unknown` as unresolved evidence, not completion or permission to retry.
- Preserve recorded identities and profiles. Consult command help for stop,
  resume, dispatch's `--pane` flag and executor limitations.
- When something surprises you, `crew issue log` it and continue with work that
  remains safe and understood. Do not paper over an executor disagreement.

All flag details and examples live in `crew <command> --help`.
