mod herder;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    model::CallToolResult,
    schemars::{self, JsonSchema},
    tool, tool_router,
    transport::stdio_pipes,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Parser)]
#[command(version, about = "Local stdio MCP companion for Herder reviews")]
struct Companion {
    /// Path to the Herder executable.
    #[arg(long)]
    herder: PathBuf,
    /// Root of the one checkout available to callers.
    #[arg(long)]
    repo: PathBuf,
    /// Codex worker model for review dispatches.
    #[arg(long)]
    model: String,
    /// Codex worker reasoning effort for the configured model.
    #[arg(long, value_parser = ["low", "medium", "high", "xhigh", "max", "ultra"])]
    effort: String,
}

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
}

#[tool_router(server_handler)]
impl Companion {
    #[tool(
        description = "Commission one independent read-only review of the configured checkout. Launches a Codex worker and uses its quota; call only when the coordinator has permission for this goal. Returns a Herder run ID when recorded. If the reply is lost or uncertain, use status with the same goal and name; do not repeat the launch.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn delegate(&self, Parameters(args): Parameters<DelegateArgs>) -> CallToolResult {
        let companion = self.clone();
        run_cli(move || companion.start_review(args)).await
    }

    #[tool(
        description = "Check a Herder run's current attempt, worker state and evidence. If a delegation reply was lost, supply its original goal and name to find the recorded run before considering another launch.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn status(&self, Parameters(args): Parameters<StatusArgs>) -> CallToolResult {
        let companion = self.clone();
        run_cli(move || companion.read_status(args)).await
    }

    #[tool(
        description = "Read the current attempt's Herder report by run ID. Returns the worker state and any available text, including partial reports. A missing report is a normal result.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn report(&self, Parameters(args): Parameters<ReportArgs>) -> CallToolResult {
        let companion = self.clone();
        run_cli(move || companion.read_report(&args.run_id)).await
    }
}

async fn run_cli(work: impl FnOnce() -> Result<Value> + Send + 'static) -> CallToolResult {
    // An MCP cancellation must not interrupt dispatch or remove its brief while Herder still needs it.
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(data)) => CallToolResult::structured(data),
        Ok(Err(error)) => CallToolResult::structured_error(json!({ "error": format!("{error:#}") })),
        Err(error) => CallToolResult::structured_error(json!({ "error": format!("Herder task failed: {error}") })),
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
    companion.serve(stdio_pipes()).await?.waiting().await?;
    Ok(())
}
