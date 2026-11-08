# Vibe ACP experiment

Keep the current Vibe adapter for now. On 2026-11-08, a real ACP session could
stream a response, reload its conversation in a new process, and cancel a turn.
The tested client-owned process exited when the client exited, and session listing
did not expose a live running state. Crew would need a persistent owner and its own
status evidence to preserve detached dispatch semantics. A separate supervisor
could provide those; this experiment does not rule it out.

## Setup and capabilities

The host was macOS Darwin 25.6.0 on arm64, with `vibe 3.1.0`, `vibe-acp 3.1.0`,
Python 3.15.0, and an empty scratch Git repository. The initial sandboxed launch
failed before initialization with `PermissionError` / `Operation not permitted`.
The successful runs used the existing Vibe home and authentication outside the
sandbox. No global configuration was intentionally edited; Vibe session logging
was enabled and the calls created normal session records.

The sanitized `initialize` response contained:

```json
{
  "protocolVersion": 1,
  "agentInfo": {"name": "@mistralai/mistral-vibe", "version": "3.1.0"},
  "agentCapabilities": {
    "loadSession": true,
    "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true},
    "sessionCapabilities": {"list": {}, "fork": {}, "close": {}}
  },
  "authMethodCount": 1
}
```

`session/resume` was not advertised and was not called. The initial session
reported mode `auto-approve`, model `mistral-large-4-0-3`, and thinking `high`.
The configured default agent was `auto-approve`; the probe switched its session
to `ask` before sending a prompt. A real model response confirms the ACP process
could use existing authentication. Full parity with the headless CLI, alternate
profiles, model overrides, and CLI limits remains untested. Crew's current Vibe
launcher explicitly passes `--auto-approve` and saves streaming and export files.

## Observed behavior

| Check | Evidence | Result |
| --- | --- | --- |
| Initialization and prompt | `session/new` succeeded; the turn emitted user, thought, message, and usage updates and returned `end_turn` with the expected reply. | Passed |
| Reload and history | After the first agent exited, a new process used `session/load`; it replayed the earlier user and agent messages, and a second prompt recalled the word from the first. | Passed |
| Listing | `session/list` found the session with `cwd`, `sessionId`, `title`, and `updatedAt`, but no live state field. | Metadata only |
| Cancel and close | During a third prompt, `session/cancel` led to a `cancelled` stop reason. `session/close` then succeeded. | Passed while connected |
| Client exits during work | A separate client exited after receiving the user-message update. Its agent process was present immediately and absent at 1 and 6 seconds. Reload found the user message but no completed assistant reply. | Did not continue in this arrangement |

The disconnect result does not establish whether a provider continued processing
the request remotely. It also does not test a persistent supervisor, a networked
ACP transport, or status and cancellation after an independent client disconnects.

## Reproduction and evidence

The disposable client and sanitized event log are machine-local and may be
removed by temporary-file cleanup:

- `/private/tmp/crew-acp-experiment.ROfmPC/probe.py` is the 175-line stdlib
  JSON-RPC client for initialize, prompt, reload, list, cancel, and close.
- `/private/tmp/crew-acp-experiment.ROfmPC/disconnect.py` starts a separate
  client and observes the agent after that client exits.
- `/private/tmp/crew-acp-experiment.ROfmPC/executed/` preserves the exact two
  script versions used for the successful runs. Use the amended scripts above
  for a rerun; the archived versions record session IDs in their event output.
- `/private/tmp/crew-acp-experiment.ROfmPC/events.jsonl` contains event types,
  capability fields, stop reasons, and boolean checks, without session IDs, prompt
  contents, credentials, or raw provider output. It includes the initial failed
  launches.

These commands were run with the scratch `repo` directory as the working
directory. The Python commands make real model calls using existing Vibe auth:

```sh
rtk proxy vibe --version
rtk proxy vibe-acp --version
rtk proxy vibe-acp --help
rtk proxy python3 /private/tmp/crew-acp-experiment.ROfmPC/probe.py
rtk proxy python3 /private/tmp/crew-acp-experiment.ROfmPC/disconnect.py
```

The probe was adjusted after the recorded runs to report whether its cancel
notification was sent, recheck the advertised load capability, and omit session
IDs from event output. Those changes did not trigger another model call.

