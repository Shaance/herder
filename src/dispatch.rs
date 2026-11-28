mod args;

use self::args::Models;
use crate::{
    DispatchArgs,
    dispatch_request::DispatchRequest,
    domain::*,
    output::{CheckedLaunchVerification, LaunchOutcome, Launched, Output},
    process::{self, ScriptSource},
    runs,
    store::{self, LockedRun, RunId, Store},
    writer_slot::Slot,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Write},
    num::NonZeroU32,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, Instant},
};

fn lint_snapshot(bytes: &[u8], mode: &str, repo: &Path, state: &Path) -> Result<()> {
    let state = state.canonicalize().unwrap_or_else(|_| state.to_owned());
    let snapshot = [std::env::temp_dir(), "/tmp".into()]
        .into_iter()
        .filter_map(|dir| dir.canonicalize().ok())
        .filter(|dir| !dir.starts_with(repo) && !dir.starts_with(&state))
        .find_map(|dir| {
            let mut file = tempfile::NamedTempFile::new_in(dir).ok()?;
            file.write_all(bytes).ok()?;
            Some(file)
        })
        .context("write brief lint snapshot outside the checkout and state directory")?;
    process::lint(snapshot.path(), mode)?;
    if let Err(error) = snapshot.close()
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error).context("remove brief lint snapshot");
    }
    Ok(())
}

