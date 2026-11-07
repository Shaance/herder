use crate::scripts::ScriptExecutor;
use crate::{claude, domain::*, process, scripts, store::LockedRun};
use anyhow::{Result, bail, ensure};
use std::{path::Path, process::Command};

pub fn launch(run: &mut Run, dir: &Path, delta: Option<&Path>, store: &LockedRun) -> Result<()> {
    let target = run.launch.target();
    let outcome = match target {
        Target::Claude { .. } => claude::launch(run, dir, delta, store).map(Some),
        Target::Vibe => ScriptExecutor::VIBE.launch(run, dir, delta),
        Target::Codex => ScriptExecutor::CODEX.launch(run, dir, delta),
    };
    let result = match outcome {
        Ok(Some(native)) => {
            run.attempt = run.launch_attempt();
            run.launch = Launch::Launched { native, intent: Intent::Running };
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => {
            let message = format!("{error:#}");
            let remediation = if matches!(
                run.launch,
                Launch::Accepted { .. } | Launch::Launched { native: Native::Claude { .. }, .. }
            ) {
                "worker accepted; use crew status to observe it; do not relaunch".into()
            } else if message.contains("Workspace not trusted") {
                let profile = match &target {
                    Target::Claude { profile } => {
                        profile.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none (default)".into())
                    }
                    _ => String::new(),
                };
                format!("open `claude` in {} under profile {profile} once and accept trust", run.repo.display())
            } else {
                "inspect the executor message and run crew doctor; correct the cause before retrying".into()
            };
            let error = LaunchError { message: message.clone(), remediation: remediation.clone() };
            match &mut run.launch {
                Launch::Accepted { error: recorded, .. } => *recorded = Some(error),
                Launch::PendingResume { error: recorded, .. } => *recorded = Some(error),
                Launch::Launched { native: Native::Claude { .. }, intent } => *intent = Intent::Unconfirmed { error },
                Launch::Launched { intent, .. } => *intent = Intent::Failed { error },
                _ => run.launch = Launch::Failed { target, error },
            }
            Err(anyhow::anyhow!("run {}: {message}; {remediation}", run.id))
        }
    };
    store.save(run)?;
    result
}
pub fn observe(run: &mut Run, store: Option<&LockedRun>) -> Observation {
    let Some(store) = store else {
        return Observation {
            state: run.launch.state(),
            evidence: "run busy: another crew command holds the lock".into(),
        };
    };
    if let Launch::Failed { error, .. } = &run.launch {
        return Observation { state: State::Failed, evidence: format!("{}; {}", error.message, error.remediation) };
    }
    let result = (|| -> Result<Observation> {
        let dir = &store.dir;
        let mut observed = match run.launch.target() {
            Target::Claude { profile } => {
                if run.launch.native().is_some()
                    || matches!(
                        run.launch,
                        Launch::Accepted { .. } | Launch::PendingResume { previous: Native::Claude { .. }, .. }
                    )
                {
                    let accepted = matches!(run.launch, Launch::Accepted { .. });
                    let observed = claude::query(run)?;
                    if matches!(run.launch, Launch::PendingResume { .. }) {
                        return Ok(observed);
                    }
                    if accepted {
                        store.save(run)?;
                    }
                    observed
                } else {
                    process::query(
                        claude::command(profile.as_deref()).current_dir(&run.repo).args(["agents", "--json", "--all"]),
                    )?;
                    bail!("no recorded Claude identity; inspect launch facts before attaching");
                }
            }
            target @ (Target::Vibe | Target::Codex) => {
                let executor = match target {
                    Target::Vibe => ScriptExecutor::VIBE,
                    _ => ScriptExecutor::CODEX,
                };
                let observed = executor.query(run, dir)?;
                if matches!(run.launch, Launch::Starting { .. } | Launch::PendingResume { .. })
                    && let Ok(native) = executor.identity(dir)
                {
                    if let Launch::PendingResume { previous, .. } = &run.launch {
                        ensure!(
                            previous.process().map(|p| p.2) == native.process().map(|p| p.2),
                            "resume identity changed"
                        );
                        ensure!(
                            previous.process().map(|p| p.0) != native.process().map(|p| p.0),
                            "resume pending; no new pid recorded"
                        );
                    }
                    run.attempt = run.launch_attempt();
                    run.launch = Launch::Launched { native, intent: Intent::Running };
                    store.save(run)?;
                }
                observed
            }
        };
        if matches!(run.launch, Launch::Launched { intent: Intent::StopSent, .. }) {
            if matches!(observed.state, State::Stopped | State::Finished | State::Failed) {
                if let Launch::Launched { intent, .. } = &mut run.launch {
                    *intent = Intent::Stopped;
                }
                store.save(run)?;
            } else {
                return Ok(Observation {
                    state: observed.state,
                    evidence: format!("stop sent; {}", observed.evidence),
                });
            }
        }
        if matches!(run.launch, Launch::Launched { intent: Intent::Stopped, .. }) {
            observed.evidence = format!("stopped by crew stop; {}", observed.evidence);
        }
        Ok(reconcile(run.launch.state(), observed))
    })();
    result.unwrap_or_else(|e| Observation {
        state: State::Unknown,
        evidence: format!("recorded={:?}; executor query failed: {e:#}", run.launch.state()),
    })
}
pub fn stop(run: &mut Run, store: &LockedRun) -> Result<Observation> {
    let observed = observe(run, Some(store));
    ensure!(matches!(observed.state, State::Running | State::Blocked), "cannot stop: {observed}");
    let dir = &store.dir;
    let native = match &run.launch {
        Launch::PendingResume { previous: native @ Native::Claude { .. }, .. } => native,
        _ => run.launch.native().ok_or_else(|| anyhow::anyhow!("no recorded identity"))?,
    };
    match native {
        Native::Claude { id, profile, .. } => {
            process::checked(claude::command(profile.as_deref()).current_dir(&run.repo).args(["stop", id]))?;
        }
        _ => scripts::stop(native, run, dir)?,
    }
    if let Launch::Launched { intent, .. } = &mut run.launch {
        *intent = Intent::StopSent;
    }
    store.save(run)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let observed = observe(run, Some(store));
        if observed.state == State::Stopped {
            return Ok(observed);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(Observation {
                state: observed.state,
                evidence: format!("stop sent; exit not confirmed: {}", observed.evidence),
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
pub fn resume(run: &mut Run, delta: &Path, request_id: Option<&str>, store: &LockedRun) -> Result<()> {
    if let Some(id) = request_id {
        ensure!(
            !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control),
            "request id must be 1-200 bytes without control characters"
        );
    }
    let contents = std::fs::read(delta)?;
    if let Some(attempt) = request_id.and_then(|id| run.request_attempts.get(id).copied()) {
        // Known request: a replay returns the recorded run without another launch.
        let input = store.dir.join(format!("input{attempt}.md"));
        ensure!(
            std::fs::read(&input).ok().as_deref() == Some(contents.as_slice()),
            "request id {request_id:?} already owns attempt {attempt} with different brief bytes"
        );
        return Ok(());
    }
    ensure!(!contents.is_empty(), "delta brief missing or empty");
    let dir = &store.dir;
    let (native, attempt) = if let Launch::PendingResume { previous, attempt, error } = &run.launch
        && (error.is_some() || matches!(previous, Native::Claude { .. }))
    {
        let (previous, attempt) = (previous.clone(), *attempt);
        if request_id.is_some() && run.request_attempts.values().any(|a| *a == attempt) {
            bail!(
                "pending attempt {attempt} is reserved by another request id; resolve it with status first, or retry without --request-id to continue the same attempt"
            );
        }
        let executor = match &previous {
            Native::Claude { .. } => None,
            Native::Vibe { .. } => Some(ScriptExecutor::VIBE),
            Native::Codex { .. } => Some(ScriptExecutor::CODEX),
        };
        let observed = match &executor {
            Some(executor) => executor.query(run, dir)?,
            None => claude::query(run)?,
        };
        ensure!(
            matches!(observed.state, State::Finished | State::Failed | State::Stopped),
            "cannot retry pending resume: {observed}"
        );
        if let Some(executor) = executor {
            let recorded = executor.identity(dir)?;
            ensure!(recorded.process().map(|p| p.2) == previous.process().map(|p| p.2), "resume identity changed");
        }
        (previous, attempt)
    } else {
        let observed = observe(run, Some(store));
        ensure!(
            matches!(observed.state, State::Finished | State::Failed | State::Stopped),
            "cannot resume: {observed}"
        );
        let native = run.launch.native().ok_or_else(|| anyhow::anyhow!("no recorded identity to resume"))?;
        if !matches!(native, Native::Claude { .. }) {
            scripts::confirm(native, dir)?;
        }
        (native.clone(), run.attempt.checked_add(1).ok_or_else(|| anyhow::anyhow!("attempt overflow"))?)
    };
    let delta = dir.join(format!("input{attempt}.md"));
    if run.request_attempts.values().any(|a| *a == attempt) {
        ensure!(
            std::fs::read(&delta).ok().as_deref() == Some(contents.as_slice()),
            "input for attempt {attempt} is reserved by a request id with different content"
        );
    }
    crate::store::atomic(&delta, &contents)?;
    if let Some(id) = request_id {
        run.request_attempts.insert(id.to_owned(), attempt);
    }
    run.launch = Launch::PendingResume { previous: native, attempt, error: None };
    store.save(run)?;
    launch(run, dir, Some(&delta), store)
}
pub fn pane(run: &Run) -> Result<()> {
    let block = std::env::var("REX_BLOCK")?;
    let mut cmd = Command::new("rex");
    cmd.arg("split");
    if let Ok(session) = std::env::var("REX_SESSION") {
        cmd.args(["-s", &session]);
    }
    cmd.args(["-b", &block, "--focus=false", "--shell=none", "--"]);
    match run.launch.native().ok_or_else(|| anyhow::anyhow!("pane requires native identity"))? {
        Native::Claude { id, profile, .. } => {
            cmd.arg("env");
            if let Some(p) = profile {
                cmd.arg(format!("CLAUDE_CONFIG_DIR={}", p.display()));
            } else {
                cmd.args(["-u", "CLAUDE_CONFIG_DIR"]);
            }
            cmd.args(["claude", "attach", id]);
        }
        Native::Vibe { .. } | Native::Codex { .. } => {
            cmd.arg("env");
            if let Some(root) = std::env::var_os("CREW_STATE_DIR") {
                let mut assignment = std::ffi::OsString::from("CREW_STATE_DIR=");
                assignment.push(std::path::absolute(Path::new(&root))?);
                cmd.arg(assignment);
            } else {
                cmd.args(["-u", "CREW_STATE_DIR"]);
            }
            cmd.arg(std::env::current_exe()?).arg("wait").arg(&run.id);
        }
    }
    process::checked(&mut cmd)?;
    Ok(())
}
