use crate::{
    domain::{Check, ProtectedVerification, Run, State, Verdict},
    process, progress, runs,
    store::{LockedRun, ScopedNotFound, Store, timestamp},
    verify,
    writer_slot::Slot,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fmt, fs, io,
    num::NonZeroU32,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub fn host_check(repo: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "--accept-with must be an absolute executable path");
    let path = path.canonicalize().context("resolve the host check")?;
    ensure!(
        !path.starts_with(repo) && process::executable(&path),
        "the host check must be executable and outside the checkout"
    );
    Ok(path)
}

pub fn require_external_state(repo: &Path, state_dir: &Path) -> Result<()> {
    ensure!(!resolve_path(state_dir)?.starts_with(repo), "the Herder state directory must be outside the checkout");
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DispatchDenied {
    CheckoutBusy { run_id: Option<String>, message: &'static str },
    CheckoutMismatch { reason: CheckoutMismatch },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutMismatch {
    Branch,
    Head,
}

impl fmt::Display for DispatchDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CheckoutBusy { message, .. } => message,
            Self::CheckoutMismatch { reason } => match reason {
                CheckoutMismatch::Branch => {
                    "the checkout branch differs from --expected-branch; inspect it with herder context"
                }
                CheckoutMismatch::Head => {
                    "the checkout HEAD differs from --expected-head; inspect it with herder context"
                }
            },
        })
    }
}

impl std::error::Error for DispatchDenied {}

pub fn check_pins(repo: &Path, store: &Store, head: Option<&str>, branch: Option<&str>) -> Result<()> {
    if head.is_some() || branch.is_some() {
        let current = crate::context::read(repo, &store.root)?;
        ensure!(
            branch.is_none_or(|branch| current.branch.as_deref() == Some(branch)),
            DispatchDenied::CheckoutMismatch { reason: CheckoutMismatch::Branch }
        );
        ensure!(
            head.is_none_or(|head| current.head.as_deref() == Some(head)),
            DispatchDenied::CheckoutMismatch { reason: CheckoutMismatch::Head }
        );
    }
    Ok(())
}

pub fn require_available(slot: &Slot, store: &Store, repo: &Path) -> Result<()> {
    let Some(owner) = slot.owner()? else { return Ok(()) };
    let unresolved = || DispatchDenied::CheckoutBusy {
        run_id: None,
        message: "the checkout has an unresolved protected work order",
    };
    let id = store.observation_id(&owner, true)?;
    let previous = match store.read_for_observation(&id, Some(repo), None) {
        Ok(Some(previous)) => previous,
        Ok(None) => return Err(unresolved().into()),
        Err(error) if error.is::<ScopedNotFound>() => return Err(unresolved().into()),
        Err(error) => return Err(error),
    };
    let (lock, mut run) = previous;
    if run.protected_verification.is_some() {
        return Ok(());
    }
    let mut message = "wait for the active protected work order to finish";
    if let Some(lock) = &lock {
        let deadline = Instant::now() + Duration::from_secs(2);
        let state =
            runs::observe_until(&mut run, Some(lock), Some(deadline), process::ScriptSource::Configured).state();
        if protected_completion(&run, state, Some(lock)).is_ok() {
            message = "verify the finished protected work order before starting another";
        } else if matches!(state, State::Failed | State::Stopped | State::Finished) {
            return Ok(());
        }
    }
    Err(DispatchDenied::CheckoutBusy { run_id: Some(run.id), message }.into())
}

fn relative_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && Path::new(prefix).components().all(|component| match component {
            Component::Normal(name) => name != ".git",
            Component::CurDir => true,
            _ => false,
        })
}

pub fn require_clean(repo: &Path, paths: &[String]) -> Result<()> {
    ensure!(!paths.is_empty(), "protected dispatch requires explicit --path values");
    for prefix in paths {
        let path = Path::new(prefix);
        ensure!(relative_prefix(prefix), "protected dispatch requires non-escaping relative --path values");
        ensure!(resolve_path(&repo.join(path))?.starts_with(repo), "an authorized path resolves outside the checkout");
    }
    ensure!(verify::dirty_paths(repo, None)?.is_empty(), "protected dispatch requires a clean Git checkout");
    Ok(())
}

// Resolve links in existing parents even when the final path does not exist yet.
fn resolve_path(path: &Path) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        if component == Component::ParentDir {
            resolved.pop();
            continue;
        }
        resolved.push(component);
        match fs::symlink_metadata(&resolved) {
            Ok(_) => resolved = resolved.canonicalize()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("resolve protected path"),
        }
    }
    Ok(resolved)
}

