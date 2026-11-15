use crate::{
    domain::{Intent, Launch, Native, Observation, Run, State, Target, UnknownCause, UnknownError},
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
pub fn launch(run: &mut Run, dir: &Path, delta: Option<&Path>, store: &LockedRun) -> Result<Native> {
    let Target::Claude { profile } = run.launch.target() else { bail!("Claude target required") };
    let previous = match &run.launch {
        Launch::PendingResume { previous, .. } => Some(previous.clone()),
        _ => None,
    };
    let report =
        if previous.is_some() { dir.join(format!("final{}.md", run.launch_attempt())) } else { dir.join("final.md") };
    let prompt = format!(
        "{}\n\nWrite your final report to {}.\n",
        fs::read_to_string(delta.unwrap_or(&run.brief))?,
        report.display()
    );
    let mut cmd = command(profile.as_deref());
    cmd.current_dir(&run.repo).arg("--bg");
    if let Some(Native::Claude { id, session_id, .. }) = &previous {
        let a = identity(run)?;
        ensure!(
            matches!(a.state.as_str(), "done" | "failed" | "stopped" | "exited"),
            "cannot resume active Claude session"
        );
        if a.status.is_some() {
            process::checked(command(profile.as_deref()).current_dir(&run.repo).args(["stop", id]))?;
            ensure!(identity(run)?.status.is_none(), "Claude session {id} is still live after stop");
        }
        // Saved options must be used: passing any launch flags makes Claude fork the session.
        cmd.arg("--resume").arg(session_id).arg(prompt);
    } else {
        cmd.arg(prompt).arg("-n").arg(&run.name).arg("--add-dir").arg(dir);
        // Edit --repo in place; --bg otherwise moves into .claude/worktrees/.
        cmd.args(["--settings", r#"{"worktree":{"bgIsolation":"none"}}"#]);
        if let Some(m) = &run.model {
            cmd.arg("--model").arg(m);
        }
        cmd.args(&run.extra);
    }
    let report_before = fs::metadata(&report).and_then(|m| m.modified()).ok();
    let (code, text) = output(&mut cmd)?;
    let backgrounded = text
        .lines()
        .find(|l| l.contains("backgrounded"))
        .and_then(|l| l.split('·').nth(1))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(Native::Claude { id, .. }) = &previous {
        let notices: Vec<_> = text
            .lines()
            .filter(|l| l.starts_with("note:"))
            .filter_map(|l| l.split_once("started a copy as").map(|(_, token)| token.trim_start()))
            .collect();
        let mut unparsed = false;
        let parsed = notices.iter().map(|token| {
            let end = token.find(|c: char| !matches!(c, '0'..='9' | 'a'..='f')).unwrap_or(token.len());
            unparsed |= end == 0;
            &token[..end]
        });
        let mut copies = Vec::new();
        for candidate in parsed.chain(backgrounded).filter(|s| !s.is_empty()) {
            if candidate != id && !copies.contains(&candidate) {
                copies.push(candidate);
            }
        }
        let mut failures = Vec::new();
        for copy_id in &copies {
            if let Err(e) = process::checked(command(profile.as_deref()).current_dir(&run.repo).args(["stop", copy_id]))
            {
                failures.push(format!("{copy_id}: {e}"));
            }
        }
        ensure!(
            !unparsed,
            "Claude resume created a copy; no copy id could be parsed; the copy must be stopped manually{}; {text}",
            if failures.is_empty() { String::new() } else { format!("; cleanup failures: {}", failures.join("; ")) }
        );
        ensure!(
            notices.is_empty() && copies.is_empty(),
            "Claude resume created a copy; refused identity change; cleanup failures: {}; {text}",
            failures.join("; ")
        );
    }
    ensure!(
        code == Some(0) && !text.contains("idle — send a prompt") && !text.contains("Workspace not trusted"),
        "{text}"
    );
    let id = backgrounded.ok_or_else(|| anyhow::anyhow!("missing background id: {text}"))?;
    if previous.is_none() {
        run.launch = Launch::Accepted { id: id.into(), profile, error: None };
        store.save(run)?;
    }
    // A new background session reports idle for a moment before it starts working; give it the launch window.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let a = identity(run)?;
        let started = if previous.is_some() {
            a.status.as_deref() == Some("busy")
                || a.state == "working"
                || fs::metadata(&report)
                    .and_then(|m| m.modified())
                    .ok()
                    .is_some_and(|now| report_before.is_none_or(|before| now > before))
        } else {
            a.status.as_deref() == Some("busy") || a.state == "done" || report.exists()
        };
        ensure!(
            started || Instant::now() < deadline,
            "background session {id} stayed idle for 30s with no report: {text}"
        );
        if started {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(previous.unwrap_or_else(|| run.launch.native().expect("identity validated before launch gate").clone()))
}
// Resolve acceptance once; all later observations must match the full recorded identity.
fn identity(run: &mut Run) -> Result<Agent> {
    let (id, profile, session) = match &run.launch {
        Launch::Accepted { id, profile, .. } => (id, profile, None),
        Launch::Launched { native: Native::Claude { id, session_id, profile }, .. }
        | Launch::PendingResume { previous: Native::Claude { id, session_id, profile }, .. } => {
            (id, profile, Some(session_id))
        }
        _ => bail!("Claude identity required"),
    };
    let a = agents(profile.as_deref(), &run.repo)?.into_iter().find(|a| &a.id == id).ok_or_else(|| {
        UnknownError(UnknownCause::IdentityMismatch, format!("recorded Claude id {id} absent from agents"))
    })?;
    ensure!(
        !a.session_id.is_empty() && Path::new(&a.cwd) == run.repo && session.is_none_or(|s| s == &a.session_id),
        UnknownError(
            UnknownCause::IdentityMismatch,
            format!(
                "Claude identity mismatch: recorded={id}/{session:?}/{} observed={}/{}/{}",
                run.repo.display(),
                a.id,
                a.session_id,
                a.cwd
            )
        )
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
    Ok(Observation::new(
        state,
        format!("id={id} session={session_id} status={} state={}", a.status.as_deref().unwrap_or("none"), a.state),
        UnknownCause::UnrecognizedStatus,
    ))
}
