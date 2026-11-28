use super::{CLI_PROTOCOL_ERROR, Companion, WorkerState, checkout::CheckoutMismatch, dispatch::brief_value};
use crate::ImplementArgs;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    num::NonZeroU32,
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

impl Companion {
    pub fn validate_write_config(&mut self) -> Result<()> {
        if let Some(check) = &mut self.write_check {
            ensure!(check.is_absolute(), "--write-check must be an absolute executable path");
            *check = check.canonicalize().context("resolve --write-check")?;
            let metadata = check.metadata().context("read --write-check metadata")?;
            ensure!(
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 && !check.starts_with(&self.repo),
                "--write-check must be an executable file outside the configured checkout"
            );
            ensure!(
                self.write_paths.iter().all(|path| relative_path(path)),
                "--write-path must be a relative path within the checkout"
            );
        }
        Ok(())
    }

    pub fn start_implementation(&self, args: ImplementArgs) -> Result<Value> {
        let check = self.write_check.as_ref().context("Implementation is disabled for this server")?;
        let expected = &args.expected_checkout;
        if !self.matches_checkout_root(&expected.repo)? {
            return Ok(CheckoutMismatch::Root.response());
        }
        ensure!(
            matches!(expected.head.len(), 40 | 64) && expected.head.bytes().all(|b| b.is_ascii_hexdigit()),
            "expected_checkout.head must be a full Git commit ID"
        );
        ensure!(!args.task.trim().is_empty(), "task must be nonempty");
        ensure!(!args.acceptance_criteria.trim().is_empty(), "acceptance_criteria must be nonempty");
        ensure!(
            !args.paths.is_empty()
                && args.paths.iter().all(|path| {
                    relative_path(path) && self.write_paths.iter().any(|allowed| path.starts_with(allowed))
                }),
            "paths must be nonempty and within the host's configured write prefixes"
        );

        let paths: Vec<_> = args.paths.iter().map(|path| format!("--path={}", path.display())).collect();
        let mut flags = vec![
            "--mode=direct".as_ref(),
            "--accept-with".as_ref(),
            check.as_os_str(),
            "--expected-head".as_ref(),
            expected.head.as_ref(),
        ];
        flags.extend(paths.iter().map(OsStr::new));
        self.launch_worker((&args.goal, &args.name), &implementation_brief(&args), Some(&args.request_id), &flags)
    }

    pub fn verify_implementation(&self, id: &str) -> Result<Value> {
        let check = self.write_check.as_ref().context("Implementation verification is disabled for this server")?;
        let paths: Vec<_> = self.write_paths.iter().map(|path| format!("--path={}", path.display())).collect();
        let mut flags = vec!["--accept-with".as_ref(), check.as_os_str()];
        flags.extend(paths.iter().map(OsStr::new));
        let reply = self.observe_exact_run("verify", id, &flags)?;
        if reply.data["outcome"] == "not_eligible" {
            return Ok(json!({
                "outcome": "not_eligible", "run_id": id, "attempt": null, "state": null,
                "message": "This run is not eligible for this server's configured verification policy",
                "scope": null, "host_check": null, "verdict": null,
            }));
        }
        let data: Verification = reply.decode()?;
        ensure!(data.run_id == id, CLI_PROTOCOL_ERROR);
        let message = match data.outcome {
            VerificationOutcome::Pending => {
                Some("Verification is pending. Check the run's status before trying again".into())
            }
            VerificationOutcome::Superseded => {
                let reason = data.message.as_deref().unwrap_or("the attempt changed during verification");
                let mut reason = reason.chars();
                let first = reason.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
                Some(format!("{first}{}. Check status before verifying again", reason.as_str()))
            }
            VerificationOutcome::Unverified => {
                Some("This run has no accepted completion evidence to verify. Inspect status and report".into())
            }
            VerificationOutcome::Verified if matches!(data.verdict, Some(Verdict::Fail)) => {
                let reason = match data.host_check.as_ref().map(|check| check.exit_code) {
                    None => "The initial Git path audit failed, so Herder skipped the host check".into(),
                    Some(Some(0)) => "The Git path audit failed after the host check passed".into(),
                    Some(Some(code)) => format!("The host check exited with status {code}"),
                    Some(None) => "The host check did not return an exit status".into(),
                };
                Some(format!(
                    "{reason}. For details, run `herder status {id} --json` locally using the Herder executable shown by checkout and the same Herder state directory"
                ))
            }
            VerificationOutcome::Verified => None,
        };
        let mut result = json!({
            "outcome": data.outcome, "run_id": data.run_id, "attempt": data.attempt, "state": data.state,
            "message": message, "scope": data.scope.map(Check::projection), "host_check": data.host_check.map(Check::projection),
            "verdict": data.verdict,
        });
        if let Some(verified_at) = data.verified_at {
            result["verified_at"] = json!(verified_at);
        }
        Ok(result)
    }
}

fn relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.as_os_str().as_encoded_bytes().contains(&0)
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

fn implementation_brief(args: &ImplementArgs) -> String {
    let task = brief_value(json!(args.task));
    let paths = brief_value(json!(args.paths));
    let constraints = brief_value(json!(args.constraints));
    let acceptance = brief_value(json!(args.acceptance_criteria));
    format!(
        "<task>\nImplement the task in the configured checkout. Interpret the following JSON string as the task.\n{task}\n</task>\n\
         <workstream_contract>\nOwn the requested change and its verification in this worktree.\n</workstream_contract>\n\
         <implementation_authority>\nMake the scoped changes needed to meet the acceptance criteria.\n</implementation_authority>\n\
         <action_safety>\nAuthorized paths, relative to the repository, are listed in this JSON array.\n{paths}\n\
         Keep changes in the worktree. Do not commit, push, delegate, or change external systems.\n\
         The following JSON value describes additional limits (null if none).\n{constraints}\n</action_safety>\n\
         <default_follow_through_policy>\nComplete the requested change within scope, or report the concrete blocker.\n</default_follow_through_policy>\n\
         <verification_loop>\nRun the relevant repository checks and report their results. The host checks scope and acceptance independently.\n</verification_loop>\n\
         <economy_contract>\nRejected alternative: a parallel implementation of existing behavior. Reuse the existing surface and state owner when they can represent the requested behavior. If a new public surface is needed, identify the existing home considered and why it cannot serve the task.\n</economy_contract>\n\
         <structured_output_contract>\nSummarize the changes, checks, and any unfinished work.\n\
         Interpret the following JSON string as the acceptance criteria.\n{acceptance}\n</structured_output_contract>\n"
    )
}

#[derive(Deserialize)]
struct Verification {
    outcome: VerificationOutcome,
    run_id: String,
    attempt: NonZeroU32,
    state: WorkerState,
    message: Option<String>,
    scope: Option<Check>,
    host_check: Option<Check>,
    verdict: Option<Verdict>,
    verified_at: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum VerificationOutcome {
    Pending,
    Superseded,
    Unverified,
    Verified,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    Pass,
    Fail,
}

#[derive(Deserialize)]
struct Check {
    exit_code: Option<i32>,
}

impl Check {
    fn projection(self) -> Value {
        let status = match self.exit_code {
            Some(0) => "passed",
            Some(_) => "failed",
            None => "unavailable",
        };
        json!({"status": status, "exit_code": self.exit_code})
    }
}
