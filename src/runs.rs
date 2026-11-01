use crate::scripts::ScriptExecutor;
use crate::{claude, domain::*, process, scripts, store::Store};
use anyhow::{Result, bail, ensure};
use std::{path::Path, process::Command};

pub fn launch(run: &mut Run, dir: &Path, delta: Option<&Path>, extra: &[String], store: &Store) -> Result<()> {
    let target = run.launch.target();
    let outcome = match target {
        Target::Claude { .. } => claude::launch(run, dir, extra).map(Some),
        Target::Vibe => ScriptExecutor::VIBE.launch(run, dir, delta, extra),
        Target::Codex => ScriptExecutor::CODEX.launch(run, dir, delta, extra),
    };
    let result = match outcome {
        Ok(Some(native)) => {
            run.launch = Launch::Launched { native, intent: Intent::Running };
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => {
            let message = format!("{error:#}");
            let remediation = if message.contains("Workspace not trusted") {
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
            if let Launch::Launched { intent, .. } = &mut run.launch {
                *intent = Intent::Failed { error };
            } else {
                run.launch = Launch::Failed { target, error };
            }
            Err(anyhow::anyhow!("run {}: {message}; {remediation}", run.id))
        }
    };
    store.save(run)?;
    result
}
pub fn observe(run: &mut Run, store: &Store) -> Observation {
    let result = (|| -> Result<Observation> {
        let dir = store.dir(&run.id)?;
        let observed = match run.launch.target() {
            Target::Claude { profile } => {
                if let Some(native) = run.launch.native() {
                    claude::query(native, run, &dir)?
                } else {
                    process::checked(
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
                let observed = executor.query(run, &dir)?;
                if matches!(run.launch, Launch::Starting { .. }) {
                    if let Ok(native) = executor.identity(&dir) {
                        run.launch = Launch::Launched { native, intent: Intent::Running };
                        store.save(run)?;
                    }
                }
                observed
            }
        };
        Ok(reconcile(run.launch.state(), observed))
    })();
    result.unwrap_or_else(|e| Observation {
        state: State::Unknown,
        evidence: format!("recorded={:?}; executor query failed: {e:#}", run.launch.state()),
    })
}
pub fn stop(run: &mut Run, store: &Store) -> Result<Observation> {
    let observed = observe(run, store);
    ensure!(matches!(observed.state, State::Running | State::Blocked), "cannot stop: {observed:?}");
    let dir = store.dir(&run.id)?;
    let native = run.launch.native().ok_or_else(|| anyhow::anyhow!("no recorded identity"))?;
    match native {
        Native::Claude { id, profile, .. } => {
            process::checked(claude::command(profile.as_deref()).current_dir(&run.repo).args(["stop", id]))?;
        }
        _ => scripts::stop(native, run, &dir)?,
    }
    if let Launch::Launched { intent, .. } = &mut run.launch {
        *intent = Intent::Stopped;
    }
    store.save(run)?;
    Ok(observe(run, store))
}
pub fn resume(run: &mut Run, delta: &Path, store: &Store) -> Result<()> {
    if let Some(Native::Claude { id, .. }) = run.launch.native() {
        bail!(
            "Claude has no live background follow-up command; use `claude attach {id}` under the run's recorded profile"
        );
    }
    let observed = observe(run, store);
    ensure!(matches!(observed.state, State::Finished | State::Failed | State::Stopped), "cannot resume: {observed:?}");
    let native = run.launch.native().ok_or_else(|| anyhow::anyhow!("no recorded identity to resume"))?;
    let dir = store.dir(&run.id)?;
    scripts::confirm(native, &dir)?;
    let attempt = run.attempt.checked_add(1).ok_or_else(|| anyhow::anyhow!("attempt overflow"))?;
    let delta = crate::store::copy(delta, &dir.join(format!("input{attempt}.md")))?;
    run.attempt = attempt;
    if let Launch::Launched { intent, .. } = &mut run.launch {
        *intent = Intent::Starting;
    }
    store.save(run)?;
    launch(run, &dir, Some(&delta), &[], store)
}
pub fn pane(run: &Run, dir: &Path) -> Result<()> {
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
        native @ (Native::Vibe { .. } | Native::Codex { .. }) => {
            let executor = match native {
                Native::Vibe { .. } => ScriptExecutor::VIBE,
                _ => ScriptExecutor::CODEX,
            };
            // watch owns the display loop; no shell command strings are constructed.
            cmd.args(["watch", "-n", "2", "-x"]).arg(executor.path("status")).arg(dir).args(executor.status_args(run));
        }
    }
    process::checked(&mut cmd)?;
    Ok(())
}
