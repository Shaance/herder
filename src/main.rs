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
use clap::{Args, Parser, Subcommand};
use domain::Executor;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Dispatch, track and verify agent workers",
    after_help = "Example: crew doctor --json\nExit codes: 0 success, 1 operational failure, 2 usage error."
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
        #[arg(long)]
        executor: Option<Executor>,
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    #[command(
        about = "Lint a brief and launch a recorded worker",
        after_help = "Example: crew dispatch --executor codex --brief task.md --repo . --path src/\nExtra arguments go directly to the executor; Vibe launcher policy stays at its default."
    )]
    Dispatch(Dispatch),
    #[command(about = "List runs with fresh executor status", after_help = "Example: crew ls --goal feature-1 --json")]
    Ls {
        #[arg(long)]
        goal: Option<String>,
    },
    #[command(
        about = "Query a run's current executor status",
        after_help = "Example: crew status 20261101-120000-abcd"
    )]
    Status { run: String },
    #[command(about = "Read the current attempt's report", after_help = "Example: crew report 20261101-120000-abcd")]
    Report { run: String },
    #[command(about = "Stop the exact recorded worker", after_help = "Example: crew stop 20261101-120000-abcd")]
    Stop { run: String },
    #[command(
        about = "Resume an exact session with a delta brief",
        after_help = "Example: crew resume 20261101-120000-abcd --brief delta.md"
    )]
    Resume {
        run: String,
        #[arg(long)]
        brief: PathBuf,
    },
    #[command(
        about = "Record scope and command verification",
        after_help = "Example: crew verify 20261101-120000-abcd --path src/ --check 'cargo test --offline'"
    )]
    Verify {
        run: String,
        #[arg(long = "path")]
        paths: Vec<String>,
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
    #[arg(long)]
    executor: Executor,
    #[arg(long)]
    brief: PathBuf,
    #[arg(long)]
    repo: PathBuf,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    goal: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long, help = "Show worker in Rex; Vibe/Codex require watch")]
    pane: bool,
    #[arg(long = "path", help = "Authorized prefix for collect_evidence.sh (repeatable)")]
    paths: Vec<String>,
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
        #[arg(long)]
        title: String,
        #[arg(long)]
        observed: String,
        #[arg(long)]
        expected: String,
        #[arg(long)]
        evidence: Vec<String>,
        #[arg(long, default_value = "crew")]
        area: String,
        #[arg(long)]
        run: Option<String>,
    },
    #[command(about = "Fold issue events into current issues", after_help = "Example: crew issue ls --open")]
    Ls {
        #[arg(long)]
        open: bool,
    },
    #[command(
        about = "Append a resolution to an issue",
        after_help = "Example: crew issue resolve issue-0001 --note 'Fixed'"
    )]
    Resolve {
        id: String,
        #[arg(long)]
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
            render::emit(json, Err(anyhow::anyhow!(e.to_string())), 2);
        }
    };
    render::emit(cli.json, handlers::handle(cli.command), 1);
}
