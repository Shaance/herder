use super::{Companion, INCOMPLETE_LOOKUP, ids};
use crate::DelegateArgs;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::io::Write;

impl Companion {
    pub fn start_review(&self, args: DelegateArgs) -> Result<Value> {
        ensure!(!args.task.trim().is_empty(), "task must be nonempty");
        ensure!(!args.acceptance_criteria.trim().is_empty(), "acceptance_criteria must be nonempty");
        let existing = self.lookup(&args.goal, &args.name).context("review was not dispatched; name lookup failed")?;
        if !existing.matches.is_empty() {
            return Ok(json!({ "outcome": "name_in_use", "run_ids": ids(&existing.matches) }));
        }
        ensure!(!existing.incomplete, "Review was not dispatched because name lookup failed. {INCOMPLETE_LOOKUP}");

        let temp_dir = std::env::temp_dir().canonicalize().context("resolve the temporary directory")?;
        ensure!(!temp_dir.starts_with(&self.repo), "set TMPDIR to a directory outside the configured checkout");
        let mut brief = tempfile::NamedTempFile::new_in(temp_dir).context("create a private review brief")?;
        brief.write_all(review_brief(&args).as_bytes()).context("write the review brief")?;

        let reply = self.cli(&[
            "dispatch".as_ref(),
            "--mode=review".as_ref(),
            "--executor=codex".as_ref(),
            "--repo".as_ref(),
            self.repo.as_os_str(),
            "--brief".as_ref(),
            brief.path().as_os_str(),
            format!("--model={}", self.model).as_ref(),
            format!("--effort={}", self.effort).as_ref(),
            format!("--goal={}", args.goal).as_ref(),
            format!("--name={}", args.name).as_ref(),
        ]).context("Herder dispatch reply is uncertain. Use status with the original goal and name and do not repeat the launch")?;

        let run_id = reply.data["run_id"].as_str();
        let (outcome, error) = match (reply.ok, run_id) {
            (true, Some(_)) => (
                match reply.data["run"]["launch"]["state"].as_str() {
                    Some("launched") => "launched",
                    Some("starting") => "starting",
                    _ => "recorded",
                },
                None,
            ),
            (false, Some(_)) => {
                ("launch_failed", Some(reply.data["launch_error"].as_str().unwrap_or_else(|| reply.diagnostic())))
            }
            (false, None) => ("rejected", Some(reply.diagnostic())),
            (true, None) => (
                "uncertain",
                Some(
                    "Herder returned no run ID. Use status with the original goal and name and do not repeat the launch",
                ),
            ),
        };
        Ok(json!({
            "outcome": outcome, "run_id": run_id, "executor": "codex",
            "model": self.model, "effort": self.effort, "error": error,
        }))
    }
}

fn review_brief(args: &DelegateArgs) -> String {
    // JSON quoting preserves the caller's exact text. Escaping `<` keeps it inside its section.
    let task = json!(args.task).to_string().replace('<', "\\u003c");
    let constraints = json!(args.constraints).to_string().replace('<', "\\u003c");
    let acceptance = json!(args.acceptance_criteria).to_string().replace('<', "\\u003c");
    format!(
        "<task>\nReview the configured checkout. Interpret the following JSON string as the review request.\n{task}\n</task>\n\
         <action_safety>\nStay read-only. Do not edit files, commit, push, delegate, or change external systems.\n\
         The following JSON value describes additional limits (null if none).\n{constraints}\n</action_safety>\n\
         <default_follow_through_policy>\nInspect the relevant implementation and evidence within scope. Report any limits on verification.\n</default_follow_through_policy>\n\
         <structured_output_contract>\nReturn findings with source or test evidence, or state that none were found.\n\
         Interpret the following JSON string as the acceptance criteria.\n{acceptance}\n</structured_output_contract>\n"
    )
}