The [ACP Rust SDK example](https://github.com/agentclientprotocol/rust-sdk/blob/main/src/agent-client-protocol/examples/yolo_one_shot_client.rs)
shows initialization, session creation, notifications, and prompting. The
[ACP session setup specification](https://agentclientprotocol.com/protocol/v1/session-setup)
defines capability-gated load and history replay. The
[Vibe ACP setup guide](https://github.com/mistralai/mistral-vibe/blob/main/docs/acp-setup.md)
describes using `vibe-acp` with existing Vibe authentication.


## Detached worker follow-up, 2026-11-08

A disposable per-job worker preserved an ACP turn after the dispatch command
exited and accepted a later cancellation. This answers the lifetime objection in
the first experiment for the tested local setup. It does not establish that an
ACP integration would replace Crew's existing Vibe adapter economically or with
matching behavior.

The 170-line Python worker owns the Vibe ACP process and its standard streams.
It writes a local state record and event summaries, and answers `status` and
`stop` requests on a job-local Unix socket. Each dispatch, status, and stop was
a separate process. A socket response confirms a responsive worker and reports
whether its child is alive; a submitted prompt, an observed prompt update, a
provider update, and a terminal response are recorded separately. The worker
stores assistant message text for this benign probe but discards thought text.

| Check | Observed evidence | Result |
| --- | --- | --- |
| Detached lifetime | At 15:13:15 UTC, job A's dispatch exited in 0.092 s. A separate status 0.062 s later saw both worker and agent alive. At 15:13:17 it saw the submitted prompt and the agent's user-message update. | Passed |
| Completion and readback | At 15:13:22, a separate status reported `end_turn`, answer `AMBERWORKER`, agent exit code 0, and an unreachable worker. Two subsequent reads returned the same answer and did not change the event log. | Passed |
| Cancellation | Job B's status saw a submitted prompt, the agent's user-message update, and both processes alive. A separate stop was accepted; the prompt returned `stopReason=cancelled` and the agent exited with code 0. | Passed before provider output |
| Settled status | Source inspection shows the status path reads the record and queries the local socket. The two settled reads made no new worker events, and the worker was unreachable. That path does not launch an agent or call the model. | Passed |
| Worker crash | Job C paused immediately after spawning its agent, before initialize or a model prompt. After checking that the agent was the worker's child, the test killed the worker and status reported `unknown_interrupted`. Both recorded processes subsequently disappeared. | Passed for this pre-prompt crash |

For job B, the worker recorded prompt submission at 1.750 s, the agent's
user-message update at 1.882 s, the cancellation event at 1.976 s, and the
`cancelled` terminal response at 2.004 s, measured from worker start. There was
no provider thought or message update before cancellation, so the test does not
show cancellation after streaming has begun. The worker logs `cancel_sent`
immediately before writing the notification; the terminal response, rather than
the log label or stop acknowledgement, proves cancellation was observed in this
run. All recorded worker and agent PIDs for the three jobs were later checked
with `ps` and were absent. This does not establish what happened remotely after
the provider request was cancelled.

An unreachable socket can also mean startup or a temporarily unresponsive worker.
The prototype calls that state `unknown_interrupted`; it is not proof of process
death. The crash test independently checked the processes. The prototype has no
reconnection or recovery, and its atomic file replacement is not a durability
guarantee across host failure.

### Comparison and remaining work

The current shell adapter already detaches Vibe, keeps stream and export files,
resumes recorded sessions, and checks recorded process identity before sending
TERM. The prototype demonstrates the alternative ownership and ACP cancel
path, but adds a long-lived protocol client, socket, state writes, and cleanup
responsibilities. It does not demonstrate that those costs remove the existing
adapter's maintenance burden. Retaining the current adapter remains the prudent
choice until a real integration has a concrete benefit and covers the missing
behavior.

A production evaluation would need to address Crew's launch and status contract,
worker failure and reconnection, cancellation races and cleanup, secure local
ownership, permission handling, profile/model/argument parity, resume behavior,
and reporting of output and failures. These are untested here. The probe selected
`ask` mode and was set to deny unexpected agent requests; Crew's present Vibe launcher
uses `--auto-approve`, so this run does not establish permission parity.

### Follow-up provenance

The host was Darwin 25.6.0 arm64, Python 3.15.0, `vibe` 3.1.0, and
`vibe-acp` 3.1.0. The agent used ACP protocol version 1 and existing Vibe auth.
The following commands were run after creating a fresh scratch directory; the
last command ran from its empty Git repository outside the sandbox:

```sh
rtk proxy git init --quiet /private/tmp/crew-acp-worker-experiment.LzXfOp/repo
rtk proxy python3 -m py_compile /private/tmp/crew-acp-worker-experiment.LzXfOp/worker.py /private/tmp/crew-acp-worker-experiment.LzXfOp/exercise.py
rtk proxy python3 /private/tmp/crew-acp-worker-experiment.LzXfOp/exercise.py
```

`exercise.py` invokes `worker.py dispatch job-a complete`, then independent
`status job-a` reads; it repeats this with `dispatch job-b cancel`, independent
status and `stop job-b`, then performs the pre-prompt `job-c crash-check`.
Only jobs A and B submit model prompts. Its sanitized timeline is
`/private/tmp/crew-acp-worker-experiment.LzXfOp/observations.jsonl`;
`jobs/job-{a,b,c}/events.jsonl` contain event types and elapsed times, and
`cleanup.jsonl` records the later process checks. Job labels are stable
redacted identifiers; local state files retain process IDs needed for ownership
checks. The worker and exercise scripts are the exact versions used. The
prototype and its evidence are temporary files and may be removed by system
cleanup; they are experimental artifacts, not a Crew interface or contract.
