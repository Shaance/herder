use anyhow::{Context, Result, ensure};
use std::{
    env,
    io::{Read, Seek},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

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
pub fn query(cmd: &mut Command) -> Result<String> {
    // Unlinked capture avoids pipe-reader hangs when a query leaves descendants behind.
    let mut capture = tempfile::tempfile()?;
    let mut child =
        cmd.stdin(Stdio::null()).stdout(capture.try_clone()?).stderr(capture.try_clone()?).process_group(0).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = Command::new("/bin/kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
            let _ = child.kill();
            child.wait()?;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    capture.rewind()?;
    let mut text = String::new();
    capture.read_to_string(&mut text)?;
    let status = status.ok_or_else(|| anyhow::anyhow!("executor query timed out after 2s: {text}"))?;
    ensure!(status.success(), "exit {:?}: {text}", status.code());
    Ok(text)
}
pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
pub fn script(owner: &str, name: &str) -> Command {
    if let Some(dir) = env::var_os("HERDER_SKILLS_DIR") {
        return Command::new(PathBuf::from(dir).join(owner).join("scripts").join(format!("{name}.sh")));
    }
    let text = match (owner, name) {
        ("implementer-core", "check_brief") => {
            include_str!("../vendor/ai-skills/implementer-core/scripts/check_brief.sh")
        }
        ("implementer-core", "collect_evidence") => {
            include_str!("../vendor/ai-skills/implementer-core/scripts/collect_evidence.sh")
        }
        ("vibe-implementer", "launch_vibe") => {
            include_str!("../vendor/ai-skills/vibe-implementer/scripts/launch_vibe.sh")
        }
        ("vibe-implementer", "status_vibe") => {
            include_str!("../vendor/ai-skills/vibe-implementer/scripts/status_vibe.sh")
        }
        ("vibe-implementer", "resume_vibe") => {
            include_str!("../vendor/ai-skills/vibe-implementer/scripts/resume_vibe.sh")
        }
        ("codex-implementer", "launch_classic") => {
            include_str!("../vendor/ai-skills/codex-implementer/scripts/launch_classic.sh")
        }
        ("codex-implementer", "status_classic") => {
            include_str!("../vendor/ai-skills/codex-implementer/scripts/status_classic.sh")
        }
        ("codex-implementer", "resume_classic") => {
            include_str!("../vendor/ai-skills/codex-implementer/scripts/resume_classic.sh")
        }
        _ => unreachable!("unknown embedded script: {owner}/{name}"),
    };
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(text).arg(format!("{name}.sh"));
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
    checked(script("implementer-core", "check_brief").arg(brief).arg(mode))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_returns_when_descendant_inherits_output() {
        let started = Instant::now();
        let result = query(Command::new("/bin/sh").args(["-c", "sleep 6 & printf stdout; printf stderr >&2"]));
        assert_eq!(result.unwrap(), "stdoutstderr");
        assert!(started.elapsed() < Duration::from_secs(4), "query waited for its descendant");
    }
}
