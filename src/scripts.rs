use crate::{
    domain::{Launch, Native, Observation, Run, State},
    process::{checked, output, query, script},
};
use anyhow::{Result, ensure};
use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
};

pub struct ScriptExecutor {
    script: (&'static str, &'static str),
    identity: (&'static str, &'static str),
    launch_separator: Option<&'static str>,
    numbered_status: bool,
    native: fn(String, NonZeroU32) -> Native,
}
impl ScriptExecutor {
    pub const VIBE: Self = Self {
        script: ("vibe-implementer", "vibe"),
        identity: ("session-id", "SESSION_ID"),
        launch_separator: Some("--"),
        numbered_status: true,
        native: |session_id, pid| Native::Vibe { session_id, pid },
    };
    pub const CODEX: Self = Self {
        script: ("codex-implementer", "classic"),
        identity: ("thread-id", "THREAD_ID"),
        launch_separator: None,
        numbered_status: false,
        native: |thread_id, pid| Native::Codex { thread_id, pid },
    };
    pub fn path(&self, action: &str) -> PathBuf {
        script(self.script.0, &format!("{action}_{}", self.script.1))
    }
    pub fn status_args(&self, run: &Run) -> Option<String> {
        self.numbered_status
            .then(|| if run.launch_attempt().get() == 1 { String::new() } else { run.launch_attempt().to_string() })
    }
    fn native(&self, pid: NonZeroU32, id: String) -> Result<Native> {
        ensure!(!id.is_empty() && !id.chars().any(char::is_whitespace), "invalid native id");
        Ok((self.native)(id, pid))
    }
    pub fn identity(&self, dir: &Path) -> Result<Native> {
        self.native(
            fs::read_to_string(dir.join("run.pid"))?.trim().parse()?,
            fs::read_to_string(dir.join(self.identity.0))?.trim().into(),
        )
    }
    pub fn launch(&self, run: &Run, dir: &Path, delta: Option<&Path>) -> Result<Option<Native>> {
        let mut cmd = Command::new(self.path(if delta.is_some() { "resume" } else { "launch" }));
        cmd.env_remove("REX_SESSION").env_remove("REX_BLOCK").env_remove("REX_SERVER");
        if let Some(delta) = delta {
            cmd.arg(dir).arg(&run.repo).arg(delta).arg(run.launch_attempt().to_string()).arg("--");
        } else {
            cmd.arg(&run.repo).arg(&run.brief).arg(dir);
            cmd.args(self.launch_separator);
        }
        if let Some(model) = &run.model {
            cmd.arg("--model").arg(model);
        }
        cmd.args(&run.extra);
        let (code, text) = output(&mut cmd)?;
        crate::store::atomic(
            &dir.join(format!(
                "launch{}.log",
                if delta.is_some() { run.launch_attempt().to_string() } else { run.suffix() }
            )),
            text.as_bytes(),
        )?;
        if code == Some(3) {
            return Ok(None);
        }
        ensure!(code == Some(0), "{text}");
        let native = self.native(field(&text, "PID")?.parse()?, field(&text, self.identity.1)?.into())?;
        if let Launch::PendingResume { previous: old, .. } = &run.launch {
            ensure!(
                old.process().map(|p| p.2) == native.process().map(|p| p.2),
                "resumed identity disagrees with record: {text}"
            );
            ensure!(old.process().map(|p| p.0) != native.process().map(|p| p.0), "resume returned the old pid: {text}");
        }
        Ok(Some(native))
    }
    pub fn query(&self, run: &Run, dir: &Path) -> Result<Observation> {
        let text = query(Command::new(self.path("status")).arg(dir).args(self.status_args(run)))?;
        let recovered = self.identity(dir).ok();
        if let Some(native) = run.launch.native().or(recovered.as_ref()) {
            confirm(native, dir)?;
            ensure!(
                field(&text, "pid")? == native.process().unwrap().0.to_string(),
                "status pid disagrees with record: {text}"
            );
        }
        let state = match field(&text, "state")? {
            "alive" => State::Running,
            "exited" if text.contains("verdict: COMPLETED") => State::Finished,
            "exited" if text.contains("verdict: STOPPED") => State::Stopped,
            "exited" if text.contains("verdict: DIED") || text.contains("verdict: FAILED") => State::Failed,
            _ => State::Unknown,
        };
        Ok(Observation { state, evidence: text })
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
        "executor files disagree with recorded identity"
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
