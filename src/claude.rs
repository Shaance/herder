use crate::{
    domain::{Native, Observation, Run, State, Target},
    process::{checked, output},
};
use anyhow::{Result, bail, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub fn command(profile: Option<&Path>) -> Command {
    let mut cmd = Command::new("claude");
    if let Some(p) = profile {
        cmd.env("CLAUDE_CONFIG_DIR", p);
    } else {
        cmd.env_remove("CLAUDE_CONFIG_DIR");
    }
    cmd
}
// `claude agents` also lists interactive sessions, which carry no short id; only background ones are workers.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Listed {
    Background(Agent),
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Agent {
    id: String,
    session_id: String,
    cwd: String,
    status: Option<String>,
    state: String,
}
fn agents(profile: Option<&Path>, repo: &Path) -> Result<Vec<Agent>> {
    let listed: Vec<Listed> =
        serde_json::from_str(&checked(command(profile).current_dir(repo).args(["agents", "--json", "--all"]))?)?;
    Ok(listed.into_iter().filter_map(|l| if let Listed::Background(a) = l { Some(a) } else { None }).collect())
}
pub fn launch(run: &Run, dir: &Path, extra: &[String]) -> Result<Native> {
    let Target::Claude { profile } = run.launch.target() else { bail!("Claude target required") };
    let prompt = format!(
        "{}\n\nWrite your final report to {}.\n",
        fs::read_to_string(&run.brief)?,
        dir.join("final.md").display()
    );
    let mut cmd = command(profile.as_deref());
    cmd.current_dir(&run.repo).arg("--bg").arg(prompt).arg("-n").arg(&run.name).arg("--add-dir").arg(dir);
    if let Some(m) = &run.model {
        cmd.arg("--model").arg(m);
    }
    let (code, text) = output(cmd.args(extra))?;
    ensure!(
        code == Some(0) && !text.contains("idle — send a prompt") && !text.contains("Workspace not trusted"),
        "{text}"
    );
    let id = text
        .lines()
        .find(|l| l.contains("backgrounded"))
        .and_then(|l| l.split('·').nth(1))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing background id: {text}"))?;
    // A new background session reports idle for a moment before it starts working; give it the launch window.
    let deadline = Instant::now() + Duration::from_secs(30);
    let a = loop {
        let a = agents(profile.as_deref(), &run.repo)?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow::anyhow!("background id {id} absent from agents: {text}"))?;
        let started = a.status.as_deref() == Some("busy") || a.state == "done" || dir.join("final.md").exists();
        ensure!(started || Instant::now() < deadline, "background session {id} stayed idle for 30s with no report: {text}");
        if started {
            break a;
        }
        thread::sleep(Duration::from_secs(1));
    };
    ensure!(
        Path::new(&a.cwd) == run.repo && !a.session_id.is_empty(),
        "background identity disagrees with launch: {text}"
    );
    Ok(Native::Claude { id: id.into(), session_id: a.session_id, profile })
}
pub fn query(native: &Native, run: &Run, dir: &Path) -> Result<Observation> {
    let Native::Claude { id, session_id, profile } = native else { bail!("Claude identity required") };
    let a = agents(profile.as_deref(), &run.repo)?
        .into_iter()
        .find(|a| &a.id == id)
        .ok_or_else(|| anyhow::anyhow!("recorded Claude id {id} absent from agents"))?;
    ensure!(
        &a.session_id == session_id && Path::new(&a.cwd) == run.repo,
        "Claude identity mismatch: recorded={id}/{session_id}/{} observed={}/{}/{}",
        run.repo.display(),
        a.id,
        a.session_id,
        a.cwd
    );
    let state = match (a.status.as_deref().unwrap_or(""), a.state.as_str()) {
        (_, "failed") => State::Failed,
        (_, "done") => State::Finished,
        (_, "stopped" | "exited") => State::Stopped,
        (_, "blocked" | "waiting_for_permission") => State::Blocked,
        ("busy", _) => State::Running,
        ("idle", _) if dir.join(run.report_name()).exists() => State::Finished,
        ("idle", _) => State::Blocked,
        _ => State::Unknown,
    };
    Ok(Observation { state, evidence: format!("id={id} session={session_id} status={} state={}", a.status.as_deref().unwrap_or("none"), a.state) })
}
