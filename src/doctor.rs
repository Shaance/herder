use crate::{domain::Executor, process, scripts::ScriptExecutor};
use anyhow::Result;
use serde::Serialize;
use std::{path::PathBuf, process::Command};

#[derive(Serialize)]
pub struct Doctor {
    pub executors: Vec<ExecutorChecks>,
    pub claude_profile: Option<PathBuf>,
    pub profile_resolution: &'static str,
    pub rex: RexCheck,
}

#[derive(Serialize)]
pub struct ExecutorChecks {
    pub executor: Executor,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Serialize)]
pub struct DoctorCheck {
    pub item: PathBuf,
    pub ok: bool,
    pub fix: Option<String>,
}

#[derive(Serialize)]
pub struct RexCheck {
    pub ok: bool,
    pub fix: Option<&'static str>,
}

pub fn run(executor: Option<Executor>, profile: Option<PathBuf>) -> Result<(Doctor, i32)> {
    let profile = process::profile(profile)?;
    let mut executors = vec![];
    let mut healthy = true;
    for e in [Executor::Claude, Executor::Vibe, Executor::Codex] {
        if executor.is_some_and(|wanted| wanted != e) {
            continue;
        }
        let (name, adapter) = match e {
            Executor::Claude => ("claude", None),
            Executor::Vibe => ("vibe", Some(ScriptExecutor::VIBE)),
            Executor::Codex => ("codex", Some(ScriptExecutor::CODEX)),
        };
        let mut checks = vec![];
        let override_set = std::env::var_os("HERDER_SKILLS_DIR").is_some();
        let binaries = match (e == Executor::Claude, override_set) {
            (true, true) => vec![name],
            (true, false) => vec![name, "bash"],
            (false, _) => vec![name, "bash", "jq"],
        };
        for exe in binaries {
            let present = process::on_path(exe);
            healthy &= present;
            checks.push(DoctorCheck {
                item: exe.into(),
                ok: present,
                fix: (!present).then(|| format!("install {exe} and put its executable on PATH")),
            });
        }
        if override_set {
            let mut commands = vec![
                process::script("implementer-core", "check_brief"),
                process::script("implementer-core", "collect_evidence"),
            ];
            if let Some(adapter) = adapter {
                commands.extend(["launch", "status", "resume"].map(|a| adapter.command(a)));
            }
            for command in commands {
                let path = PathBuf::from(command.get_program());
                let present = process::executable(&path);
                healthy &= present;
                let fix = (!present).then(|| {
                    format!("restore executable {} or set HERDER_SKILLS_DIR to the skills directory", path.display())
                });
                checks.push(DoctorCheck { item: path, ok: present, fix });
            }
        } else {
            checks.push(DoctorCheck { item: "scripts (embedded)".into(), ok: true, fix: None });
        }
        executors.push(ExecutorChecks { executor: e, checks });
    }
    let rex = process::checked(Command::new("rex").args(["--autostart=false", "whoami", "--json"]));
    let data = Doctor {
        executors,
        claude_profile: profile,
        profile_resolution: "--profile > CLAUDE_CONFIG_DIR > none",
        rex: RexCheck {
            ok: rex.is_ok(),
            fix: rex.is_err().then_some("Rex is optional; install and start it to use --pane"),
        },
    };
    Ok((data, if healthy { 0 } else { 1 }))
}
