mod claude;
mod context;
mod dispatch;
mod dispatch_request;
mod doctor;
mod domain;
mod guard;
mod handlers;
mod handoff;
mod output;
mod process;
mod program_resume;
mod progress;
mod render;
mod runs;
mod scripts;
mod store;
mod verify;
mod wait;
mod writer_slot;
use clap::{Args, Parser, Subcommand};
use domain::Executor;
use std::path::PathBuf;

const DEFAULT_WAIT_TIMEOUT: u64 = 3600;

#[derive(Parser)]
#[command(
    version,
    about = "Dispatch, track and verify agent workers",
    after_help = "Example: herder doctor --json\nEnvironment: HERDER_STATE_DIR (default ~/.local/state/herder), HERDER_SKILLS_DIR (optional ai-skills-shaped script directory; default embedded), CLAUDE_CONFIG_DIR (Claude profile).\nExit codes: 0 success, 1 operational failure, 2 usage error."
)]
struct Cli {
    #[arg(long, global = true, help = "Emit a stable {ok,data,error} JSON envelope")]
    json: bool,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    #[command(hide = true)]
    ClaudeStopHook { dir: PathBuf },
    #[command(about = "Show the configured directory, Git checkout, and Herder state directory")]
    Context {
        /// Configured checkout directory
        #[arg(long)]
        repo: PathBuf,
    },
    #[command(
        about = "Check executors, profiles and Rex",
        after_help = "Example: herder doctor --executor claude --profile ~/.claude"
    )]
    Doctor {
        /// Executor to use
        #[arg(long, value_enum)]
        executor: Option<Executor>,
        /// Claude configuration directory (--profile > CLAUDE_CONFIG_DIR > none, Claude default)
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    #[command(
        about = "Lint a brief and launch a recorded worker",
        after_help = "Example: herder dispatch --executor codex --brief /tmp/task.md --repo . --path src/\nKeep brief files outside --repo: untracked briefs inside it fail verify scope.\nExtra arguments go directly to the executor; Vibe runs are auto-approve only.\nAgy requires --model and rejects --effort and extra arguments.\nCodex extras: --image=<file> and -p <profile> after --; extras are replayed on resume.\nLauncher gate exit 3 leaves the run starting; herder returns 0. Use herder wait to await settlement or status to follow progress. Use --check to keep dispatch running through worker completion and verification."
    )]
    Dispatch {
        #[command(flatten)]
        args: DispatchArgs,
        /// Run this coordinator command in --repo after a single dispatch finishes (repeatable)
        #[arg(long = "check")]
        checks: Vec<String>,
        /// Seconds to wait and verify after launch returns (default: 3600 with --check)
        #[arg(long, requires = "checks", value_parser = clap::value_parser!(u32).range(1..))]
        check_timeout: Option<u32>,
    },
    #[command(
        about = "Hand a plain continuation document to a worker and launch it",
        after_help = "Example: herder handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/\nAccepts a plain UTF-8 continuation document instead of a structured brief: its full bytes are snapshotted into the run's input.md behind the standard continuation contract, linted like any brief, and sent to the worker. Missing, empty, whitespace-only or non-UTF-8 documents fail before launch and leave no run record; later edits to the original file cannot change the stored input.\nThis launches a continuation only: it does not transfer hidden conversation, stop another writer, or prove the context was understood. The launch result is launch evidence only. Use --pane for the existing Claude attach functionality when in Rex."
    )]
    Handoff(DispatchArgs),
    #[command(
        about = "List runs with fresh executor status",
        after_help = "Example: herder ls --goal feature-1 --json"
    )]
    Ls {
        /// Match the exact goal string recorded at dispatch
        #[arg(long)]
        goal: Option<String>,
        /// Only observe runs recorded for this directory
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    #[command(
        about = "Wait for any or all runs to settle",
        after_help = "Example: herder wait 20261101-120000-abcd 20261101-120001-ef01 --all --timeout 60\nSettled states: finished, failed, stopped, blocked. Exits 0 if all settled runs at return are finished.\nExits 1 if a settled run failed, stopped or blocked, or if the wait times out.\nPolls every 5 seconds; final output includes every run and its report path. At timeout, stderr (or JSON progress) shows the last available evidence for each unsettled attempt."
    )]
    Wait {
        /// Run IDs or unique dispatch names. With --repo, only exact run IDs are accepted
        #[arg(required_unless_present = "run_dir", num_args = 1..)]
        runs: Vec<String>,
        /// Observe one exact run by its absolute, canonical directory
        #[arg(long, conflicts_with_all = ["runs", "repo"])]
        run_dir: Option<PathBuf>,
        /// Wait for all runs instead of any run
        #[arg(long)]
        all: bool,
        /// Maximum wait in seconds
        #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT)]
        timeout: u64,
        /// Only observe runs recorded for this directory
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    #[command(
        about = "Query a run's current executor status",
        after_help = "Example: herder status 20261101-120000-abcd\nStates: starting, running, blocked, finished, failed, stopped, unknown.\nUnknown means unresolved evidence (e.g. an identity mismatch or failed query); inspect evidence before acting."
    )]
    Status {
        /// Run ID or unique dispatch name. With --repo, only an exact run ID is accepted
        run: String,
        /// Show the last N event-stream lines (1-1000), capped at 500 characters each
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=1000))]
        tail: Option<u16>,
        /// Only observe runs recorded for this directory
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    #[command(
        about = "Read the current attempt's report",
        after_help = "Example: herder report 20261101-120000-abcd\nReads final.md (attempt 1), final2.md, etc. from $HERDER_STATE_DIR/runs/<id>.\nFor Codex, final.md contains the last agent message (codex exec -o).\nThe current attempt is the latest accepted attempt; a pending resume keeps the previous report current."
    )]
    Report {
        /// Run ID or unique dispatch name. With --repo, only an exact run ID is accepted
        run: String,
        /// Only observe runs recorded for this directory
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    #[command(
        about = "Stop the exact recorded worker",
        after_help = "Example: herder stop 20261101-120000-abcd\nClaude uses claude stop. For Vibe, Codex and Agy, Herder confirms the worker identity and sends TERM to the worker and its visible descendants.\nSurviving descendants receive KILL after a short grace period. The run stays running until the worker and recorded descendants exit.\nIf exit is still unconfirmed after two seconds, use status to check or repeat stop to signal survivors. A process already reparented to PID 1 before the snapshot can survive."
    )]
    Stop {
        /// Run id or unique dispatch name
        run: String,
    },
    #[command(
        about = "Resume an exact session with a delta brief",
        after_help = "Example: herder resume 20261101-120000-abcd --brief /tmp/delta.md --request-id sync-42\nVibe/Codex resume the recorded session with a non-empty delta after finished, failed or stopped.\nClaude stops a settled live session and resumes the same identity with saved options under the recorded profile.\nUse a fresh dispatch to continue an Agy run.\nA failed pending resume can retry the same attempt after terminal executor evidence confirms no live worker;\nrunning/unknown blocks retry and executor restrictions still apply. Gate exit 3 leaves the launch pending for observation.\nResume exits 1 if that observation finds a Codex identity conflict, and otherwise exits 0 after gate exit 3.\n--request-id KEY (1-200 bytes, no control characters) records the request before launch: replaying KEY with\nbyte-identical --brief returns the recorded run without another launch; a different brief under KEY is rejected."
    )]
    Resume {
        /// Run id or unique dispatch name
        run: String,
        /// Brief file to send to the worker
        #[arg(long)]
        brief: PathBuf,
        /// Optional request identity making replays with identical brief bytes return the recorded run
        #[arg(long)]
        request_id: Option<String>,
    },
    #[command(
        about = "Record scope and command verification",
        after_help = "Example: herder verify 20261101-120000-abcd --path src/ --check 'cargo test --offline'"
    )]
    Verify {
        /// Exact checkout directory for protected verification
        #[arg(long, requires = "accept_with")]
        repo: Option<PathBuf>,
        /// The host check configured at protected dispatch
        #[arg(long, requires_all = ["repo", "paths"], conflicts_with_all = ["baseline", "checks"])]
        accept_with: Option<PathBuf>,
        /// Replace the recorded scope baseline with this commit revision
        #[arg(long)]
        baseline: Option<String>,
        /// Run id or unique dispatch name
        run: String,
        /// Prefix for auditing Git-visible changes (repeatable). Protected verification requires the host's allowed prefixes.
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Shell command to verify in the repository (repeatable). When omitted, reuse the latest recorded commands from this run. If it has none, use the latest commands from another run with the same goal and repository, when available.
        #[arg(long = "check")]
        checks: Vec<String>,
    },
}
#[derive(Args, Clone)]
struct DispatchArgs {
    /// Brief lint mode (review also selects a read-only sandbox for Codex or plan mode for Agy)
    #[arg(long, default_value = "direct", value_parser = ["direct", "review"])]
    mode: String,
    /// Executor to use
    #[arg(long, value_enum)]
    executor: Executor,
    /// Brief file to send to the worker
    #[arg(long)]
    brief: PathBuf,
    /// Repository directory for the worker
    #[arg(long)]
    repo: PathBuf,
    /// Executor model name (passed through unchecked); repeat with --mode review to launch one run per model
    #[arg(long)]
    model: Vec<String>,
    /// Requested reasoning effort for Codex or Claude (defaults to the executor's configuration)
    #[arg(long, value_name = "LEVEL", value_parser = ["low", "medium", "high", "xhigh", "max", "ultra"])]
    effort: Option<String>,
    /// Optional goal string; ls --goal matches it exactly
    #[arg(long)]
    goal: Option<String>,
    /// Worker display name
    #[arg(long)]
    name: Option<String>,
    /// Claude configuration directory (--profile > CLAUDE_CONFIG_DIR > none, Claude default)
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long, help = "Show worker in Rex. Vibe, Codex and Agy panes use herder wait")]
    pane: bool,
    #[arg(long = "path", help = "Allowed prefix for the post-run Git change audit (repeatable)")]
    paths: Vec<String>,
    /// Reserve one initial dispatch per checkout and request ID
    #[arg(long)]
    request_id: Option<String>,
    /// Absolute host check for a protected Codex launch with workspace write access
    #[arg(long, requires_all = ["request_id", "expected_head"])]
    accept_with: Option<PathBuf>,
    /// Require the checkout to start at this exact commit
    #[arg(long)]
    expected_head: Option<String>,
    /// Require the checkout to be on this branch
    #[arg(long)]
    expected_branch: Option<String>,
    /// Arguments passed to the executor after -- (repeatable)
    #[arg(last = true)]
    extra: Vec<String>,
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let json = args.iter().take_while(|a| *a != "--").any(|a| a == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            if e.exit_code() == 0 {
                let _ = e.print();
                std::process::exit(0);
            }
            render::emit(json, Err(anyhow::anyhow!(e.to_string())), 2);
        }
    };
    render::emit(cli.json, handlers::handle(cli.command, cli.json), 1);
}
