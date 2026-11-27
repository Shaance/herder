mod herder;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::stdio_pipes,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Parser)]
#[command(version, about = "Local stdio MCP companion for Herder reviews and guarded implementation")]
struct Companion {
    /// Path to the Herder executable.
    #[arg(long)]
    herder: PathBuf,
    /// Root of the one checkout available to callers.
    #[arg(long)]
    repo: PathBuf,
    /// Codex worker model for dispatches.
    #[arg(long)]
    model: String,
    /// Codex worker reasoning effort for the configured model.
    #[arg(long, value_parser = ["low", "medium", "high", "xhigh", "max", "ultra"])]
    effort: String,
    /// Host-owned executable for verifying guarded writes.
    #[arg(long, requires = "write_paths")]
    write_check: Option<PathBuf>,
    /// Relative prefix allowed in the post-run Git change audit. Repeat to allow multiple prefixes.
    #[arg(long = "write-path", requires = "write_check")]
    write_paths: Vec<PathBuf>,
}

struct McpServer {
    companion: Companion,
    router: ToolRouter<Self>,
}

#[tool_handler(router = self.router)]
impl ServerHandler for McpServer {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    /// Canonical Herder run ID. Supply only this field when the ID is known.
    run_id: Option<String>,
    /// Exact coordination goal label. Supply with name to recover a lost reply.
    goal: Option<String>,
    /// Exact run name originally supplied to delegate. Supply with goal.
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    /// Canonical Herder run ID returned by delegate or status.
    run_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    /// Exact Herder run ID returned by delegate or status.
    run_id: String,
    /// Maximum seconds to wait, from 1 to 30. Defaults to 15.
    #[schemars(range(min = 1, max = 30))]
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpectedCheckout {
    /// Absolute path to the worktree you independently intend to review.
    repo: PathBuf,
    /// Exact branch name, if the review requires a particular branch.
    branch: Option<String>,
    /// Full commit ID, if the review requires a particular HEAD.
    head: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// Existing coordinator goal label. Use the same exact label to recover a lost reply.
    goal: String,
    /// Fresh run name within the goal and checkout. Keep it for recovering a lost reply.
    name: String,
    /// What the reviewer should inspect and answer.
    task: String,
    /// What the review must contain to be useful, including required evidence.
    acceptance_criteria: String,
    /// Additional limits on the review's scope or actions.
    constraints: Option<String>,
    /// Verify this worktree identity and any supplied branch or HEAD before launch.
    expected_checkout: Option<ExpectedCheckout>,
    /// Stable identity for this dispatch. Replay the identical request with the same key after a lost reply.
    request_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteCheckout {
    /// Absolute path to the intended Git worktree root.
    repo: PathBuf,
    /// Full Git commit ID of the intended starting revision.
    head: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImplementArgs {
    /// Keep this key and replay identical arguments to recover a lost reply without another launch.
    request_id: String,
    goal: String,
    name: String,
    task: String,
    acceptance_criteria: String,
    constraints: Option<String>,
    expected_checkout: WriteCheckout,
    /// Repository-relative prefixes for auditing Git-visible changes after the run, within the host's write_paths.
    paths: Vec<PathBuf>,
}

#[tool_router]
impl McpServer {
    #[tool(
        description = "Inspect the server's canonical configured directory and Git worktree root, branch or checkout state, full HEAD if available, and configured worker model and effort. Compare both paths with your intended worktree root. A registration below the Git root cannot dispatch. Set --repo to the intended worktree root and reconnect. An empty write_paths array means implementation is disabled. Otherwise Codex can write to the workspace, and Herder applies these prefixes in a post-run Git change audit. This tool launches no worker.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn checkout(&self) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.read_checkout()).await
    }

    #[tool(
        description = "Commission one independent read-only review of the configured checkout. Launches a Codex worker and uses its quota; call only when the coordinator has permission for this goal. If the reply is lost or uncertain, use status with the original goal and name, or replay identical arguments if you supplied request_id.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn delegate(&self, Parameters(args): Parameters<DelegateArgs>) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.start_review(args)).await
    }

    #[tool(
        description = "Check a Herder run's current attempt, worker state and evidence. If a delegation reply was lost, supply its original goal and name to find the recorded run before considering another launch.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn status(&self, Parameters(args): Parameters<StatusArgs>) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.read_status(args)).await
    }

    #[tool(
        description = "Read the current attempt's Herder report by run ID. Returns the worker state and any available text, including partial reports. A missing report is a normal result.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn report(&self, Parameters(args): Parameters<ReportArgs>) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.read_report(&args.run_id)).await
    }

    #[tool(
        description = "Wait up to the requested number of seconds for one run in the configured checkout to settle. Returns the observed attempt and state, including on timeout or worker failure. Use report to read available findings.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn wait(&self, Parameters(args): Parameters<WaitArgs>) -> CallToolResult {
        tool_result(self.companion.wait_for_run(args).await)
    }

    #[tool(
        description = "Launch a Codex worker with workspace write access. Herder audits Git-visible changes against paths after execution. Edits stay in the worktree if verification fails. Uses Codex quota. Supply the expected Git root and full HEAD, and keep request_id for safe replay. Use wait to observe worker completion and verify to check the result.",
        annotations(read_only_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn implement(&self, Parameters(args): Parameters<ImplementArgs>) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.start_implementation(args)).await
    }

    #[tool(
        description = "Audit a guarded implementation's Git-visible changes and run the configured host check. Later requests return the first stored verdict for that attempt. Use wait separately to observe worker completion.",
        annotations(read_only_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn verify(&self, Parameters(args): Parameters<ReportArgs>) -> CallToolResult {
        let companion = self.companion.clone();
        run_cli(move || companion.verify_implementation(&args.run_id)).await
    }
}

async fn run_cli(work: impl FnOnce() -> Result<Value> + Send + 'static) -> CallToolResult {
    // An MCP cancellation must not interrupt dispatch or remove its brief while Herder still needs it.
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => tool_result(result),
        Err(error) => CallToolResult::structured_error(json!({ "error": format!("Herder task failed: {error}") })),
    }
}

fn tool_result(result: Result<Value>) -> CallToolResult {
    match result {
        Ok(data) => CallToolResult::structured(data),
        Err(error) if error.is::<herder::RunNotFound>() => {
            CallToolResult::structured_error(json!({ "outcome": "not_found", "error": error.to_string() }))
        }
        Err(error) if error.is::<herder::HerderUnavailable>() => {
            CallToolResult::structured_error(json!({ "outcome": "herder_unavailable", "error": format!("{error:#}") }))
        }
        Err(error) => CallToolResult::structured_error(json!({ "error": format!("{error:#}") })),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut companion = Companion::parse();
    companion.repo = companion.repo.canonicalize().context("resolve --repo")?;
    anyhow::ensure!(companion.repo.is_dir(), "--repo must be a directory");
    companion.herder = companion.herder.canonicalize().context("resolve --herder")?;
    anyhow::ensure!(companion.herder.is_file(), "--herder must be an executable file");
    anyhow::ensure!(!companion.model.trim().is_empty(), "--model must be nonempty");
    companion.validate_write_config()?;
    let mut router = McpServer::tool_router();
    if companion.write_check.is_none() {
        router.remove_route("implement");
        router.remove_route("verify");
    }
    McpServer { companion, router }.serve(stdio_pipes()).await?.waiting().await?;
    Ok(())
}