pub(crate) fn dispatch(
    mut args: DispatchArgs,
    store: &Store,
    handoff: bool,
    checks: Vec<String>,
    check_timeout: Option<u32>,
) -> Result<(Output, i32)> {
    let reject = |message: String| Ok((Output::Rejection(message), 2));
    let (extra, models) = match args::validate(&mut args, handoff, &checks) {
        Ok(validated) => validated,
        Err(message) => return reject(message),
    };
    let DispatchArgs {
        mode,
        executor,
        brief,
        repo,
        effort,
        goal,
        name,
        profile,
        pane,
        paths,
        request_id,
        accept_with,
        expected_head,
        expected_branch,
        ..
    } = args;
    let repo = store::repo(&repo)?;
    let resolve_target = || -> Result<Target> {
        Ok(match executor {
            Executor::Claude => Target::Claude { profile: process::profile(profile)? },
            Executor::Vibe => Target::Vibe,
            Executor::Codex => Target::Codex,
            Executor::Agy => Target::Agy,
        })
    };
    let read_unkeyed = || -> Result<Vec<u8>> {
        let nonempty_file = |m: fs::Metadata| m.is_file() && m.len() > 0;
        let mut failure = None;
        if fs::metadata(&brief).is_ok_and(nonempty_file) {
            // The path can be replaced between the metadata check and open.
            let read = || -> std::io::Result<Option<Vec<u8>>> {
                let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&brief)?;
                let mut bytes = Vec::new();
                Ok((file.metadata().is_ok_and(nonempty_file) && file.read_to_end(&mut bytes)? > 0).then_some(bytes))
            };
            match read() {
                Ok(Some(bytes)) => return Ok(bytes),
                Ok(None) => {}
                Err(error) => failure = Some(error),
            }
        }
        // Let the linter describe a brief that cannot be read, as before the snapshot.
        process::lint(&brief, &mode)?;
        match failure {
            Some(error) => Err(error).with_context(|| format!("read dispatch brief {}", brief.display())),
            None => anyhow::bail!("brief '{}' must be a regular, nonempty file", brief.display()),
        }
    };
    let stage = |bytes: &[u8], protected: bool| -> Result<_> {
        let baseline_commit = if !protected {
            process::checked(std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "HEAD"]))
                .ok()
                .map(|s| s.trim().to_owned())
        } else {
            None
        };
        let mut dispatch_dirty = std::collections::BTreeMap::new();
        let dispatch_staged = if let Some(head) = &baseline_commit {
            for path in crate::verify::dirty_paths(&repo, None)? {
                let hash = crate::verify::content_hash(&repo, &path, None)?;
                dispatch_dirty.insert(path, hash);
            }
            Some(crate::verify::staged_states(&repo, head, dispatch_dirty.keys(), None)?)
        } else {
            None
        };
        let (id, locked) = store.create()?;
        store::atomic(&locked.dir.join("input.md"), bytes)?;
        Ok((id, locked, (baseline_commit, dispatch_dirty, dispatch_staged)))
    };
    let new_run =
        |id: &RunId, name: Option<String>, model: Option<String>, target: &Target, dispatch_snapshot, accept_with| {
            let (baseline_commit, dispatch_dirty, dispatch_staged) = dispatch_snapshot;
            Run {
                name: name.unwrap_or_else(|| id.to_string()),
                id: id.to_string(),
                model,
                requested_effort: effort.clone(),
                extra: extra.clone(),
                repo: repo.clone(),
                baseline_commit,
                dispatch_dirty,
                dispatch_staged,
                brief: store.dir(id).join("input.md"),
                goal: goal.clone(),
                launch: Launch::Starting { target: target.clone() },
                attempt: NonZeroU32::MIN,
                resume_worker_dir: None,
                request_attempts: Default::default(),
                authorized_paths: paths.clone(),
                accept_with,
                protected_verification: None,
                verifications: vec![],
                created_at: store::timestamp(),
            }
        };
    let launch = |mut run: Run, locked: &LockedRun| {
        let outcome = match runs::launch(&mut run, LaunchPhase::Initial, locked) {
            Err(error) => LaunchOutcome::LaunchFailed(format!("{error:#}")),
            Ok(()) if pane => match runs::pane(&run, &locked.dir) {
                Ok(()) => LaunchOutcome::Started,
                Err(error) => LaunchOutcome::PaneFailed(format!("worker recorded; pane failed: {error:#}")),
            },
            Ok(()) => LaunchOutcome::Started,
        };
        let code = i32::from(!matches!(outcome, LaunchOutcome::Started));
        (Launched { run: Box::new(run), outcome }, code)
    };
    let models = match models {
        Models::Single(model) => {
            if accept_with.is_some() {
                crate::guard::require_external_state(&repo, &store.root)?;
            }
            let target = resolve_target()?;
            let bytes = if handoff {
                // Validate the continuation document before any staging or launch side effect.
                let document = match crate::handoff::document(&brief) {
                    Ok(document) => document,
                    Err(error) => return reject(format!("{error:#}")),
                };
                crate::handoff::prompt(&mode, &document).into_bytes()
            } else if request_id.is_some() {
                fs::read(&brief).context("read dispatch brief")?
            } else {
                read_unkeyed()?
            };
            let request = if let Some(key) = &request_id {
                let payload = DispatchPayload {
                    mode: &mode,
                    target: &target,
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                    goal: goal.as_deref(),
                    name: name.as_deref(),
                    extra: &extra,
                    paths: &paths,
                    accept_with: accept_with.as_deref(),
                    expected_head: expected_head.as_deref(),
                    expected_branch: expected_branch.as_deref(),
                    brief: &bytes,
                };
                let request = DispatchRequest::new(store, &repo, key, &payload)?;
                if let Some(run_id) = request.existing()? {
                    return Ok((Output::Replayed { run_id }, 0));
                }
                Some(request)
            } else {
                None
            };
            let accept_with = accept_with.as_deref().map(|path| crate::guard::host_check(&repo, path)).transpose()?;
            lint_snapshot(&bytes, &mode, &repo, &store.root)?;
            let (id, mut locked, (mut baseline_commit, dispatch_dirty, dispatch_staged)) =
                stage(&bytes, accept_with.is_some())?;
            let slot = accept_with.is_some().then(|| Slot::lock(store, &repo)).transpose()?;
            if let Some(request) = &request
                && let Some(run_id) = request.existing()?
            {
                return Ok((Output::Replayed { run_id }, 0));
            }
            if let Some(slot) = &slot {
                crate::guard::require_available(slot, store, &repo)?;
                crate::guard::require_clean(&repo, &paths)?;
                baseline_commit = expected_head.clone();
            }
            crate::guard::check_pins(&repo, store, expected_head.as_deref(), expected_branch.as_deref())?;
            let run =
                new_run(&id, name, model, &target, (baseline_commit, dispatch_dirty, dispatch_staged), accept_with);
            if let Some(request) = &request
                && let Some(run_id) = request.claim(&mut locked, &run.id)?
            {
                return Ok((Output::Replayed { run_id }, 0));
            }
            store.publish(&mut locked, &run, &store.dir(&id))?;
            if let Some(slot) = slot {
                slot.assign(&run.id)?;
            }
            let (mut launched, code) = launch(run, &locked);
            // Checked dispatch acquires the published run lock again.
            drop(locked);

            if checks.is_empty() {
                return Ok((Output::Launch(launched), code));
            }
            let attempt = launched.run.attempt;
            let result = if matches!(launched.outcome, LaunchOutcome::LaunchFailed(_)) {
                Err(anyhow::anyhow!("worker launch failed"))
            } else {
                let deadline = Instant::now() + Duration::from_secs(check_timeout.unwrap_or(3600).into());
                verify_launched_attempt(store, &id, &mut launched.run, checks, deadline)
            };
            let (verification, check_code) = match result {
                Ok(verification) => {
                    let code = i32::from(matches!(verification.verdict, Verdict::Fail));
                    (CheckedLaunchVerification::Verified(verification), code)
                }
                Err(error) => (CheckedLaunchVerification::Skipped { attempt, message: format!("{error:#}") }, 1),
            };
            return Ok((Output::CheckedLaunch { launched, verification }, code.max(check_code)));
        }
        Models::Review(models) => models,
    };
    let target = resolve_target()?;
    let bytes = read_unkeyed()?;
    lint_snapshot(&bytes, &mode, &repo, &store.root)?;
    let (mut runs, mut code, mut error) = (vec![], 0, None);
    for model in models {
        let name = name.as_ref().map(|name| format!("{name}-{model}"));
        let result = (|| -> Result<_> {
            let (id, mut locked, snapshot) = stage(&bytes, false)?;
            let run = new_run(&id, name, Some(model.clone()), &target, snapshot, None);
            store.publish(&mut locked, &run, &store.dir(&id))?;
            Ok(launch(run, &locked))
        })();
        match result {
            Ok((launched, one_code)) => {
                runs.push(launched);
                code = code.max(one_code);
            }
            // Earlier runs may already be working, so their ids must still be reported.
            Err(e) => {
                error = Some(format!("{model}: {e:#}"));
                code = 1;
                break;
            }
        }
    }
    Ok((Output::Launches { runs, error }, code))
}

