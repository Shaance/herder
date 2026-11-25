use crate::{
    domain::{LaunchPhase, Native, Observation, Run, State, Target, UnknownCause, UnknownError},
    process::{Script, checked, output, query, script},
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{BufRead, BufReader, ErrorKind},
    num::NonZeroU32,
    path::Path,
    process::Command,
    time::Instant,
};

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
        if matches!(previous, Some(Native::Codex { .. })) {
            return Ok(None);
        }
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
    pub fn query(&self, run: &Run, dir: &Path, deadline: Option<Instant>) -> Result<(Observation, Result<Native>)> {
        let [_, status, _] = self.scripts;
        let (stdout, stderr) = query(script(status).arg(dir).args(self.status_args(run)), deadline)?;
        let text = stdout + &stderr;
        let identity = run.launch.native().cloned().map_or_else(|| self.identity(dir), Ok);
        if let Ok(native) = &identity {
            confirm(native, dir)?;
            ensure!(
                field(&text, "pid")? == native.process().unwrap().0.to_string(),
                UnknownError(UnknownCause::IdentityMismatch, format!("status pid disagrees with record: {text}"))
            );
        }
        // Vibe can print worker stderr after reporting a terminal verdict.
        let verdict =
            text.lines().find_map(|line| line.strip_prefix("verdict: ").and_then(|v| v.split_whitespace().next()));
        let state = match (field(&text, "state")?, verdict) {
            ("alive", _) => State::Running,
            ("exited", Some("COMPLETED")) => State::Finished,
            ("exited", Some("STOPPED")) => State::Stopped,
            ("exited", Some("DIED" | "FAILED")) => State::Failed,
            _ => return Ok((Observation::unknown(UnknownCause::UnrecognizedStatus, text), identity)),
        };
        Ok((Observation::known(state, text), identity))
    }
}
pub fn codex_resume_started(run: &Run, dir: &Path, thread_id: &str, pid: NonZeroU32) -> Result<bool> {
    let path = dir.join(run.launch_events_name());
    let Some(event) = codex_thread_started(&path).with_context(|| format!("read {}", path.display()))? else {
        return Ok(false);
    };
    let observed = event["thread_id"].as_str();
    ensure!(
        observed == Some(thread_id),
        UnknownError(
            UnknownCause::IdentityMismatch,
            format!(
                "Codex resume pid {pid}: expected thread {thread_id:?}, observed {:?}",
                observed.unwrap_or("<missing or invalid>")
            )
        )
    );
    Ok(true)
}

fn codex_thread_started(path: &Path) -> std::io::Result<Option<serde_json::Value>> {
    let mut stream = match fs::File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut line = Vec::new();
    loop {
        stream.read_until(b'\n', &mut line)?;
        if !line.ends_with(b"\n") {
            return Ok(None);
        }
        if let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line)
            && event["type"] == "thread.started"
        {
            return Ok(Some(event));
        }
        line.clear();
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
