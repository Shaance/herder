use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    io::{Read, Seek},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub fn output(cmd: &mut Command) -> Result<(Option<i32>, String)> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("execute {:?}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}
pub fn checked(cmd: &mut Command) -> Result<String> {
    let (code, text) = output(cmd)?;
    ensure!(code == Some(0), "exit {code:?}: {text}");
    Ok(text)
}
pub fn query(cmd: &mut Command) -> Result<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path =
        env::temp_dir().join(format!("crew-query-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let mut capture = fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path)?;
    // Unlinked capture avoids pipe-reader hangs when a query leaves descendants behind.
    fs::remove_file(path)?;
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
pub fn skills() -> PathBuf {
    env::var_os("CREW_SKILLS_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".ai-skills/skills"))
}
pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
pub fn script(owner: &str, name: &str) -> PathBuf {
    skills().join(owner).join("scripts").join(format!("{name}.sh"))
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
    checked(Command::new(script("implementer-core", "check_brief")).arg(brief).arg(mode))?;
    Ok(())
}
