use crate::{
    domain::Executor,
    process::{self, Script},
    scripts::ScriptExecutor,
};
use anyhow::{Result, ensure};
use clap::ValueEnum;
use serde::Serialize;
use std::{fs, path::PathBuf, process::Command};

#[derive(Serialize)]
pub struct Doctor {
    pub executors: Vec<ExecutorChecks>,
    pub claude_profile: Option<PathBuf>,
    pub profile_resolution: &'static str,
    pub rex: CheckStatus,
}

#[derive(Serialize)]
pub struct ExecutorChecks {
    pub executor: Executor,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Serialize)]
pub struct DoctorCheck {
    pub item: PathBuf,
    #[serde(flatten)]
    pub status: CheckStatus,
}

pub enum CheckStatus {
    Present,
    Missing(String),
}

impl CheckStatus {
    fn from_probe(present: bool, fix: impl FnOnce() -> String) -> Self {
        if present { Self::Present } else { Self::Missing(fix()) }
    }
}

impl Serialize for CheckStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let fix = match self {
            Self::Present => None,
            Self::Missing(fix) => Some(fix),
        };
        serde_json::json!({"ok": fix.is_none(), "fix": fix}).serialize(serializer)
    }
}

pub fn run(executor: Option<Executor>, profile: Option<PathBuf>) -> Result<(Doctor, i32)> {
    let profile = process::profile(profile)?;
    ensure!(
        profile.as_ref().is_none_or(|path| path.to_str().is_some()),
        "Claude profile path contains invalid UTF-8 characters"
    );
    let mut executors = vec![];
    let override_set = std::env::var_os("HERDER_SKILLS_DIR").is_some();
    for &e in executor.as_ref().map_or(Executor::value_variants(), std::slice::from_ref) {
        let name = e.as_str();
        let adapter = ScriptExecutor::for_executor(e);
        let mut checks = vec![];
        let binaries = match (e == Executor::Claude, override_set) {
            (true, true) => vec![name],
            (true, false) => vec![name, "bash"],
            (false, _) => vec![name, "bash", "jq"],
        };
        for exe in binaries {
            let status = CheckStatus::from_probe(process::on_path(exe), || {
                format!("install {exe} and put its executable on PATH")
            });
            checks.push(DoctorCheck { item: exe.into(), status });
        }
        if e == Executor::Agy {
            let relative = ".gemini/antigravity-cli/settings.json";
            let settings =
                std::env::var_os("HOME").filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join(relative));
            let api_key_mode = settings
                .as_ref()
                .and_then(|path| fs::read(path).ok())
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|config| config["modelProvider"] == "gemini");
            let item = settings.unwrap_or_else(|| PathBuf::from("~").join(relative));
            let status = CheckStatus::from_probe(api_key_mode, || {
                format!("set \"modelProvider\": \"gemini\" in {} to use API-key mode", item.display())
            });
            checks.push(DoctorCheck { item, status });
        }
        if override_set {
            let mut scripts = vec![Script::CheckBrief, Script::CollectEvidence];
            if let Some(adapter) = adapter {
                scripts.extend([adapter.launch_script, adapter.status_script]);
                scripts.extend(adapter.resume_script);
            }
            for script in scripts {
                let path = PathBuf::from(process::script(script).get_program());
                let status = CheckStatus::from_probe(process::executable(&path), || {
                    format!("restore executable {} or set HERDER_SKILLS_DIR to the skills directory", path.display())
                });
                checks.push(DoctorCheck { item: path, status });
            }
        } else {
            checks.push(DoctorCheck { item: "scripts (embedded)".into(), status: CheckStatus::Present });
        }
        executors.push(ExecutorChecks { executor: e, checks });
    }
    let healthy = executors.iter().flat_map(|e| &e.checks).all(|c| matches!(c.status, CheckStatus::Present));
    let rex = process::checked(Command::new("rex").args(["--autostart=false", "whoami", "--json"]));
    let rex = CheckStatus::from_probe(rex.is_ok(), || "Rex is optional; install and start it to use --pane".into());
    let data =
        Doctor { executors, claude_profile: profile, profile_resolution: "--profile > CLAUDE_CONFIG_DIR > none", rex };
    Ok((data, if healthy { 0 } else { 1 }))
}
