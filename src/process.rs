use anyhow::{Context, Result, ensure};
use std::{
    env,
    io::{Read, Seek},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const EXECUTOR_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

pub fn output(cmd: &mut Command) -> Result<(Option<i32>, String)> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("execute {:?}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}
pub fn checked(cmd: &mut Command) -> Result<String> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("execute {:?}", cmd.get_program()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    ensure!(out.status.success(), "exit {:?}: {}{}", out.status.code(), stdout, String::from_utf8_lossy(&out.stderr));
    Ok(stdout.into_owned())
}
pub fn checked_until(cmd: &mut Command, deadline: Option<Instant>) -> Result<String> {
    let Some(deadline) = deadline else { return checked(cmd) };
    let (status, stdout, stderr) = bounded_output(cmd, deadline)?;
    let status = status.context("command timed out")?;
    ensure!(status.success(), "exit {:?}: {stdout}{stderr}", status.code());
    Ok(stdout)
}

pub fn query(cmd: &mut Command, deadline: Option<Instant>) -> Result<String> {
    let limit = Instant::now() + EXECUTOR_QUERY_TIMEOUT;
    let (status, stdout, stderr) = bounded_output(cmd, deadline.map_or(limit, |end| end.min(limit)))?;
    let status = status.ok_or_else(|| anyhow::anyhow!("executor query timed out: {stdout}{stderr}"))?;
    ensure!(status.success(), "exit {:?}: {stdout}{stderr}", status.code());
    Ok(stdout + &stderr)
}

pub fn bounded_output(cmd: &mut Command, deadline: Instant) -> Result<(Option<ExitStatus>, String, String)> {
    ensure!(Instant::now() < deadline, "command deadline elapsed");
    // Unlinked capture avoids pipe-reader hangs when a command leaves descendants behind.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child =
        cmd.stdin(Stdio::null()).stdout(stdout.try_clone()?).stderr(stderr.try_clone()?).process_group(0).spawn()?;
    let status = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
            let _ = child.kill();
            child.wait()?;
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        std::thread::sleep(Duration::from_millis(20).min(remaining));
    };
    let read_capture = |file: &mut std::fs::File| -> Result<String> {
        let size = file.metadata()?.len();
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(size).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    Ok((status, read_capture(&mut stdout)?, read_capture(&mut stderr)?))
}
pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
#[derive(Clone, Copy)]
pub enum Script {
    CheckBrief,
    CollectEvidence,
    LaunchVibe,
    StatusVibe,
    ResumeVibe,
    LaunchClassic,
    StatusClassic,
    ResumeClassic,
}

pub fn script(script: Script) -> Command {
    macro_rules! embedded {
        ($owner:literal, $file:literal) => {
            ($owner, $file, include_str!(concat!("../vendor/ai-skills/", $owner, "/scripts/", $file)))
        };
    }
    let (owner, file, text) = match script {
        Script::CheckBrief => embedded!("implementer-core", "check_brief.sh"),
        Script::CollectEvidence => embedded!("implementer-core", "collect_evidence.sh"),
        Script::LaunchVibe => embedded!("vibe-implementer", "launch_vibe.sh"),
        Script::StatusVibe => embedded!("vibe-implementer", "status_vibe.sh"),
        Script::ResumeVibe => embedded!("vibe-implementer", "resume_vibe.sh"),
        Script::LaunchClassic => embedded!("codex-implementer", "launch_classic.sh"),
        Script::StatusClassic => embedded!("codex-implementer", "status_classic.sh"),
        Script::ResumeClassic => embedded!("codex-implementer", "resume_classic.sh"),
    };
    if let Some(dir) = env::var_os("HERDER_SKILLS_DIR") {
        return Command::new(PathBuf::from(dir).join(owner).join("scripts").join(file));
    }
    let mut cmd = Command::new("bash");
    cmd.args(["-c", text, file]);
    cmd
}
pub fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub fn on_path(name: &str) -> bool {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|p| executable(&p.join(name)))
}
pub fn profile(value: Option<PathBuf>) -> Result<Option<PathBuf>> {
    value
        .or_else(|| env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .map(|p| std::path::absolute(p).map_err(Into::into))
        .transpose()
}
pub fn lint(brief: &Path, mode: &str) -> Result<()> {
    checked(script(Script::CheckBrief).arg(brief).arg(mode))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_returns_when_descendant_inherits_output() {
        let started = Instant::now();
        let result = query(Command::new("/bin/sh").args(["-c", "sleep 6 & printf stdout; printf stderr >&2"]), None);
        assert_eq!(result.unwrap(), "stdoutstderr");
        assert!(started.elapsed() < Duration::from_secs(4), "query waited for its descendant");
    }
}
