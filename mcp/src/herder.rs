mod checkout;
mod dispatch;
mod wait;
mod writer;

use crate::{Companion, StatusArgs};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    fmt,
    num::NonZeroU32,
    path::PathBuf,
    process::{Command, ExitStatus},
};

const INCOMPLETE_LOOKUP: &str =
    "Some Herder records could not be read. Run lookup is incomplete for the configured checkout";
const CLI_PROTOCOL_ERROR: HerderUnavailable =
    HerderUnavailable("Herder returned an invalid JSON response. Check that Herder and herder-mcp are compatible");
const CLI_OBSERVED_ATTEMPT_MISSING: HerderUnavailable = HerderUnavailable(
    "The configured Herder CLI is older than herder-mcp. Install matching versions of Herder and herder-mcp",
);
const CLI_START_ERROR: HerderUnavailable =
    HerderUnavailable("Could not start Herder. Check the configured executable and checkout directory");

#[derive(Debug)]
pub struct HerderUnavailable(&'static str);

impl fmt::Display for HerderUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for HerderUnavailable {}

#[derive(Debug)]
pub struct RunNotFound;

impl fmt::Display for RunNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Could not resolve or read this run in the configured checkout")
    }
}

impl std::error::Error for RunNotFound {}

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    data: Value,
    error: Option<String>,
    #[serde(skip)]
    exit_code: i32,
}

impl Envelope {
    fn parse(status: ExitStatus, stdout: &[u8]) -> Result<Self> {
        let exit_code =
            status.code().context(HerderUnavailable("Herder exited unexpectedly. Check the configured executable"))?;
        let mut reply: Self = serde_json::from_slice(stdout).map_err(|_| CLI_PROTOCOL_ERROR)?;
        ensure!(reply.ok == (exit_code == 0), CLI_PROTOCOL_ERROR);
        reply.exit_code = exit_code;
        Ok(reply)
    }

    fn observation(self) -> Result<Self> {
        ensure!(
            self.exit_code != 2,
            HerderUnavailable(
                "Herder rejected the observation command. Check that Herder and herder-mcp are compatible"
            )
        );
        ensure!(
            self.error.is_none() && self.exit_code <= 1,
            HerderUnavailable(
                "Herder could not complete the observation. Check the configured checkout, Git, and state directory with the Herder CLI"
            )
        );
        Ok(self)
    }

    fn exact_observation(self) -> Result<Self> {
        let reply = self.observation()?;
        if reply.data["outcome"] == "not_found" {
            ensure!(!reply.ok, CLI_PROTOCOL_ERROR);
            bail!(RunNotFound);
        }
        Ok(reply)
    }

    fn diagnostic(&self) -> &str {
        self.error.as_deref().or_else(|| self.data["message"].as_str()).unwrap_or("Herder command failed")
    }

    fn decode<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        serde_json::from_value(self.data).map_err(|_| anyhow::anyhow!(CLI_PROTOCOL_ERROR))
    }
}

#[derive(Deserialize)]
struct Run {
    id: String,
    repo: PathBuf,
    attempt: NonZeroU32,
}

#[derive(Deserialize)]
struct ObservedRun {
    run: Run,
    observation: Observation,
    // Optional only so an older CLI gets a version diagnostic after the reply otherwise validates.
    observed_attempt: Option<NonZeroU32>,
    report: Option<Report>,
}

impl ObservedRun {
    fn status(&self) -> Value {
        json!({
            "outcome": "found", "run_id": self.run.id, "attempt": self.observed_attempt,
            "observation": self.observation,
        })
    }
}

