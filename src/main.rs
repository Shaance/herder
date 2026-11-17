mod claude;
mod doctor;
mod domain;
mod handlers;
mod handoff;
mod process;
mod render;
mod runs;
mod scripts;
mod store;
mod verify;
use clap::{Args, Parser, Subcommand, builder::TypedValueParser};
use domain::Executor;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Dispatch, track and verify agent workers",
    after_help = "Example: crew doctor --json\nEnvironment: CREW_STATE_DIR (default ~/.local/state/crew), CREW_SKILLS_DIR (optional ai-skills-shaped script directory; default embedded), CLAUDE_CONFIG_DIR (Claude profile).\nExit codes: 0 success, 1 operational failure, 2 usage error."
)]
struct Cli {
    #[arg(long, global = true, help = "Emit a stable {ok,data,error} JSON envelope")]
    json: bool,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    #[command(
        about = "Check executors, profiles and Rex",
        after_help = "Example: crew doctor --executor claude --profile ~/.claude"
    )]
    Doctor {
        /// Executor to use
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["claude", "vibe", "codex"]).map(|s| s.parse::<Executor>().expect("listed executor is valid")))]
        executor: Option<Executor>,
        /// Claude configuration directory (--profile > CLAUDE_CONFIG_DIR > none, Claude default)
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    #[command(
        about = "Lint a brief and launch a recorded worker",
        after_help = "Example: crew dispatch --executor codex --brief /tmp/task.md --repo . --path src/\nKeep brief files outside --repo: untracked briefs inside it fail verify scope.\nExtra arguments go directly to the executor; Vibe runs are auto-approve only.\nLauncher gate exit 3 leaves the run starting; crew returns 0. Use crew wait to await settlement or status to follow progress."
    )]
    Dispatch(Dispatch),
    #[command(
        about = "Hand a plain continuation document to a worker and launch it",
        after_help = "Example: crew handoff --executor claude --brief /tmp/HANDOFF.md --repo . --path src/\nAccepts a plain UTF-8 continuation document instead of a structured brief: its full bytes are snapshotted into the run's input.md behind the standard continuation contract, linted like any brief, and sent to the worker. Missing, empty, whitespace-only or non-UTF-8 documents fail before launch and leave no run record; later edits to the original file cannot change the stored input.\nThis launches a continuation only: it does not transfer hidden conversation, stop another writer, or prove the context was understood. The launch result is launch evidence only. Use --pane for the existing Claude attach functionality when in Rex."
    )]
    Handoff(Dispatch),
    #[command(about = "List runs with fresh executor status", after_help = "Example: crew ls --goal feature-1 --json")]
    Ls {
        /// Match the exact goal string recorded at dispatch
        #[arg(long)]
        goal: Option<String>,
    },
    #[command(
        about = "Wait for any or all runs to settle",
        after_help = "Example: crew wait 20261101-120000-abcd 20261101-120001-ef01 --all --timeout 60\nSettled: finished, failed, stopped, blocked (exit 0). Unknown or timeout exits 1.\nPolls every 5 seconds; final output includes every run and its report path."
    )]
    Wait {
        /// Run ids or unique dispatch names to observe
        #[arg(required = true, num_args = 1..)]
        runs: Vec<String>,
        /// Wait for all runs instead of any run
        #[arg(long)]
        all: bool,
        /// Maximum wait in seconds
        #[arg(long, default_value = "3600")]
        timeout: u64,
    },
    #[command(
        about = "Query a run's current executor status",
        after_help = "Example: crew status 20261101-120000-abcd\nStates: starting, running, blocked, finished, failed, stopped, unknown.\nUnknown means unresolved evidence (e.g. an identity mismatch or failed query); inspect evidence before acting."
    )]
    Status {
        /// Run id or unique dispatch name
        run: String,
        /// Show the last N event-stream lines (1-1000), capped at 500 characters each
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=1000))]
        tail: Option<u16>,
    },
    #[command(
        about = "Read the current attempt's report",
        after_help = "Example: crew report 20261101-120000-abcd\nReads final.md (attempt 1), final2.md, etc. from the run directory.\nThe current attempt is the latest accepted attempt; a pending resume keeps the previous report current."
    )]
    Report {
        /// Run id or unique dispatch name
        run: String,
    },
    #[command(
        about = "Stop the exact recorded worker",
        after_help = "Example: crew stop 20261101-120000-abcd\nRequires running or blocked state and a matching recorded identity. Claude uses claude stop;\nVibe/Codex receive TERM only after the PID command matches this run. Waits briefly for exit;\nif exit is not confirmed, reports stop sent with exit 0. Use status to confirm."
    )]
    Stop {
        /// Run id or unique dispatch name
        run: String,
    },
    #[command(
        about = "Resume an exact session with a delta brief",
        after_help = "Example: crew resume 20261101-120000-abcd --brief /tmp/delta.md --request-id sync-42\nVibe/Codex resume the recorded session with a non-empty delta after finished, failed or stopped.\nClaude stops a settled live session and resumes the same identity with saved options under the recorded profile.\nA failed pending resume can retry the same attempt after terminal executor evidence confirms no live worker;\nrunning/unknown blocks retry and executor restrictions still apply. Gate exit 3 leaves pending_resume and returns 0.\n--request-id KEY (1-200 bytes, no control characters) records the request before launch: replaying KEY with\nbyte-identical --brief returns the recorded run without another launch; a different brief under KEY is rejected."
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
        after_help = "Example: crew verify 20261101-120000-abcd --path src/ --check 'cargo test --offline'"
    )]
    Verify {
        /// Replace the recorded scope baseline with this commit revision
        #[arg(long)]
        baseline: Option<String>,
        /// Run id or unique dispatch name
        run: String,
        /// Authorized path prefix (repeatable); required if none were recorded at dispatch
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Shell command to verify in the repository (repeatable)
        #[arg(long = "check")]
        checks: Vec<String>,
    },
}
#[derive(Args, Clone)]
struct Dispatch {
    /// Brief lint mode
    #[arg(long, default_value = "direct", value_parser = ["direct", "review"])]
    mode: String,
    /// Executor to use
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["claude", "vibe", "codex"]).map(|s| s.parse::<Executor>().expect("listed executor is valid")))]
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
    /// Optional goal string; ls --goal matches it exactly
    #[arg(long)]
    goal: Option<String>,
    /// Worker display name
    #[arg(long)]
    name: Option<String>,
    /// Claude configuration directory (--profile > CLAUDE_CONFIG_DIR > none, Claude default)
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long, help = "Show worker in Rex; Vibe/Codex panes use crew wait")]
    pane: bool,
    #[arg(long = "path", help = "Authorized path prefix for verification evidence (repeatable)")]
    paths: Vec<String>,
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
