use crate::{
    domain::{Intent, Launch, Native, Observation, Run, State, Target},
    process::{self, output},
    store::LockedRun,
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
        serde_json::from_str(&process::query(command(profile).current_dir(repo).args(["agents", "--json", "--all"]))?)?;
    Ok(listed.into_iter().filter_map(|l| if let Listed::Background(a) = l { Some(a) } else { None }).collect())
}
pub fn launch(run: &mut Run, dir: &Path, store: &LockedRun) -> Result<Native> {
    let Target::Claude { profile } = run.launch.target() else { bail!("Claude target required") };
    let prompt = format!(
        "{}\n\nWrite your final report to {}.\n",
        fs::read_to_string(&run.brief)?,
        dir.join("final.md").display()
    );
    let mut cmd = command(profile.as_deref());
    cmd.current_dir(&run.repo).arg("--bg").arg(prompt).arg("-n").arg(&run.name).arg("--add-dir").arg(dir);
    // Edit --repo in place like Vibe/Codex; --bg otherwise moves into .claude/worktrees/, which verify never sees.
    cmd.args(["--settings", r#"{"worktree":{"bgIsolation":"none"}}"#]);
    if let Some(m) = &run.model {
        cmd.arg("--model").arg(m);
    }
    let (code, text) = output(cmd.args(&run.extra))?;
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
    run.launch = Launch::Accepted { id: id.into(), profile, error: None };
    store.save(run)?;
    // A new background session reports idle for a moment before it starts working; give it the launch window.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let a = identity(run)?;
        let started = a.status.as_deref() == Some("busy") || a.state == "done" || dir.join("final.md").exists();
        ensure!(
            started || Instant::now() < deadline,
            "background session {id} stayed idle for 30s with no report: {text}"
        );
        if started {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(run.launch.native().expect("identity validated before launch gate").clone())
}
// Resolve acceptance once; all later observations must match the full recorded identity.
fn identity(run: &mut Run) -> Result<Agent> {
    let (id, profile, session) = match &run.launch {
        Launch::Accepted { id, profile, .. } => (id, profile, None),
        Launch::Launched { native: Native::Claude { id, session_id, profile }, .. } => (id, profile, Some(session_id)),
        _ => bail!("Claude identity required"),
    };
    let a = agents(profile.as_deref(), &run.repo)?
        .into_iter()
        .find(|a| &a.id == id)
        .ok_or_else(|| anyhow::anyhow!("recorded Claude id {id} absent from agents"))?;
    ensure!(
        !a.session_id.is_empty() && Path::new(&a.cwd) == run.repo && session.is_none_or(|s| s == &a.session_id),
        "Claude identity mismatch: recorded={id}/{session:?}/{} observed={}/{}/{}",
        run.repo.display(),
        a.id,
        a.session_id,
        a.cwd
    );
    if let Launch::Accepted { error, .. } = &run.launch {
        run.launch = Launch::Launched {
            native: Native::Claude { id: id.clone(), session_id: a.session_id.clone(), profile: profile.clone() },
            intent: match error {
                Some(error) => Intent::Unconfirmed { error: error.clone() },
                None => Intent::Running,
            },
        };
    }
    Ok(a)
}
pub fn query(run: &mut Run) -> Result<Observation> {
    let a = identity(run)?;
    let (id, session_id) = (&a.id, &a.session_id);
    let state = match (a.status.as_deref().unwrap_or(""), a.state.as_str()) {
        (_, "failed") => State::Failed,
        (_, "done") => State::Finished,
        (_, "stopped" | "exited") => State::Stopped,
        (_, "blocked" | "waiting_for_permission") => State::Blocked,
        ("busy", _) => State::Running,
        ("idle", _) => State::Blocked,
        _ => State::Unknown,
    };
    Ok(Observation {
        state,
        evidence: format!(
            "id={id} session={session_id} status={} state={}",
            a.status.as_deref().unwrap_or("none"),
            a.state
        ),
    })
}
