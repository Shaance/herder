mod claude;
mod doctor;
mod domain;
mod handlers;
mod issues;
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
    after_help = "Example: crew doctor --json\nEnvironment: CREW_STATE_DIR (default ~/.local/state/ai-skills/crew), CREW_SKILLS_DIR (default ~/.ai-skills/skills), CLAUDE_CONFIG_DIR (Claude profile).\nExit codes: 0 success, 1 operational failure, 2 usage error."
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
        after_help = "Example: crew dispatch --executor codex --brief /tmp/task.md --repo . --path src/\nKeep brief files outside --repo: untracked briefs inside it fail verify scope.\nExtra arguments go directly to the executor; Vibe runs are auto-approve only.\nLauncher gate exit 3 leaves the run starting; crew returns 0. Use status to follow progress."
    )]
    Dispatch(Dispatch),
    #[command(about = "List runs with fresh executor status", after_help = "Example: crew ls --goal feature-1 --json")]
    Ls {
        /// Match the exact goal string recorded at dispatch
        #[arg(long)]
        goal: Option<String>,
    },
    #[command(
        about = "Query a run's current executor status",
        after_help = "Example: crew status 20261101-120000-abcd\nStates: starting, running, blocked, finished, failed, stopped, unknown.\nUnknown means unresolved evidence (e.g. an identity mismatch or failed query); inspect evidence before acting."
    )]
    Status {
        /// run id, e.g. 20261101-120000-abcd
        run: String,
    },
    #[command(
        about = "Read the current attempt's report",
        after_help = "Example: crew report 20261101-120000-abcd\nReads final.md (attempt 1), final2.md, etc. from the run directory.\nThe current attempt is the latest accepted attempt; a pending resume keeps the previous report current."
    )]
    Report {
        /// run id, e.g. 20261101-120000-abcd
        run: String,
    },
    #[command(
        about = "Stop the exact recorded worker",
        after_help = "Example: crew stop 20261101-120000-abcd\nRequires running or blocked state and a matching recorded identity. Claude uses claude stop;\nVibe/Codex receive TERM only after the PID command matches this run. Waits briefly for exit;\nif exit is not confirmed, reports stop sent with exit 0. Use status to confirm."
    )]
    Stop {
        /// run id, e.g. 20261101-120000-abcd
        run: String,
    },
    #[command(
        about = "Resume an exact session with a delta brief",
        after_help = "Example: crew resume 20261101-120000-abcd --brief /tmp/delta.md\nVibe/Codex resume the recorded session with a non-empty delta after finished, failed or stopped.\nClaude requires manual claude attach <id> under the recorded profile.\nA failed pending resume can retry the same attempt after terminal executor evidence confirms no live worker;\nrunning/unknown blocks retry and executor restrictions still apply. Gate exit 3 leaves pending_resume and returns 0."
    )]
    Resume {
        /// run id, e.g. 20261101-120000-abcd
        run: String,
        /// Brief file to send to the worker
        #[arg(long)]
        brief: PathBuf,
    },
    #[command(
        about = "Record scope and command verification",
        after_help = "Example: crew verify 20261101-120000-abcd --path src/ --check 'cargo test --offline'"
    )]
    Verify {
        /// run id, e.g. 20261101-120000-abcd
        run: String,
        /// Authorized path prefix (repeatable); required if none were recorded at dispatch
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Shell command to verify in the repository (repeatable)
        #[arg(long = "check")]
        checks: Vec<String>,
    },
    #[command(about = "Log and resolve workflow problems", after_help = "Example: crew issue ls --open --json")]
    Issue {
        #[command(subcommand)]
        command: IssueAction,
    },
}
#[derive(Args)]
struct Dispatch {
    /// Executor to use
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["claude", "vibe", "codex"]).map(|s| s.parse::<Executor>().expect("listed executor is valid")))]
    executor: Executor,
    /// Brief file to send to the worker
    #[arg(long)]
    brief: PathBuf,
    /// Repository directory for the worker
    #[arg(long)]
    repo: PathBuf,
    /// Executor model name (passed through unchecked)
    #[arg(long)]
    model: Option<String>,
    /// Optional goal string; ls --goal matches it exactly
    #[arg(long)]
    goal: Option<String>,
    /// Worker display name
    #[arg(long)]
    name: Option<String>,
    /// Claude configuration directory (--profile > CLAUDE_CONFIG_DIR > none, Claude default)
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long, help = "Show worker in Rex; Vibe/Codex require watch with -x support")]
    pane: bool,
    #[arg(long = "path", help = "Authorized path prefix for verification evidence (repeatable)")]
    paths: Vec<String>,
    /// Arguments passed to the executor after -- (repeatable)
    #[arg(last = true)]
    extra: Vec<String>,
}
#[derive(Subcommand)]
enum IssueAction {
    #[command(
        about = "Append an observed workflow problem",
        after_help = "Example: crew issue log --title 'Missing report' --observed 'No file' --expected 'final.md'"
    )]
    Log {
        /// Short issue title
        #[arg(long, allow_hyphen_values = true)]
        title: String,
        /// What actually happened
        #[arg(long, allow_hyphen_values = true)]
        observed: String,
        /// What should have happened
        #[arg(long, allow_hyphen_values = true)]
        expected: String,
        /// Supporting evidence as free text, not a file to read (repeatable)
        #[arg(long, allow_hyphen_values = true)]
        evidence: Vec<String>,
        /// Affected area, e.g. crew or executor:claude
        #[arg(long, default_value = "crew")]
        area: String,
        /// run id, e.g. 20261101-120000-abcd
        #[arg(long)]
        run: Option<String>,
    },
    #[command(about = "Fold issue events into current issues", after_help = "Example: crew issue ls --open")]
    Ls {
        /// Show only unresolved issues
        #[arg(long)]
        open: bool,
    },
    #[command(
        about = "Append a resolution to an issue",
        after_help = "Example: crew issue resolve issue-0001 --note 'Fixed'"
    )]
    Resolve {
        /// issue id, e.g. issue-0001
        id: String,
        /// Resolution and supporting evidence
        #[arg(long, allow_hyphen_values = true)]
        note: String,
    },
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
            render::emit(json, Err(anyhow::anyhow!(e.to_string())), 2, render::pretty);
        }
    };
    let human = render::human(&cli.command);
    render::emit(cli.json, handlers::handle(cli.command), 1, human);
}
