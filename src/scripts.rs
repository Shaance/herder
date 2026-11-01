use crate::{
    domain::{Executor, Native, Observation, Run, State},
    process::{checked, output, script},
};
use anyhow::{Result, bail, ensure};
use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
};

pub fn path(executor: Executor, action: &str) -> PathBuf {
    let (owner, suffix) = match executor {
        Executor::Vibe => ("vibe-implementer", "vibe"),
        Executor::Codex => ("codex-implementer", "classic"),
        Executor::Claude => unreachable!("Claude has no scripts"),
    };
    script(owner, &format!("{action}_{suffix}"))
}
fn field<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    text.split_whitespace()
        .find_map(|s| s.strip_prefix(&format!("{key}=")))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing {key}: {text}"))
}
fn native(executor: Executor, pid: NonZeroU32, id: String) -> Result<Native> {
    ensure!(!id.is_empty() && !id.chars().any(char::is_whitespace), "invalid native id");
    Ok(match executor {
        Executor::Vibe => Native::Vibe { session_id: id, pid },
        Executor::Codex => Native::Codex { thread_id: id, pid },
        Executor::Claude => bail!("script identity cannot be Claude"),
    })
}
pub fn identity(executor: Executor, dir: &Path) -> Result<Native> {
    let file = if executor == Executor::Vibe { "session-id" } else { "thread-id" };
    native(
        executor,
        fs::read_to_string(dir.join("run.pid"))?.trim().parse()?,
        fs::read_to_string(dir.join(file))?.trim().into(),
    )
}
pub fn launch(run: &Run, dir: &Path, delta: Option<&Path>, extra: &[String]) -> Result<Option<Native>> {
    let executor = run.launch.target().executor();
    let mut cmd = Command::new(path(executor, if delta.is_some() { "resume" } else { "launch" }));
    if let Some(delta) = delta {
        cmd.arg(dir).arg(&run.repo).arg(delta).arg(run.attempt.to_string());
    } else {
        cmd.arg(&run.repo).arg(&run.brief).arg(dir);
        if executor == Executor::Vibe {
            cmd.arg("--");
        }
        if let Some(model) = &run.model {
            cmd.arg("--model").arg(model);
        }
        cmd.args(extra);
    }
    let (code, text) = output(&mut cmd)?;
    crate::store::atomic(&dir.join(format!("launch{}.log", run.suffix())), text.as_bytes())?;
    if code == Some(3) {
        return Ok(None);
    }
    ensure!(code == Some(0), "{text}");
    let key = if executor == Executor::Vibe { "SESSION_ID" } else { "THREAD_ID" };
    let native = native(executor, field(&text, "PID")?.parse()?, field(&text, key)?.into())?;
    if let Some(old) = run.launch.native() {
        ensure!(
            old.process().map(|p| p.2) == native.process().map(|p| p.2),
            "resumed identity disagrees with record: {text}"
        );
    }
    Ok(Some(native))
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
pub fn query(run: &Run, dir: &Path) -> Result<Observation> {
    let executor = run.launch.target().executor();
    let mut cmd = Command::new(path(executor, "status"));
    cmd.arg(dir);
    if executor == Executor::Vibe {
        cmd.arg(run.suffix());
    }
    let text = checked(&mut cmd)?;
    if let Some(native) = run.launch.native() {
        confirm(native, dir)?;
    }
    let live = field(&text, "state")?;
    if let Some(native) = run.launch.native() {
        ensure!(
            field(&text, "pid")? == native.process().unwrap().0.to_string(),
            "status pid disagrees with record: {text}"
        );
    }
    let state = if live == "alive" {
        State::Running
    } else if live != "exited" {
        State::Unknown
    } else if executor == Executor::Codex && run.attempt.get() > 1 {
        return Ok(Observation {
            state: State::Unknown,
            evidence: format!(
                "status_classic.sh inspects attempt 1; cannot establish attempt {} outcome: {text}",
                run.attempt
            ),
        });
    } else if text.contains("verdict: COMPLETED") {
        State::Finished
    } else if text.contains("verdict: STOPPED") {
        State::Stopped
    } else if text.contains("verdict: DIED") || text.contains("verdict: FAILED") {
        State::Failed
    } else {
        State::Unknown
    };
    Ok(Observation { state, evidence: text })
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
