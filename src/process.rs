use anyhow::{Context, Result, ensure};
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
pub fn lint(brief: &Path) -> Result<()> {
    checked(Command::new(script("implementer-core", "check_brief")).arg(brief).arg("direct"))?;
    Ok(())
}
