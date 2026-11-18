# 0001: Keep the Vibe CLI adapter

Status: accepted, 2026-11-08

## Context

herder needs detached Vibe workers whose status, reports and session identity can
be queried after the dispatch command exits. Resume and cancellation must keep
uncertain outcomes visible. The current adapter uses Vibe's CLI through the
vendored launch, status and resume scripts.

## Options considered

1. Keep the existing CLI adapter and its shared Codex/Vibe script integration.
2. Use Vibe's Agent Client Protocol (ACP) with a persistent per-job supervisor
   to own the ACP connection, expose status and handle cancellation.

## Decision

Keep the CLI adapter. The ACP experiments showed a viable direction, but did
not demonstrate enough maintenance benefit to take on the supervisor, local
IPC, recovery, permission handling and identity checks it would require.
The shared script adapter would still be needed for Codex.

## Evidence

On macOS with Vibe and Vibe ACP 3.1.0, real sessions streamed a response,
loaded saved history in a new process and cancelled a turn. A client-owned ACP
process did not continue after its client exited. A separate prototype worker
kept a turn running after dispatch exited and accepted a later cancellation.
A follow-up assessment exercised configuration, permissions, recovery and local
lifecycle with 14 provider prompts. It found that a second process could not
load an owned session, and that interrupted sessions could be loaded again.

The prototype still needed production error handling and safer stop routing.
The tests did not establish safe replay of interrupted side effects, remote
provider cancellation, host-crash durability, or exhaustive race behavior.
A 170-line early worker preceded those requirements; the Vibe scripts totaled
192 lines and the 136-line shared adapter also served Codex. The Rust ACP SDK
was inspected but not integrated or exercised by herder.

## Reconsider when

Revisit ACP for a concrete benefit, such as shared support across multiple ACP
executors. A production proposal should demonstrate configuration and argument
parity, limits, identity-safe cancellation, failure diagnostics, interruption
recovery and version compatibility. Executor completion remains separate from
herder's repository verification.
