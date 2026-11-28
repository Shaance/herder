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
pub const OSC7577_TOKEN: &str = "OSC7577_TOKEN";

pub fn output(cmd: &mut Command) -> Result<(Option<i32>, String)> {
    let out = cmd
        .env_remove(OSC7577_TOKEN)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("execute {:?}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}
pub fn checked(cmd: &mut Command) -> Result<String> {
    checked_until(cmd, None)
}
pub fn checked_until(cmd: &mut Command, deadline: Option<Instant>) -> Result<String> {
    Ok(String::from_utf8_lossy(&checked_bytes_until(cmd, deadline)?).into_owned())
}
pub fn checked_bytes_until(cmd: &mut Command, deadline: Option<Instant>) -> Result<Vec<u8>> {
    let (status, stdout, stderr) = if let Some(deadline) = deadline {
        let (status, stdout, stderr) = bounded_output_bytes(cmd, deadline)?;
        (status.context("command timed out")?, stdout, stderr)
    } else {
        let out = cmd
            .env_remove(OSC7577_TOKEN)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("execute {:?}", cmd.get_program()))?;
        (out.status, out.stdout, out.stderr)
    };
    ensure!(
        status.success(),
        "exit {:?}: {}{}",
        status.code(),
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    Ok(stdout)
}

pub fn query(cmd: &mut Command, deadline: Option<Instant>) -> Result<(String, String)> {
    let limit = Instant::now() + EXECUTOR_QUERY_TIMEOUT;
    let (status, stdout, stderr) = bounded_output(cmd, deadline.map_or(limit, |end| end.min(limit)))?;
    let status = status.ok_or_else(|| anyhow::anyhow!("executor query timed out: {stdout}{stderr}"))?;
    ensure!(status.success(), "exit {:?}: {stdout}{stderr}", status.code());
    Ok((stdout, stderr))
}

pub fn bounded_output(cmd: &mut Command, deadline: Instant) -> Result<(Option<ExitStatus>, String, String)> {
    let (status, stdout, stderr) = bounded_output_bytes(cmd, deadline)?;
    Ok((status, String::from_utf8_lossy(&stdout).into_owned(), String::from_utf8_lossy(&stderr).into_owned()))
}

fn bounded_output_bytes(cmd: &mut Command, deadline: Instant) -> Result<(Option<ExitStatus>, Vec<u8>, Vec<u8>)> {
    ensure!(Instant::now() < deadline, "command deadline elapsed");
    // Unlinked capture avoids pipe-reader hangs when a command leaves descendants behind.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = cmd
        .env_remove(OSC7577_TOKEN)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .process_group(0)
        .spawn()?;
    let status = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = Command::new("/bin/kill")
                .env_remove(OSC7577_TOKEN)
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status();
            let _ = child.kill();
            child.wait()?;
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        std::thread::sleep(Duration::from_millis(20).min(remaining));
    };
    let read_capture = |file: &mut std::fs::File| -> Result<Vec<u8>> {
        let size = file.metadata()?.len();
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(size).read_to_end(&mut bytes)?;
        Ok(bytes)
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

#[derive(Clone, Copy)]
pub enum ScriptSource {
    Configured,
    Embedded,
}

impl ScriptSource {
    pub fn override_dir(self) -> Option<PathBuf> {
        match self {
            Self::Configured => env::var_os("HERDER_SKILLS_DIR").map(PathBuf::from),
            Self::Embedded => None,
        }
    }
}

pub fn script(script: Script) -> Command {
    script_from(script, ScriptSource::Configured)
}

pub fn script_from(script: Script, source: ScriptSource) -> Command {
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
    if let Some(dir) = source.override_dir() {
        return Command::new(dir.join(owner).join("scripts").join(file));
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
        let (stdout, stderr) = result.unwrap();
        assert_eq!(stdout, "stdout");
        assert_eq!(stderr, "stderr");
        assert!(started.elapsed() < Duration::from_secs(4), "query waited for its descendant");
    }
}