#[derive(Deserialize, Serialize)]
struct Observation {
    state: WorkerState,
    evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkerState {
    Starting,
    Running,
    Blocked,
    Finished,
    Failed,
    Stopped,
    Unknown,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Report {
    Available {
        content: String,
    },
    Missing {
        #[serde(rename = "message")]
        _message: String,
    },
}

#[derive(Deserialize)]
struct Listing {
    runs: Vec<ListEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ListEntry {
    Readable { run: NamedRun, observation: Observation, observed_attempt: Option<NonZeroU32> },
    Unreadable { outcome: String },
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
    fn cli_command(&self, args: &[&OsStr]) -> Command {
        let mut command = Command::new(&self.herder);
        command.current_dir(&self.repo).arg("--json").args(args);
        command
    }

    fn cli(&self, args: &[&OsStr]) -> Result<Envelope> {
        let output = self.cli_command(args).output().map_err(|_| CLI_START_ERROR)?;
        Envelope::parse(output.status, &output.stdout)
    }

    fn observation_cli(&self, args: &[&OsStr]) -> Result<Envelope> {
        self.cli(args)?.observation()
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
        let report = entry.report.context(CLI_PROTOCOL_ERROR)?;
        let (outcome, message, content) = match report {
            Report::Available { content } if !content.is_empty() => ("available", None, Some(content)),
            Report::Available { .. } | Report::Missing { .. } => ("missing", Some("no report yet"), None),
        };
        let mut result = json!({
            "outcome": outcome, "message": message, "run_id": entry.run.id,
            "attempt": entry.run.attempt, "observation": entry.observation, "report": content,
        });
        if let Some(observed) = entry.observed_attempt
            && observed != entry.run.attempt
        {
            result["observed_attempt"] = json!(observed);
        }
        Ok(result)
    }

    fn read_run(&self, action: &str, id: &str) -> Result<ObservedRun> {
        let reply = self.observe_exact_run(action, id, &[])?;
        let ok = reply.ok;
        let entry: ObservedRun = reply.decode()?;
        ensure!(entry.run.id == id && entry.run.repo == self.repo, CLI_PROTOCOL_ERROR);
        ensure!(entry.observed_attempt.is_some(), CLI_OBSERVED_ATTEMPT_MISSING);
        let missing_report = matches!(entry.report, Some(Report::Missing { .. }));
        ensure!(ok || (action == "report" && missing_report), CLI_PROTOCOL_ERROR);
        Ok(entry)
    }

    fn observe_exact_run(&self, action: &str, id: &str, flags: &[&OsStr]) -> Result<Envelope> {
        self.cli(&self.exact_args(action, id, flags)?)?.exact_observation()
    }

    fn exact_args<'a>(&'a self, action: &'a str, id: &'a str, flags: &[&'a OsStr]) -> Result<Vec<&'a OsStr>> {
        ensure!(!id.trim().is_empty(), "run_id must be nonempty");
        ensure!(!id.contains('\0'), RunNotFound);
        let mut args = vec![action.as_ref(), "--repo".as_ref(), self.repo.as_os_str()];
        args.extend_from_slice(flags);
        args.extend_from_slice(&["--".as_ref(), id.as_ref()]);
        Ok(args)
    }

    fn lookup(&self, goal: &str, name: &str) -> Result<NameLookup> {
        validate_label("goal", goal)?;
        validate_label("name", name)?;
        let reply = self.observation_cli(&[
            "ls".as_ref(),
            "--repo".as_ref(),
            self.repo.as_os_str(),
            format!("--goal={goal}").as_ref(),
        ])?;
        ensure!(reply.ok, CLI_PROTOCOL_ERROR);
        let listing: Listing = reply.decode()?;
        let mut lookup = NameLookup::default();
        for entry in listing.runs {
            match entry {
                ListEntry::Readable { run, observation, observed_attempt } => {
                    ensure!(run.run.repo == self.repo, CLI_PROTOCOL_ERROR);
                    ensure!(observed_attempt.is_some(), CLI_OBSERVED_ATTEMPT_MISSING);
                    if run.name == name {
                        lookup.matches.push(ObservedRun { run: run.run, observation, observed_attempt, report: None });
                    }
                }
                ListEntry::Unreadable { outcome } if outcome == "incomplete" => lookup.incomplete = true,
                ListEntry::Unreadable { .. } => bail!(CLI_PROTOCOL_ERROR),
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