#[derive(Serialize)]
pub struct Verification {
    pub outcome: &'static str,
    pub run_id: String,
    pub attempt: NonZeroU32,
    pub state: State,
    pub message: Option<&'static str>,
    pub scope: Option<Check>,
    pub host_check: Option<Check>,
    pub verdict: Option<Verdict>,
    pub verified_at: Option<String>,
}

pub fn verify(
    mut run: Run,
    lock: Option<LockedRun>,
    supplied_check: &Path,
    allowed_paths: &[String],
    slot: Slot,
) -> Result<(Verification, i32)> {
    let mut result = Verification {
        outcome: "not_eligible",
        run_id: run.id.clone(),
        attempt: run.launch_attempt(),
        state: run.launch.state(),
        message: Some("protected verification requires the configured check and permission for all recorded paths"),
        scope: None,
        host_check: None,
        verdict: None,
        verified_at: None,
    };
    let Some(configured_check) = &run.accept_with else {
        return Ok((result, 1));
    };
    if allowed_paths.is_empty()
        || !allowed_paths.iter().all(|path| relative_prefix(path))
        || !run
            .authorized_paths
            .iter()
            .all(|path| allowed_paths.iter().any(|prefix| verify::under_prefix(path, prefix)))
    {
        return Ok((result, 1));
    }
    let check = if supplied_check == configured_check && run.protected_verification.is_some() {
        configured_check.clone()
    } else {
        host_check(&run.repo, supplied_check)?
    };
    if &check != configured_check {
        return Ok((result, 1));
    }
    if let Some(saved) = run.protected_verification.take() {
        return Ok(verified(result, saved));
    }
    if slot.owner()?.as_deref() != Some(&run.id) {
        result.outcome = "superseded";
        result.message = Some("the checkout no longer belongs to this protected work order");
        return Ok((result, 0));
    }
    drop(slot);

    let deadline = Instant::now() + Duration::from_secs(30);
    let observed = runs::observe_until(&mut run, lock.as_ref(), Some(deadline), process::ScriptSource::Configured);
    result.state = observed.state();
    result.attempt = run.launch_attempt();
    let lock = match protected_completion(&run, result.state, lock.as_ref()) {
        Ok(lock) => lock,
        Err((outcome, message, code)) => {
            result.outcome = outcome;
            result.message = message;
            return Ok((result, code));
        }
    };

    let mut scope = verify::scope_check(&run, &run.authorized_paths, None, Some(deadline));
    let host = (scope.exit_code == Some(0)).then(|| {
        let command = check.display().to_string();
        let (exit_code, output) = match process::bounded_output(Command::new(check).current_dir(&run.repo), deadline) {
            Ok((Some(status), stdout, stderr)) => (status.code(), stdout + &stderr),
            Ok((None, _, _)) => (None, "host check timed out".into()),
            Err(error) => (None, format!("{error:#}")),
        };
        Check { command, exit_code, output }
    });
    let host_passed = host.as_ref().is_some_and(|check| check.exit_code == Some(0));
    if host_passed {
        scope = verify::scope_check(&run, &run.authorized_paths, None, Some(deadline));
    }
    let passed = host_passed && scope.exit_code == Some(0);
    let verdict = if passed { Verdict::Pass } else { Verdict::Fail };
    let receipt =
        ProtectedVerification { attempt: result.attempt, timestamp: timestamp(), scope, host_check: host, verdict };
    run.protected_verification = Some(receipt.clone());
    lock.save(&run)?;
    Ok(verified(result, receipt))
}

fn protected_completion<'a>(
    run: &Run,
    state: State,
    lock: Option<&'a LockedRun>,
) -> Result<&'a LockedRun, (&'static str, Option<&'static str>, i32)> {
    if run.launch_attempt() != NonZeroU32::MIN {
        return Err(("superseded", Some("the protected attempt was superseded"), 0));
    }
    let lock = lock.ok_or(("pending", Some("another Herder command holds the run lock"), 0))?;
    match state {
        State::Starting | State::Running | State::Unknown => Err(("pending", None, 0)),
        State::Finished if progress::codex_turn_completed(&lock.dir.join(run.launch_events_name())) => Ok(lock),
        State::Finished => Err(("unverified", Some("the protected attempt has no completed Codex turn"), 1)),
        _ => Err(("unverified", Some("the worker did not finish successfully"), 1)),
    }
}

fn verified(mut result: Verification, saved: ProtectedVerification) -> (Verification, i32) {
    result.outcome = "verified";
    result.message = None;
    result.state = State::Finished;
    result.attempt = saved.attempt;
    result.scope = Some(saved.scope);
    result.host_check = saved.host_check;
    result.verdict = Some(saved.verdict);
    result.verified_at = Some(saved.timestamp);
    (result, i32::from(matches!(saved.verdict, Verdict::Fail)))
}
