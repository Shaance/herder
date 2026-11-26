mod review;

use crate::{Companion, StatusArgs};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{ffi::OsStr, path::PathBuf, process::Command};

const INCOMPLETE_LOOKUP: &str =
    "Some Herder records could not be read. Run lookup is incomplete for the configured checkout";
const RUN_LOOKUP_FAILED: &str = "Could not resolve or read this run in the configured checkout";

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    data: Value,
    error: Option<String>,
}

impl Envelope {
    fn diagnostic(&self) -> &str {
        self.error.as_deref().or_else(|| self.data["message"].as_str()).unwrap_or("Herder command failed")
    }
}

#[derive(Deserialize)]
struct Run {
    id: String,
    repo: PathBuf,
    attempt: u32,
    launch: Launch,
}

impl Run {
    fn observed_attempt(&self) -> u32 {
        match self.launch {
            Launch::PendingResume { attempt } => attempt,
            Launch::Other => self.attempt,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Launch {
    PendingResume {
        attempt: u32,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ObservedRun {
    run: Run,
    observation: Value,
    report: Option<Report>,
}

impl ObservedRun {
    fn status(&self) -> Value {
        json!({
            "outcome": "found", "run_id": self.run.id, "attempt": self.run.observed_attempt(),
            "observation": self.observation,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Report {
    Available { content: String },
    Missing { message: String },
}

#[derive(Deserialize)]
struct Listing {
    runs: Vec<ListEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ListEntry {
    Readable { run: NamedRun, observation: Value },
    Unreadable(serde::de::IgnoredAny),
}

#[derive(Deserialize)]
struct NamedRun {
    #[serde(flatten)]
    run: Run,
    name: String,
}

#[derive(Default)]
struct NameLookup {
    matches: Vec<ObservedRun>,
    incomplete: bool,
}

impl Companion {
    fn cli(&self, args: &[&OsStr]) -> Result<Envelope> {
        let output = Command::new(&self.herder)
            .current_dir(&self.repo)
            .arg("--json")
            .args(args)
            .output()
            .context("run the configured Herder executable")?;
        serde_json::from_slice(&output.stdout).with_context(|| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!("Herder returned no valid JSON envelope ({}) {}", output.status, stderr.trim())
        })
    }

    pub fn read_status(&self, args: StatusArgs) -> Result<Value> {
        match args {
            StatusArgs { run_id: Some(id), goal: None, name: None } => Ok(self.read_run("status", &id)?.status()),
            StatusArgs { run_id: None, goal: Some(goal), name: Some(name) } => {
                let lookup = self.lookup(&goal, &name)?;
                Ok(match (lookup.matches.as_slice(), lookup.incomplete) {
                    (runs, true) => {
                        json!({ "outcome": "incomplete", "run_ids": ids(runs), "error": INCOMPLETE_LOOKUP })
                    }
                    ([], false) => json!({ "outcome": "not_found_yet" }),
                    ([run], false) => run.status(),
                    (runs, false) => json!({ "outcome": "ambiguous", "run_ids": ids(runs) }),
                })
            }
            _ => bail!("supply either run_id alone or goal and name together"),
        }
    }

    pub fn read_report(&self, id: &str) -> Result<Value> {
        let entry = self.read_run("report", id)?;
        let report = entry.report.context("Herder report response is missing the report field")?;
        let (outcome, message, content) = match report {
            Report::Available { content } if !content.is_empty() => ("available", None, Some(content)),
            Report::Available { .. } => ("missing", Some("no report yet".into()), None),
            Report::Missing { message } => ("missing", Some(message), None),
        };
        let mut result = json!({
            "outcome": outcome, "message": message, "run_id": entry.run.id,
            "attempt": entry.run.attempt, "observation": entry.observation, "report": content,
        });
        let observed_attempt = entry.run.observed_attempt();
        if observed_attempt != entry.run.attempt {
            result["observed_attempt"] = json!(observed_attempt);
        }
        Ok(result)
    }

    fn read_run(&self, action: &str, id: &str) -> Result<ObservedRun> {
        ensure!(!id.trim().is_empty(), "run_id must be nonempty");
        let reply =
            self.cli(&[action.as_ref(), "--".as_ref(), id.as_ref()]).map_err(|_| anyhow::anyhow!(RUN_LOOKUP_FAILED))?;
        // Herder also accepts names and its failed selector diagnostics search every checkout.
        let run = &reply.data["run"];
        let repo = run["repo"].as_str().map(std::path::Path::new);
        ensure!(run["id"].as_str() == Some(id) && repo == Some(self.repo.as_path()), RUN_LOOKUP_FAILED);
        let missing_report = action == "report" && reply.error.is_none() && reply.data["report"]["message"].is_string();
        if !reply.ok && !missing_report {
            bail!("{}", reply.diagnostic());
        }
        serde_json::from_value(reply.data).context("decode Herder run")
    }

    fn lookup(&self, goal: &str, name: &str) -> Result<NameLookup> {
        validate_label("goal", goal)?;
        validate_label("name", name)?;
        let reply = self.cli(&["ls".as_ref(), format!("--goal={goal}").as_ref()])?;
        ensure!(reply.ok, "{}", reply.diagnostic());
        let listing: Listing = serde_json::from_value(reply.data).context("decode Herder listing")?;
        let mut lookup = NameLookup::default();
        for entry in listing.runs {
            match entry {
                ListEntry::Readable { run, observation } if run.run.repo == self.repo && run.name == name => {
                    lookup.matches.push(ObservedRun { run: run.run, observation, report: None });
                }
                ListEntry::Unreadable(_) => lookup.incomplete = true,
                _ => {}
            }
        }
        Ok(lookup)
    }
}

fn validate_label(field: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty() && !value.contains('\0'), "{field} must be nonempty and cannot contain NUL");
    Ok(())
}

fn ids(runs: &[ObservedRun]) -> Vec<&str> {
    runs.iter().map(|entry| entry.run.id.as_str()).collect()
}