#[derive(Serialize)]
struct DispatchPayload<'a> {
    mode: &'a str,
    target: &'a Target,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    goal: Option<&'a str>,
    name: Option<&'a str>,
    extra: &'a [String],
    paths: &'a [String],
    accept_with: Option<&'a Path>,
    expected_head: Option<&'a str>,
    expected_branch: Option<&'a str>,
    brief: &'a [u8],
}

fn verify_launched_attempt(
    store: &Store,
    id: &RunId,
    dispatched: &mut Run,
    checks: Vec<String>,
    deadline: Instant,
) -> Result<FreshVerification> {
    let poll_interval = Duration::from_secs(5).min(deadline.saturating_duration_since(Instant::now()) / 3);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "timed out waiting for the dispatched attempt to finish");
        {
            let lock = store.try_lock(id)?;
            let mut run = store.load(id)?;
            let require_attempt = |run: &Run| {
                anyhow::ensure!(run.launch_attempt() == dispatched.attempt, "the dispatched attempt was superseded");
                Ok(())
            };
            require_attempt(&run)?;
            let observation = runs::observe_until(&mut run, lock.as_ref(), Some(deadline), ScriptSource::Configured);
            require_attempt(&run)?;
            let state = observation.state();
            match state {
                State::Finished => {
                    if run.launch.target().executor() == Executor::Codex {
                        let events = store.dir(id).join(run.launch_events_name());
                        anyhow::ensure!(
                            crate::progress::codex_turn_completed(&events),
                            "Codex completion requires a turn.completed event for the dispatched attempt"
                        );
                    }
                    drop(lock);
                    return crate::verify::run(dispatched, vec![], checks, None, store, id, Some(deadline));
                }
                State::Failed | State::Stopped | State::Blocked => {
                    anyhow::bail!("worker settled in state {}", state.as_str());
                }
                _ => {}
            }
        }
        std::thread::sleep(poll_interval.min(deadline.saturating_duration_since(Instant::now())));
    }
}
