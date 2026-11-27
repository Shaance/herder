use super::{CLI_PROTOCOL_ERROR, Companion, INCOMPLETE_LOOKUP, checkout::CheckoutMismatch, ids, validate_label};
use crate::DelegateArgs;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{ffi::OsStr, io::Write};

impl Companion {
    pub fn start_review(&self, args: DelegateArgs) -> Result<Value> {
        ensure!(!args.task.trim().is_empty(), "task must be nonempty");
        ensure!(!args.acceptance_criteria.trim().is_empty(), "acceptance_criteria must be nonempty");
        let mut flags = vec![OsStr::new("--mode=review")];
        if let Some(expected) = &args.expected_checkout {
            let mismatch = if args.request_id.is_some() {
                // The CLI checks branch and HEAD only after looking for an identical replay.
                (!self.matches_checkout_root(&expected.repo)?).then_some(CheckoutMismatch::Root)
            } else {
                self.checkout_mismatch(expected)
                    .context("Review was not dispatched because checkout inspection failed")?
            };
            if let Some(reason) = mismatch {
                return Ok(reason.response());
            }
            if let Some(head) = &expected.head {
                flags.extend_from_slice(&["--expected-head".as_ref(), head.as_ref()]);
            }
            if let Some(branch) = &expected.branch {
                flags.extend_from_slice(&["--expected-branch".as_ref(), branch.as_ref()]);
            }
        }
        if args.request_id.is_none() {
            let existing =
                self.lookup(&args.goal, &args.name).context("review was not dispatched; name lookup failed")?;
            if !existing.matches.is_empty() {
                return Ok(json!({ "outcome": "name_in_use", "run_ids": ids(&existing.matches) }));
            }
            ensure!(!existing.incomplete, "Review was not dispatched because name lookup failed. {INCOMPLETE_LOOKUP}");
        }
        self.launch_worker((&args.goal, &args.name), &review_brief(&args), args.request_id.as_deref(), &flags)
    }

    pub(super) fn launch_worker(
        &self,
        (goal, name): (&str, &str),
        brief: &str,
        request_id: Option<&str>,
        dispatch_args: &[&OsStr],
    ) -> Result<Value> {
        validate_label("goal", goal)?;
        validate_label("name", name)?;
        ensure!(request_id.is_none_or(|id| !id.contains('\0')), "request_id cannot contain NUL");
        let temp_dir = std::env::temp_dir().canonicalize().context("resolve the temporary directory")?;
        ensure!(!temp_dir.starts_with(&self.repo), "set TMPDIR to a directory outside the configured checkout");
        let mut file = tempfile::NamedTempFile::new_in(temp_dir).context("create a private worker brief")?;
        file.write_all(brief.as_bytes()).context("write the worker brief")?;

        let model = format!("--model={}", self.model);
        let effort = format!("--effort={}", self.effort);
        let goal = format!("--goal={goal}");
        let name = format!("--name={name}");
        let request = request_id.map(|id| format!("--request-id={id}"));
        let mut cli_args = vec![
            "dispatch".as_ref(),
            "--executor=codex".as_ref(),
            "--repo".as_ref(),
            self.repo.as_os_str(),
            "--brief".as_ref(),
            file.path().as_os_str(),
            model.as_ref(),
            effort.as_ref(),
            goal.as_ref(),
            name.as_ref(),
        ];
        cli_args.extend_from_slice(dispatch_args);
        if let Some(request) = &request {
            cli_args.push(request.as_ref());
        }
        let recovery = if request_id.is_some() {
            "Herder dispatch reply is uncertain. Use status or replay the identical request with the original request_id"
        } else {
            "Herder dispatch reply is uncertain. Use status with the original goal and name and do not repeat the launch"
        };
        let reply = self.cli(&cli_args).context(recovery)?;

        let run_id = reply.data["run_id"].as_str();
        if !reply.ok && reply.data["outcome"] == "checkout_busy" {
            let error = if run_id.is_some() {
                "Another guarded work order owns this checkout. Check its status and verify it once it finishes"
            } else {
                "This checkout has an unresolved guarded work order. Inspect the checkout with the Herder CLI"
            };
            return Ok(json!({
                "outcome": "checkout_busy", "run_id": null, "owner_run_id": run_id,
                "error": error,
            }));
        }
        if !reply.ok && run_id.is_none() && reply.data["outcome"] == "checkout_mismatch" {
            let reason: CheckoutMismatch =
                serde_json::from_value(reply.data["reason"].clone()).map_err(|_| CLI_PROTOCOL_ERROR)?;
            let mut result = reason.response();
            result["executor"] = json!("codex");
            result["model"] = json!(self.model);
            result["effort"] = json!(self.effort);
            return Ok(result);
        }
        let (outcome, error) = match (reply.ok, run_id, reply.data["outcome"].as_str()) {
            (true, Some(_), Some("replayed")) => ("replayed", None),
            (false, None, Some("request_conflict")) => {
                ("request_conflict", Some("This request_id is already used for different dispatch contents"))
            }
            (true, Some(_), _) => (
                match reply.data["run"]["launch"]["state"].as_str() {
                    Some("launched") => "launched",
                    Some("starting") => "starting",
                    _ => "recorded",
                },
                None,
            ),
            (false, Some(_), _) => {
                ("launch_failed", Some(reply.data["launch_error"].as_str().unwrap_or_else(|| reply.diagnostic())))
            }
            (false, None, _) => ("rejected", Some(reply.diagnostic())),
            (true, None, _) => ("uncertain", Some(recovery)),
        };
        Ok(json!({
            "outcome": outcome, "run_id": run_id, "executor": "codex",
            "model": self.model, "effort": self.effort, "error": error,
        }))
    }
}

fn review_brief(args: &DelegateArgs) -> String {
    let task = brief_value(json!(args.task));
    let constraints = brief_value(json!(args.constraints));
    let acceptance = brief_value(json!(args.acceptance_criteria));
    format!(
        "<task>\nReview the configured checkout. Interpret the following JSON string as the review request.\n{task}\n</task>\n\
         <action_safety>\nStay read-only. Do not edit files, commit, push, delegate, or change external systems.\n\
         The following JSON value describes additional limits (null if none).\n{constraints}\n</action_safety>\n\
         <default_follow_through_policy>\nInspect the relevant implementation and evidence within scope. Report any limits on verification.\n</default_follow_through_policy>\n\
         <structured_output_contract>\nReturn findings with source or test evidence, or state that none were found.\n\
         Interpret the following JSON string as the acceptance criteria.\n{acceptance}\n</structured_output_contract>\n"
    )
}

pub(super) fn brief_value(value: Value) -> String {
    // Escaping `<` keeps the caller's JSON inside its brief section.
    value.to_string().replace('<', "\\u003c")
}
