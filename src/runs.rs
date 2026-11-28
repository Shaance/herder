use crate::{claude, domain::*, process, scripts, store::LockedRun};
use crate::{process::ScriptSource, scripts::ScriptExecutor};
use anyhow::{Result, bail, ensure};
use std::{path::Path, process::Command, time::Instant};

pub fn launch(run: &mut Run, phase: LaunchPhase<'_>, store: &LockedRun) -> Result<()> {
    let target = run.launch.target();
    let outcome = if let Some(executor) = ScriptExecutor::for_executor(target.executor()) {
        executor.launch(run, &store.dir, phase)
    } else {
        claude::launch(run, target.profile(), phase, store).map(Some)
    };
    let result = match outcome {
        Ok(Some(native)) => {
            run.confirm_identity(native);
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => {
            let message = format!("{error:#}");
            let remediation = if matches!(run.launch, Launch::Accepted { .. } | Launch::Launched { .. }) {
                "worker accepted; use herder status to observe it; do not relaunch".into()
            } else if let Target::Claude { profile } = &target
                && message.contains("Workspace not trusted")
            {
                let profile =
                    profile.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none (default)".into());
                format!("open `claude` in {} under profile {profile} once and accept trust", run.repo.display())
            } else {
                "inspect the executor message and run herder doctor; correct the cause before retrying".into()
            };
            let error = LaunchError { message: message.clone(), remediation: remediation.clone() };
            match &mut run.launch {
                Launch::Accepted { error: recorded, .. } | Launch::PendingResume { error: recorded, .. } => {
                    *recorded = Some(error)
                }
                Launch::Launched { intent, .. } => *intent = Intent::Unconfirmed { error },
                _ => run.launch = Launch::Failed { target, error },
            }
            Err(anyhow::anyhow!("run {}: {message}; {remediation}", run.id))
        }
    };
    store.save(run)?;
    result
}
pub fn observe(run: &mut Run, store: Option<&LockedRun>) -> Observation {
    observe_until(run, store, None, ScriptSource::Configured)
}

pub fn observe_until(
    run: &mut Run,
    store: Option<&LockedRun>,
    deadline: Option<Instant>,
    source: ScriptSource,
) -> Observation {
    let Some(store) = store else {
        let evidence = "run busy: another herder command holds the lock".into();
        return match run.launch.state() {
            State::Unknown => Observation::unknown(UnknownCause::Unconfirmed, evidence),
            state => Observation::known(state, evidence),
        };
    };
    if let Launch::Failed { error, .. } = &run.launch {
        return Observation::known(State::Failed, format!("{}; {}", error.message, error.remediation));
    }
    let result = (|| -> Result<Observation> {
        let dir = &store.dir;
        let target = run.launch.target();
        let mut observed = match ScriptExecutor::for_executor(target.executor()) {
            None => {
                let (observed, native) = claude::query(run, target.profile(), deadline)?;
                if matches!(run.launch, Launch::PendingResume { .. }) {
                    return Ok(observed);
                }
                if matches!(run.launch, Launch::Accepted { .. }) {
                    run.confirm_identity(native);
                    store.save(run)?;
                }
                observed
            }
            Some(executor) => {
                let (observed, identity) = executor.query(run, dir, deadline, source)?;
                let identity = match &run.launch {
                    Launch::PendingResume { previous: Native::Codex { .. }, .. } => Some(identity?),
                    _ => identity.ok(),
                };
                if matches!(run.launch, Launch::Starting { .. } | Launch::PendingResume { .. })
                    && let Some(native) = identity
                {
                    if let Launch::PendingResume { previous, .. } = &run.launch {
                        let (pid, _, id) = native.process().unwrap();
                        let (previous_pid, _, previous_id) = previous.process().unwrap();
                        let started = match previous {
                            Native::Codex { thread_id, .. } => scripts::codex_resume_started(run, dir, thread_id, pid)?,
                            _ => true,
                        };
                        ensure!(
                            previous_id == id,
                            UnknownError(UnknownCause::IdentityMismatch, "resume identity changed".into())
                        );
                        ensure!(
                            previous_pid != pid,
                            UnknownError(UnknownCause::Unconfirmed, "resume pending; no new pid recorded".into())
                        );
                        if let Native::Vibe { session_id, .. } = previous {
                            scripts::check_vibe_resume_session(run, dir, session_id, pid)?;
                        }
                        if !started && observed.state() != State::Failed {
                            let evidence =
                                format!("resume pending; waiting for thread.started: {}", observed.evidence());
                            return Ok(if observed.state() == State::Running {
                                Observation::known(State::Starting, evidence)
                            } else {
                                Observation::unknown(UnknownCause::Unconfirmed, evidence)
                            });
                        }
                    }
                    run.confirm_identity(native);
                    store.save(run)?;
                }
                observed
            }
        };
        if let Launch::Launched { intent: intent @ Intent::StopSent, .. } = &mut run.launch {
            if matches!(observed.state(), State::Stopped | State::Finished | State::Failed) {
                *intent = Intent::Stopped;
                store.save(run)?;
            } else {
                observed.prefix_evidence("stop sent; ");
                return Ok(observed);
            }
        }
        if matches!(run.launch, Launch::Launched { intent: Intent::Stopped, .. }) {
            observed.prefix_evidence("stopped by herder stop; ");
        }
        Ok(reconcile(run.launch.state(), observed))
    })();
    result.unwrap_or_else(|e| {
        Observation::unknown(
            e.downcast_ref::<UnknownError>().map_or(UnknownCause::QueryFailed, |error| error.0),
            format!("recorded={:?}; executor query failed: {e:#}", run.launch.state()),
        )
    })
}
pub fn stop(run: &mut Run, store: &LockedRun) -> Result<Observation> {
    let observed = observe(run, Some(store));
    ensure!(matches!(observed.state(), State::Running | State::Blocked), "cannot stop: {observed}");
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
        let mut observed = observe(run, Some(store));
        if observed.state() == State::Stopped {
            return Ok(observed);
        }
        if std::time::Instant::now() >= deadline {
            observed.prefix_evidence("stop sent; exit not confirmed: ");
            return Ok(observed);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
pub fn resume(run: &mut Run, delta: &Path, request_id: Option<&str>, store: &LockedRun) -> Result<Observation> {
    ensure!(run.launch.target().executor() != Executor::Agy, "agy does not support resume; dispatch a new run");
    if let Some(id) = request_id {
        ensure!(
            !id.is_empty() && id.len() <= 200 && !id.chars().any(char::is_control),
            "request id must be 1-200 bytes without control characters"
        );
    }
    let replay_attempt = request_id.and_then(|id| run.request_attempts.get(id).copied());
    ensure!(
        replay_attempt.is_some() || run.accept_with.is_none(),
        "dispatch a fresh keyed work order to continue a protected run"
    );
    let contents = std::fs::read(delta)?;
    if let Some(attempt) = replay_attempt {
        // Known request: a replay returns the recorded run without another launch.
        let input = store.dir.join(format!("input{attempt}.md"));
        ensure!(
            std::fs::read(&input).ok().as_deref() == Some(contents.as_slice()),
            "request id {request_id:?} already owns attempt {attempt} with different brief bytes"
        );
        return Ok(observe(run, Some(store)));
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
        let target = previous.target();
        let (observed, identity) = match ScriptExecutor::for_executor(target.executor()) {
            None => (claude::query(run, target.profile(), None)?.0, None),
            Some(executor) => {
                let (observed, identity) = executor.query(run, dir, None, ScriptSource::Configured)?;
                (observed, Some(identity))
            }
        };
        ensure!(
            matches!(observed.state(), State::Finished | State::Failed | State::Stopped),
            "cannot retry pending resume: {observed}"
        );
        if let Some(identity) = identity {
            let recorded = identity?;
            ensure!(
                recorded.process().map(|p| p.2) == previous.process().map(|p| p.2),
                UnknownError(UnknownCause::IdentityMismatch, "resume identity changed".into())
            );
            match &previous {
                Native::Codex { thread_id, .. } => {
                    scripts::codex_resume_started(run, dir, thread_id, recorded.process().unwrap().0)?;
                }
                Native::Vibe { session_id, .. } => {
                    scripts::check_vibe_resume_session(run, dir, session_id, recorded.process().unwrap().0)?;
                }
                Native::Claude { .. } | Native::Agy { .. } => {}
            }
        }
        (previous, attempt)
    } else {
        let observed = observe(run, Some(store));
        ensure!(
            matches!(observed.state(), State::Finished | State::Failed | State::Stopped),
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
    run.launch = Launch::PendingResume { previous: native.clone(), attempt, error: None };
    run.resume_worker_dir = native.process().map(|_| dir.to_path_buf());
    store.save(run)?;
    launch(run, LaunchPhase::Resume { previous: &native, delta: &delta }, store)?;
    let observed = observe(run, Some(store));
    // The stream can gain its opening event between the launch gate and observation.
    if matches!(native, Native::Codex { .. })
        && matches!(run.launch, Launch::PendingResume { .. })
        && matches!(observed.cause(), Some(UnknownCause::IdentityMismatch))
    {
        return Err(UnknownError(UnknownCause::IdentityMismatch, observed.evidence().into()).into());
    }
    Ok(observed)
}
pub fn pane(run: &Run, dir: &Path) -> Result<()> {
    let block = std::env::var("REX_BLOCK")?;
    let mut cmd = Command::new("rex");
    cmd.arg("split");
    if let Ok(session) = std::env::var("REX_SESSION") {
        cmd.args(["-s", &session]);
    }
    cmd.args(["-b", &block, "--focus=false", "--shell=none", "--"]);
    match run.launch.target() {
        Target::Claude { .. } => {
            let Some(Native::Claude { id, profile, .. }) = run.launch.native() else {
                bail!("pane requires native identity");
            };
            cmd.arg("env");
            if let Some(p) = profile {
                cmd.arg(format!("CLAUDE_CONFIG_DIR={}", p.display()));
            } else {
                cmd.args(["-u", "CLAUDE_CONFIG_DIR"]);
            }
            cmd.args(["claude", "attach", id]);
        }
        Target::Vibe | Target::Codex | Target::Agy => {
            let exe = std::env::current_exe()?;
            if let Some(skills) = std::env::var_os("HERDER_SKILLS_DIR") {
                cmd.args(["env", "-u", "HERDER_STATE_DIR"]);
                if let Some(root) = std::env::var_os("HERDER_STATE_DIR") {
                    let mut assignment = std::ffi::OsString::from("HERDER_STATE_DIR=");
                    assignment.push(std::path::absolute(Path::new(&root))?);
                    cmd.arg(assignment);
                }
                let mut assignment = std::ffi::OsString::from("HERDER_SKILLS_DIR=");
                assignment.push(std::path::absolute(Path::new(&skills))?);
                cmd.arg(assignment).arg(&exe).arg("wait").arg(&run.id);
            } else {
                let canonical = dir.canonicalize()?;
                ensure!(canonical.file_name() == dir.file_name(), "pane run path changed identity");
                cmd.arg(&exe).args(["wait", "--run-dir"]).arg(canonical);
            }
        }
    }
    process::checked(&mut cmd)?;
    Ok(())
}
