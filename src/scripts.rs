use crate::{
    domain::{LaunchPhase, Native, Observation, Run, State, Target, UnknownCause, UnknownError},
    process::{Script, checked, output, query, script},
};
use anyhow::{Result, ensure};
use std::{fs, num::NonZeroU32, path::Path, process::Command};

pub struct ScriptExecutor {
    pub scripts: [Script; 3],
    identity: (&'static str, &'static str),
    launch_separator: Option<&'static str>,
    numbered_status: bool,
    native: fn(String, NonZeroU32) -> Native,
}
impl ScriptExecutor {
    pub const VIBE: Self = Self {
        scripts: [Script::LaunchVibe, Script::StatusVibe, Script::ResumeVibe],
        identity: ("session-id", "SESSION_ID"),
        launch_separator: Some("--"),
        numbered_status: true,
        native: |session_id, pid| Native::Vibe { session_id, pid },
    };
    pub const CODEX: Self = Self {
        scripts: [Script::LaunchClassic, Script::StatusClassic, Script::ResumeClassic],
        identity: ("thread-id", "THREAD_ID"),
        launch_separator: None,
        numbered_status: false,
        native: |thread_id, pid| Native::Codex { thread_id, pid },
    };
    pub fn status_args(&self, run: &Run) -> Option<String> {
        self.numbered_status
            .then(|| if run.launch_attempt().get() == 1 { String::new() } else { run.launch_attempt().to_string() })
    }
    fn native(&self, pid: NonZeroU32, id: String) -> Result<Native> {
        ensure!(!id.is_empty() && !id.chars().any(char::is_whitespace), "invalid native id");
        Ok((self.native)(id, pid))
    }
    fn identity(&self, dir: &Path) -> Result<Native> {
        self.native(
            fs::read_to_string(dir.join("run.pid"))?.trim().parse()?,
            fs::read_to_string(dir.join(self.identity.0))?.trim().into(),
        )
    }
    pub fn launch(&self, run: &Run, dir: &Path, phase: LaunchPhase<'_>) -> Result<Option<Native>> {
        let [launch, _, resume] = self.scripts;
        let (mut cmd, suffix, previous) = match phase {
            LaunchPhase::Initial => {
                let mut cmd = script(launch);
                cmd.arg(&run.repo).arg(&run.brief).arg(dir).args(self.launch_separator);
                (cmd, run.suffix(), None)
            }
            LaunchPhase::Resume { previous, delta } => {
                let suffix = run.launch_attempt().to_string();
                let mut cmd = script(resume);
                cmd.arg(dir).arg(&run.repo).arg(delta).arg(&suffix).arg("--");
                (cmd, suffix, Some(previous))
            }
        };
        cmd.env_remove("REX_SESSION").env_remove("REX_BLOCK").env_remove("REX_SERVER");
        if let Some(model) = &run.model {
            cmd.arg("--model").arg(model);
        }
        if let (Target::Codex, Some(effort)) = (run.launch.target(), &run.requested_effort) {
            cmd.arg("-c").arg(format!("model_reasoning_effort={effort}"));
        }
        cmd.args(&run.extra);
        let (code, text) = output(&mut cmd)?;
        crate::store::atomic(&dir.join(format!("launch{suffix}.log")), text.as_bytes())?;
        if code == Some(3) {
            return Ok(None);
        }
        ensure!(code == Some(0), "{text}");
        let native = self.native(field(&text, "PID")?.parse()?, field(&text, self.identity.1)?.into())?;
        if let Some(old) = previous {
            ensure!(
                old.process().map(|p| p.2) == native.process().map(|p| p.2),
                "resumed identity disagrees with record: {text}"
            );
            ensure!(old.process().map(|p| p.0) != native.process().map(|p| p.0), "resume returned the old pid: {text}");
        }
        Ok(Some(native))
    }
    pub fn query(&self, run: &Run, dir: &Path) -> Result<(Observation, Result<Native>)> {
        let [_, status, _] = self.scripts;
        let text = query(script(status).arg(dir).args(self.status_args(run)))?;
        let identity = run.launch.native().cloned().map_or_else(|| self.identity(dir), Ok);
        if let Ok(native) = &identity {
            confirm(native, dir)?;
            ensure!(
                field(&text, "pid")? == native.process().unwrap().0.to_string(),
                UnknownError(UnknownCause::IdentityMismatch, format!("status pid disagrees with record: {text}"))
            );
        }
        let state = match field(&text, "state")? {
            "alive" => State::Running,
            "exited" if text.contains("verdict: COMPLETED") => State::Finished,
            "exited" if text.contains("verdict: STOPPED") => State::Stopped,
            "exited" if text.contains("verdict: DIED") || text.contains("verdict: FAILED") => State::Failed,
            _ => return Ok((Observation::unknown(UnknownCause::UnrecognizedStatus, text), identity)),
        };
        Ok((Observation::known(state, text), identity))
    }
}
fn field<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    text.split_whitespace()
        .find_map(|s| s.strip_prefix(&format!("{key}=")))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing {key}: {text}"))
}
pub fn confirm(native: &Native, dir: &Path) -> Result<()> {
    let (pid, file, id) = native.process().ok_or_else(|| anyhow::anyhow!("script identity required"))?;
    ensure!(
        fs::read_to_string(dir.join("run.pid"))?.trim() == pid.to_string()
            && fs::read_to_string(dir.join(file))?.trim() == id,
        UnknownError(UnknownCause::IdentityMismatch, "executor files disagree with recorded identity".into())
    );
    Ok(())
}
pub fn stop(native: &Native, run: &Run, dir: &Path) -> Result<()> {
    confirm(native, dir)?;
    let (pid, _, _) = native.process().ok_or_else(|| anyhow::anyhow!("script identity required"))?;
    let args = checked(Command::new("ps").args(["-p", &pid.to_string(), "-o", "args="]))?;
    let marker = match native {
        Native::Vibe { .. } => dir.join(format!("out{}", run.suffix())),
        _ => dir.join(run.report_name()),
    };
    ensure!(
        format!("{} ", args.trim()).contains(&format!(" {} ", marker.display())),
        "pid {pid} command does not belong to this run; refusing TERM"
    );
    checked(Command::new("kill").args(["-TERM", &pid.to_string()]))?;
    Ok(())
}
