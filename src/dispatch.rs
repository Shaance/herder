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
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

enum Models {
    Single(Option<String>),
    Review(Vec<String>),
}

fn codex_config_assignments(extra: &[String]) -> impl Iterator<Item = (&str, &str)> {
    let mut args = extra.iter().map(String::as_str);
    std::iter::from_fn(move || {
        while let Some(arg) = args.next() {
            let assignment = match arg {
                "--" => break,
                "-c" | "--config" => args.next(),
                _ => arg
                    .strip_prefix("--config=")
                    .or_else(|| arg.strip_prefix("-c").map(|value| value.strip_prefix('=').unwrap_or(value))),
            };
            if let Some((key, value)) = assignment.and_then(|value| value.split_once('=')) {
                return Some((key.trim(), value.trim()));
            }
        }
        None
    })
}

pub(crate) fn dispatch(
    args: DispatchArgs,
    store: &Store,
    handoff: bool,
    checks: Vec<String>,
    check_timeout: Option<u32>,
) -> Result<(Output, i32)> {
    let DispatchArgs {
        mode,
        executor,
        brief,
        repo,
        mut model,
        effort,
        goal,
        name,
        profile,
        pane,
        paths,
        mut extra,
        request_id,
        accept_with,
        expected_head,
        expected_branch,
    } = args;
    let reject = |message: String| Ok((Output::Rejection(message), 2));
    if !checks.is_empty() && model.len() > 1 {
        return reject("--check requires a single dispatch".into());
    }
    if request_id.is_some() && (handoff || pane || !checks.is_empty() || model.len() > 1) {
        return reject("--request-id requires one dispatch without --pane or --check".into());
    }
    if (expected_head.is_some() || expected_branch.is_some())
        && accept_with.is_none()
        && (mode != "review" || handoff || model.len() > 1)
    {
        return reject("checkout pins require a single review dispatch or --accept-with".into());
    }
    if accept_with.is_some() {
        if mode != "direct" || executor != Executor::Codex || !extra.is_empty() {
            return reject("--accept-with requires a direct Codex dispatch without native extra arguments".into());
        }
        extra.extend(["--sandbox".into(), "workspace-write".into()]);
    }
    if checks.iter().any(|check| check.trim().is_empty()) {
        return reject("--check requires a nonempty shell command".into());
    }
    if executor != Executor::Claude && profile.is_some() {
        return reject("--profile applies only to Claude".into());
    }
    if effort.is_some() {
        let conflict = match executor {
            Executor::Vibe => {
                return reject(
                    "Vibe has no per-run effort; effort comes from the model alias's `thinking` setting in Vibe's config.toml, choose an alias with --model"
                        .into(),
                );
            }
            Executor::Codex => codex_config_assignments(&extra).any(|(key, _)| key == "model_reasoning_effort"),
            Executor::Claude => extra.iter().any(|arg| arg == "--effort" || arg.starts_with("--effort=")),
        };
        if conflict {
            return reject("--effort conflicts with native effort arguments after --".into());
        }
    }
    if executor == Executor::Codex && mode == "review" {
        if codex_config_assignments(&extra).any(|(key, value)| {
            key == "sandbox_mode" && !matches!(value, "read-only" | r#""read-only""# | "'read-only'")
        }) {
            return reject("Codex review requires sandbox_mode to be read-only".into());
        }
        let mut sandbox_specified = false;
        let mut args = extra.iter_mut();
        while let Some(arg) = args.next() {
            let value = match arg.as_str() {
                "--" => return reject("Codex review extras cannot include a second -- separator".into()),
                "--dangerously-bypass-approvals-and-sandbox"
                | "--yolo"
                | "--full-auto"
                | "--approve-for-me"
                | "--not-so-yolo" => {
                    return reject(format!("Codex review requires a read-only sandbox and cannot use {arg}"));
                }
                "--sandbox" | "-s" => args.next().map(|value| value.as_str()),
                flag if flag.starts_with("--sandbox=") => flag.strip_prefix("--sandbox="),
                flag if flag.starts_with("-s") => {
                    flag.strip_prefix("-s").map(|value| value.strip_prefix('=').unwrap_or(value))
                }
                _ => continue,
            };
            if sandbox_specified {
                return reject("Codex review accepts only one explicit sandbox flag".into());
            }
            if value != Some("read-only") {
                return reject("Codex review requires --sandbox read-only".into());
            }
            if arg.starts_with("-s") && arg != "-s" {
                // The launchers only recognize the split short flag or the long flag.
                *arg = "--sandbox=read-only".into();
            }
            sandbox_specified = true;
        }
        if !sandbox_specified {
            extra.extend(["--sandbox".into(), "read-only".into()]);
        }
    }
    let models = match model.as_slice() {
        [] | [_] => Models::Single(model.pop()),
        _ if handoff => return reject("a handoff continues one conversation; pass --model at most once".into()),
        // Several writers in one worktree would edit the same files concurrently.
        _ if mode != "review" => {
            return reject("repeat --model only with --mode review; a writer runs one model".into());
        }
        _ if model.iter().enumerate().any(|(i, name)| model[..i].contains(name)) => {
            return reject("each repeated --model must name a different model".into());
        }
        _ => Models::Review(model),
    };
    let repo = store::repo(&repo)?;
    let resolve_target = || -> Result<Target> {
        Ok(match executor {
            Executor::Claude => Target::Claude { profile: process::profile(profile)? },
            Executor::Vibe => Target::Vibe,
            Executor::Codex => Target::Codex,
        })
    };
    let stage = |input: &Input, protected: bool| -> Result<_> {
        let baseline_commit = if !protected {
            process::checked(std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "HEAD"]))
                .ok()
                .map(|s| s.trim().to_owned())
        } else {
            None
        };
        let mut dispatch_dirty = std::collections::BTreeMap::new();
        if baseline_commit.is_some() {
            for path in crate::verify::dirty_paths(&repo, None)? {
                let hash = crate::verify::content_hash(&repo, &path, None)?;
                dispatch_dirty.insert(path, hash);
            }
        }
        let (id, locked) = store.create()?;
        let input_md = locked.dir.join("input.md");
        match input {
            Input::Brief(source) => {
                store::copy(source, &input_md)?;
            }
            Input::Generated(bytes) => {
                store::atomic(&input_md, bytes)?;
                process::lint(&input_md, &mode)?;
            }
        }
        Ok((id, locked, (baseline_commit, dispatch_dirty)))
    };
    let new_run =
        |id: &RunId, name: Option<String>, model, target: &Target, (baseline_commit, dispatch_dirty), accept_with| {
            Run {
                name: name.unwrap_or_else(|| id.to_string()),
                id: id.to_string(),
                model,
                requested_effort: effort.clone(),
                extra: extra.clone(),
                repo: repo.clone(),
                baseline_commit,
                dispatch_dirty,
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
            let (input, request) = if handoff {
                // Validate the continuation document before any staging or launch side effect.
                let document = match crate::handoff::document(&brief) {
                    Ok(document) => document,
                    Err(error) => return reject(format!("{error:#}")),
                };
                (Input::Generated(crate::handoff::prompt(&mode, &document).into_bytes()), None)
            } else if let Some(key) = &request_id {
                let bytes = std::fs::read(&brief).context("read dispatch brief")?;
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
                (Input::Generated(bytes), Some(request))
            } else {
                process::lint(&brief, &mode)?;
                (Input::Brief(brief), None)
            };
            let accept_with = accept_with.as_deref().map(|path| crate::guard::host_check(&repo, path)).transpose()?;
            let (id, mut locked, (mut baseline_commit, dispatch_dirty)) = stage(&input, accept_with.is_some())?;
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
            let run = new_run(&id, name, model, &target, (baseline_commit, dispatch_dirty), accept_with);
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
    process::lint(&brief, &mode)?;
    let input = Input::Brief(brief);
    let (mut runs, mut code, mut error) = (vec![], 0, None);
    for model in models {
        let name = name.as_ref().map(|name| format!("{name}-{model}"));
        let result = (|| -> Result<_> {
            let (id, mut locked, baseline) = stage(&input, false)?;
            let run = new_run(&id, name, Some(model.clone()), &target, baseline, None);
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

enum Input {
    /// Dispatch byte-copies the already linted brief file into the run staging directory.
    Brief(PathBuf),
    /// Handoff stages the generated prompt as input.md and lints it inside the staging directory.
    Generated(Vec<u8>),
}
