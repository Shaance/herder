# ACP adoption assessment, 2026-11-08

Vibe 3.1.0 can support Crew's detached-worker workflow through ACP, but replacing
the current adapter has not demonstrated a maintenance reduction. Retain the
existing production adapter. The experiments validate a possible architecture;
they do not qualify the disposable Python worker for production.

Three Terra agents at maximum effort tested separate configuration, recovery,
and lifecycle boundaries in private scratch repositories. The coordinator
reviewed source and evidence. No production code changed during this assessment;
the earlier tempfile cleanup remains separate. The previous experiments are in
[acp-experiment.md](acp-experiment.md).

## Acceptance matrix

| Area | Observed result | Adoption implication |
| --- | --- | --- |
| Existing auth and cwd | Real ACP prompts and a CLI comparison worked in isolated scratch configuration. Untrusted project configuration was identified in ACP metadata. | Existing credentials can be reused; trust behavior must remain explicit. |
| Model and thinking | ACP setters changed isolated user TOML. A nonexistent model was rejected; an unadvertised thinking value was accepted and persisted at the setter boundary. Installed source explicitly permits custom thinking values. | Do not treat these setters as job-local overrides against the user's config. The custom value was not sent to a provider, so no validation defect is established. |
| Process-local overrides | Startup-only checks changed effective model-alias/thinking settings while inspected user-config fields retained their saved values. The same overrides were also used in real permission tests, which did not record post-prompt config snapshots. | Possible workaround, with coupling to Vibe's config schema. A sparse model entry did not suffice. Full-byte scratch-config immutability and switching provider models were not proven. |
| Tool permissions | In ask mode, the edit request was rejected, the tool reported failure, and its target file was absent afterward. Auto-approve reported a completed edit without a callback and left the expected file contents. | Representative denial and approval passed. No explicit pre-call file-absence assertion was recorded; other tools and policies remain untested. |
| Turn limits and usage | ACP stopped at `max_turn_requests`; the CLI exported `turn_limit`. Both exposed tokens and cost. Invalid-model startup failed on both surfaces. | Outcome/error mapping is required. Token, price, and time limit enforcement were not demonstrated. |
| Conversation replay and continuation | A new process loaded the exact saved session. A corrected role-filtered check independently found the prior user marker and assistant reply. After cancellation and both crash cases, a new no-tool recall turn returned `end_turn` with the expected word. | Explicit continuation works in these cases; it does not establish that replaying an interrupted side effect is safe. |
| Live ownership | A second process loading an owned session received -31002, classified as session already open. Loading succeeded after owner release. | Vibe already supplies cross-process session exclusion for this case; keep Crew's job and attempt identity checks. |
| Mid-turn crashes | Killing the verified worker or its ACP child after provider activity produced interrupted/failed status. A fresh process loaded retained history and the interrupted user prompt. | No false success observed. Remote completion or tool effects remain uncertain; no automatic retry was validated. |
| Initialization and damaged evidence | Incomplete initialize returned -32602 and a failed worker with no prompt. Missing/corrupt state caused nonzero status exits; a stale socket produced unknown/interrupted. | Failure visibility passed at the tested boundary; user-facing diagnostics need production design. |
| Cancellation after streaming | A real assistant message chunk preceded a separate stop; ACP returned `cancelled`, and both local processes exited. | The earlier before-output cancellation gap is closed. Remote provider-side cessation is not proven. |
| Repeated/late stop and readers | Two near-simultaneous stops yielded one accepted cancellation. Late stop returned no response. A disconnected or briefly slow status reader did not stop work. | Basic local lifecycle passed. Saturated-output backpressure and arbitrary scheduling are not proven. |
| Concurrent-job targeting | With two real turns active under controlled process scheduling, cancelling the streaming job left the other running without a cancel event; the other then completed with its expected reply. | Isolation passed for this schedule. This is not a throughput or exhaustive race benchmark. |
| Local IPC boundary | Parent job directories were 0700; socket/state modes were 0755/0644. Status rejected a wrong-job response, but stop sent its request before checking job identity. | Parent permissions restrict traversal; raw child modes alone do not prove exposure. Prototype stop routing needs correction before production. |

The wrong-endpoint case used a synthetic local server to exercise the client
boundary. It does not prove an attacker can redirect a protected job path. An
unreachable socket can mean startup, a stalled worker, or death; the prototype's
unknown/interrupted label must not be interpreted as proof of death.

## Maintenance decision

The current Vibe launch/resume/status scripts total 192 lines. Crew's shared
136-line script adapter also serves Codex, so migrating only Vibe would not
remove it wholesale. The first detached Python worker was 170 lines before
recovery, permission/configuration parity, and production error handling. Test
driver lines are experimental evidence machinery, not a production size estimate.

The [Rust ACP SDK](https://agentclientprotocol.com/libraries/rust) can own
protocol types, request/notification transport, and agent subprocess handling.
Its [AcpAgent implementation](https://github.com/agentclientprotocol/rust-sdk/blob/main/src/agent-client-protocol/src/acp_agent.rs)
ties child lifetime to the connection owner. Crew would still own a persistent
per-job process, command routing, attempt state, job identity, permission policy,
configuration isolation, outcome mapping, and recovery decisions. That SDK
integration has been inspected in documentation/source, not executed in Rust.

For a maintenance-only change, the demonstrated benefit is insufficient to
justify the new ownership and IPC surface. A future migration needs a concrete
benefit, such as shared support across multiple ACP executors, and an explicit
supported contract. Before shipping it, cover remaining config/argument parity,
token/price/time limits, targeted cancellation and failure diagnostics, resume
across interruptions, and version compatibility. Do not equate `end_turn` with
Crew's repository verification passing.

## Evidence and scope

Each track used its own scratch Git repository and owned only its test processes.
No real-user default config writes were authorized. The coordinator independently
checked the real user's config hash against the track's recorded pre-test hash;
they matched. Shared logs omit
credentials, thought text, and native session identifiers; continuation identity
is retained only in private local state.

| Track | Findings and artifacts |
| --- | --- |
| Config/permissions | `/private/tmp/crew-acp-config.w8oT00/findings.md`, `observations.jsonl`, `probe.py`, `cli_baseline.py` |
| Recovery/identity | `/private/tmp/crew-acp-recovery.ki9aMB/findings.md`, `observations.jsonl`, `worker.py`, `executed/` |
| Lifecycle/IPC | `/private/tmp/crew-acp-lifecycle-dww4gem5/findings.md`, `observations.jsonl`, per-job events and driver scripts |

These artifacts are local and temporary and can be removed by system cleanup.
The tracks submitted 14 provider prompts in total: configuration 3, recovery 5,
and lifecycle 6. A fourth Terra agent performed a read-only evidence review.
The review corrected wording about custom thinking values, the scope of config
immutability checks, and file-creation evidence; none required a production edit.
The observations describe this macOS host and installed Vibe/ACP 3.1.0. They do
not establish other operating systems, providers, future versions, host-crash
durability, exactly-once remote effects, or a security audit of a production
implementation. Production tests were not repeated for this documentation-only
assessment.
