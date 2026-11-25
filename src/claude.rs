use crate::{
    domain::{Intent, Launch, LaunchPhase, Native, Observation, Run, State, UnknownCause, UnknownError},
    process::{self, output},
    store::{self, LockedRun},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    fs,
    io::{self, Write},
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
    pid: Option<u32>,
    state: String,
}
fn agents(profile: Option<&Path>, repo: &Path, deadline: Option<Instant>) -> Result<Vec<Agent>> {
    let (stdout, _) = process::query(command(profile).current_dir(repo).args(["agents", "--json", "--all"]), deadline)?;
    let listed: Vec<Listed> = serde_json::from_str(&stdout)?;
    Ok(listed.into_iter().filter_map(|l| if let Listed::Background(a) = l { Some(a) } else { None }).collect())
}
pub fn stop_hook(dir: &Path) -> Result<()> {
    #[derive(Deserialize)]
    struct Reply {
        last_assistant_message: Option<String>,
    }
    let reply: Reply = serde_json::from_reader(io::stdin().lock())?;
    if let Some(message) = reply.last_assistant_message.filter(|s| !s.is_empty()) {
        // Launch holds run.lock and may be waiting for this report. run.json is published atomically.
        let run = store::load(dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(message.as_bytes())?;
        temp.persist(dir.join(run.launch_report_name()))?;
    }
    Ok(())
}

pub fn launch(run: &mut Run, profile: Option<&Path>, phase: LaunchPhase<'_>, store: &LockedRun) -> Result<Native> {
    let dir = &store.dir;
    let report = dir.join(run.launch_report_name());
    let (previous, brief) = match phase {
        LaunchPhase::Initial => (None, run.brief.as_path()),
        LaunchPhase::Resume { previous, delta } => (Some(previous), delta),
    };
    let prompt = format!("{}\n\nYour last reply is your final report.\n", fs::read_to_string(brief)?);
    let mut cmd = command(profile);
    cmd.current_dir(&run.repo).arg("--bg");
    if let Some(Native::Claude { id, session_id, .. }) = previous {
        let (_, a) = identity(run, previous, profile, None)?;
        ensure!(
            matches!(a.state.as_str(), "done" | "failed" | "stopped" | "exited"),
            "cannot resume active Claude session"
        );
        if a.status.is_some() {
            process::checked(command(profile).current_dir(&run.repo).args(["stop", id]))?;
        }
        // Claude clears the listing before the stopped process exits.
        if let Some(pid) = a.pid {
            let deadline = Instant::now() + Duration::from_secs(10);
            let pid_arg = pid.to_string();
            loop {
                let (code, text) = output(Command::new("ps").args(["-p", &pid_arg, "-o", "pid="]))?;
                if code == Some(1) && text.is_empty() {
                    break;
                }
                ensure!(code == Some(0), "could not check Claude session {id} process {pid}: {text}");
                ensure!(Instant::now() < deadline, "Claude session {id} is still running after stop (pid {pid})");
                thread::sleep(Duration::from_millis(100));
            }
        }
        if a.status.is_some() {
            ensure!(
                identity(run, previous, profile, None)?.1.status.is_none(),
                "Claude session {id} is still live after stop"
            );
        }
        // Saved options must be used: passing any launch flags makes Claude fork the session.
        cmd.arg("--resume").arg(session_id).arg(prompt);
    } else {
        let quote = |p: &Path| -> Result<String> {
            Ok(format!("'{}'", p.to_str().context("Claude hook path must be UTF-8")?.replace('\'', "'\\''")))
        };
        let hook = format!("{} claude-stop-hook {} || :", quote(&std::env::current_exe()?)?, quote(dir)?);
        let settings = serde_json::json!({
            "worktree": {"bgIsolation": "none"},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": hook}]}]}
        });
        cmd.arg(prompt).arg("-n").arg(&run.name).arg("--settings").arg(settings.to_string());
        if let Some(m) = &run.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(effort) = &run.requested_effort {
            cmd.arg("--effort").arg(effort);
        }
        cmd.args(&run.extra);
    }
    let report_before = fs::metadata(&report).and_then(|m| m.modified()).ok();
    // Concurrent `claude --bg` launches can SIGKILL a session (seen on 2.1.327, 2026-11-18).
    // Remove this lock once two simultaneous `claude --bg` launches both survive.
    let path = store.root.join("claude-launch.lock");
    let launch_lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
    launch_lock.lock().with_context(|| format!("lock {}", path.display()))?;
    let (code, text) = output(&mut cmd)?;
    let backgrounded = text
        .lines()
        .find(|l| l.contains("backgrounded"))
        .and_then(|l| l.split('·').nth(1))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(Native::Claude { id, .. }) = previous {
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
            if let Err(e) = process::checked(command(profile).current_dir(&run.repo).args(["stop", copy_id])) {
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
        run.launch = Launch::Accepted { id: id.into(), profile: profile.map(Path::to_path_buf), error: None };
        store.save(run)?;
    }
    // A new background session reports idle for a moment before it starts working; give it the launch window.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut confirmed = previous.cloned();
    loop {
        let (native, a) = identity(run, confirmed.as_ref(), profile, None)?;
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
            return Ok(native);
        }
        confirmed = Some(native);
        thread::sleep(Duration::from_secs(1));
    }
}
// Resolve acceptance once; all later observations must match the full recorded identity.
fn identity(
    run: &mut Run,
    previous: Option<&Native>,
    profile: Option<&Path>,
    deadline: Option<Instant>,
) -> Result<(Native, Agent)> {
    let recorded = match (previous, &run.launch) {
        (Some(native), _)
        | (None, Launch::Launched { native, .. } | Launch::PendingResume { previous: native, .. }) => Some(native),
        _ => None,
    };
    let (id, profile, session) = match (recorded, &run.launch) {
        (Some(Native::Claude { id, session_id, profile }), _) => (id, profile, Some(session_id)),
        (_, Launch::Accepted { id, profile, .. }) => (id, profile, None),
        _ => {
            process::query(command(profile).current_dir(&run.repo).args(["agents", "--json", "--all"]), deadline)?;
            bail!(UnknownError(
                UnknownCause::IdentityMismatch,
                "no recorded Claude identity; inspect launch facts before attaching".into()
            ));
        }
    };
    let a = agents(profile.as_deref(), &run.repo, deadline)?.into_iter().find(|a| &a.id == id).ok_or_else(|| {
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
    let native = Native::Claude { id: id.clone(), session_id: a.session_id.clone(), profile: profile.clone() };
    if let Launch::Accepted { error, .. } = &run.launch {
        run.launch = Launch::Launched {
            native: native.clone(),
            intent: error.clone().map_or(Intent::Running, |error| Intent::Unconfirmed { error }),
        };
    }
    Ok((native, a))
}
pub fn query(run: &mut Run, profile: Option<&Path>, deadline: Option<Instant>) -> Result<Observation> {
    let (_, a) = identity(run, None, profile, deadline)?;
    let Agent { id, session_id, status, state, .. } = a;
    let evidence = format!("id={id} session={session_id} status={} state={state}", status.as_deref().unwrap_or("none"));
    let state = match (status.as_deref(), state.as_str()) {
        (_, "failed") => State::Failed,
        (_, "done") => State::Finished,
        (_, "stopped" | "exited") => State::Stopped,
        (_, "blocked" | "waiting_for_permission") => State::Blocked,
        (Some("busy"), _) | (Some("idle") | None, "working") => State::Running,
        _ => return Ok(Observation::unknown(UnknownCause::UnrecognizedStatus, evidence)),
    };
    Ok(Observation::known(state, evidence))
}
