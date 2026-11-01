# crew: an agent-agnostic CLI for dispatching and tracking agent runs

Status: implementation notes, 2026-11-01. Inputs: pilot ledger, Vibe consult (`runs/vibe-consult.md`), Codex consult (`runs/codex-consult.md`).
Caveat: the Vibe consult read Codex's in-progress transcript, so the two opinions are not fully independent.

## What it is

One command, `crew`, that any coordinator in a Rex pane (Claude Code, Codex, Vibe, or a person) uses to
start a worker on Claude, Vibe or Codex, check on it, read its report, verify its output, and log workflow
problems. Executor quirks (flag order, env, trust, report location) live in code, so every coordinator
gets the same behavior. A short skill, synced to every host by ai-skills-manager, explains the loop.

```
coordinator (any agent, any Rex pane)
   │  crew dispatch / ls / report / verify / issue
   ▼
crew ── claude adapter ── claude --bg   (profile env, report-to-file contract)
     ├─ vibe adapter   ── launch_vibe.sh / status_vibe.sh / resume_vibe.sh   (existing)
     └─ codex adapter  ── launch_classic.sh / status_classic.sh / resume_classic.sh (existing)
   │
   ├─ state: ~/.local/state/ai-skills/crew/runs/<run-id>/run.json  (+ evidence dir)
   ├─ state: ~/.local/state/ai-skills/crew/issues.jsonl
   └─ grouping: optional goal label; approvals remain outside crew
```

## Decisions (where both consultants agreed)

- Build new, thin. Reuse implementer-core's brief format, `check_brief.sh`, `collect_evidence.sh` and the
  Vibe/Codex launch/status/resume scripts. Crew owns attempt transitions and baseline-aware scope;
  executor invocation and terminal evidence stay in the scripts.
- Crew stores goal labels; the coordinator handles user approvals. Vibe runs are auto-approve only.
- skill-feedback keeps its confirmation contract. crew gets its own plain issue log; promotion into
  skill-feedback goes through skill-feedback's normal flow.
- Rust (user decision 2026-11-01): `clap` derive for `--help`, `serde_json` state, `anyhow` errors. Installed with `cargo install --path .`.
- Run identity is a crew-minted run id. The executor's native id (Claude session, Vibe session, Codex
  thread) is written once and used only for exact stop/resume. Resumes are attempts on the same run.
- Run states: `starting`, `running`, `blocked`, `finished`, `failed`, `stopped`, `unknown`. Verification is recorded separately from lifecycle; crew does not record approval.
- Dispatch writes the run record (launch intent) before spawning, so an interrupted dispatch is reconciled
  when identity evidence exists. Claude acceptance is persisted before its launch gate.
- Run writes hold `run.lock`, including recovery. Reads try once and return recorded state if busy;
  mutating waits time out after two seconds. Verification executes checks outside the lock and reloads
  before appending its original attempt's verdict. Dispatch publishes a prepared directory containing
  `run.json`; list errors are isolated per directory.
- Resume records a pending attempt separately from the accepted identity. It advances only when a
  new PID for the same executor session is accepted or recovered. A recorded failure can retry the same
  pending attempt after terminal executor evidence confirms no live worker; wrapper restrictions still apply.
- Every read command re-queries the executor before printing. If crew's record and the executor
  disagree, it prints both and says `unknown`. Queries have a two-second timeout; an existing report
  remains available. Stop records that signalling succeeded before polling briefly for exit.

## MVP commands

| Command | Job |
|---|---|
| `crew doctor [--executor X] [--profile P]` | Check executor and script availability, resolved Claude profile, Rex connectivity and watch availability |
| `crew dispatch --executor claude\|vibe\|codex --brief B --repo R [--model M] [--goal G] [--pane]` | Lint brief, launch, wait for the first artifact, print the run id (JSON with `--json`) |
| `crew ls [--goal G] [--json]` | All runs with reconciled status |
| `crew status RUN` | One run: state, evidence, executor, attempt, run dir, report path, latest verdict |
| `crew report RUN` | The current attempt's report (`final.md`, `final2.md`, ...). Missing report = incomplete |
| `crew stop RUN` / `crew resume RUN --brief DELTA` | Exact-identity stop and resume |
| `crew verify RUN [--path PREFIX]... [--check CMD]...` | Crew checks paths since the dispatch baseline, runs `collect_evidence.sh` and declared checks, and records an attempt-specific verdict |
| `crew issue log\|ls\|resolve` | Log a workflow problem: title, observed, expected, evidence, area (`crew`, `skill:<name>`, `executor:<x>`) |

All commands: `--help` with examples, stable `--json`, documented exit codes, no prompts.

## How the pilot's problems are fixed

| Problem | Fix |
|---|---|
| `claude --bg` refused an untrusted repo | Dispatch retains the executor diagnostic and suggests accepting trust interactively under the recorded profile |
| Variadic tool flags ate the prompt | Claude adapter builds an argv array with the prompt first; a launch gate checks the session is busy, not idle |
| Rex pane showed the wrong profile | Profile resolved once, stored on the run, passed explicitly to every Claude call and dispatch's optional `--pane` view |
| `claude logs` unreadable | Claude workers get a report-to-file contract (`<run dir>/final.md`); `crew report` reads only that |
| Hand-written ledger | Runs are registered at dispatch; `crew ls --goal` is the ledger |
| Release check missed open review findings | Coordinator duty in the skill plus `crew verify --check`: every standing finding needs a fix or a recorded deferral |

## Agent-agnostic by construction

- Only shell commands and files. No Claude-only tools (ListAgents, SendMessage, AskUserQuestion).
- The skill is one `SKILL.md` (source in this repo at `skill/SKILL.md`, installed to `~/.ai-skills/skills/crew/`), synced to Claude, Codex, Cursor and Hermes by ai-skills-manager.
- crew never inherits the caller's identity by accident: the Claude profile is an explicit, recorded input.

## The startup skill (target: under 60 lines)

1. **When to use**: you are coordinating work across agents in Rex.
2. **The loop**: `crew doctor` → write a brief (implementer-core format) → `crew dispatch` → `crew ls` /
   `crew report` → `crew verify` → coordinator reviews and publishes.
3. **Rules**: routing per AGENT_DEFAULTS; a worker report is never proof; publish only after approval.
4. **When something surprises you**: `crew issue log` with observed, expected and evidence, then carry on.
   Details live in `crew <cmd> --help`, not in the skill.

## Not in the MVP

No dashboard, no scheduler or auto-retry, no automatic executor choice (routing stays with
the coordinator per AGENT_DEFAULTS), no transcript summarizer, no cost accounting, no publishing commands
(tag/push/release stay coordinator actions).

## Size budget

See the implementation brief; files split by job (CLI, store, one module per executor), each under ~250 lines.

## Biggest risk

The CLI faithfully records "READY" from a worker while the thing is still broken (Codex), or its records
drift from the executors' (Vibe). Countered by: re-query on every read, `unknown` instead of guessing,
and separate verification tied to an attempt and a dispatch baseline. Checks inspect a changing worktree;
they do not freeze a snapshot or grant approval. Old records without a baseline cannot pass scope verification.
