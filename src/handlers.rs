use crate::{
    Action, DispatchArgs,
    dispatch_request::DispatchRequest,
    domain::*,
    process::{self, ScriptSource},
    runs,
    store::{self, RunId, ScopedNotFound, Store},
    writer_slot::Slot,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub enum Output {
    Doctor(crate::doctor::Doctor),
    Context(crate::context::Checkout),
    Ls {
        runs: Vec<ListEntry>,
        summary: BTreeMap<&'static str, BTreeMap<&'static str, usize>>,
    },
    Wait {
        runs: Vec<WaitEntry>,
        message: Option<&'static str>,
        timed_out: Option<bool>,
    },
    Launch(Launched),
    Replayed {
        run_id: String,
    },
    GuardVerification(crate::guard::Verification),
    CheckedLaunch {
        launched: Launched,
        verification: CheckedLaunchVerification,
    },
    /// One review dispatch fanned out to several models; `error` stopped the remaining launches.
    Launches {
        runs: Vec<Launched>,
        error: Option<String>,
    },
    Run {
        run: Box<Run>,
        observation: Observation,
        detail: RunDetail,
    },
    Message(String),
    Rejection(String),
}

pub struct Launched {
    pub run: Box<Run>,
    pub outcome: LaunchOutcome,
}

pub enum CheckedLaunchVerification {
    Verified(FreshVerification),
    Skipped { attempt: NonZeroU32, message: String },
}

pub enum LaunchOutcome {
    Started,
    LaunchFailed(String),
    PaneFailed(String),
}

pub enum RunDetail {
    Status { dir: PathBuf, tail: Option<Tail> },
    Report { path: PathBuf, content: Option<String> },
    Stop,
    Resume,
    Verify(FreshVerification),
}

pub enum Tail {
    Lines(PathBuf, Vec<String>),
    Unavailable(PathBuf, String),
    NoStream(String),
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ListEntry {
    Run { run: Box<Run>, observation: Observation },
    Broken { run_id: String, error: String },
    Incomplete { outcome: &'static str },
}

#[derive(Serialize)]
pub struct WaitEntry {
    pub id: String,
    pub name: String,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<NonZeroU32>,
    pub report_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::progress::Progress>,
}

pub fn handle(action: Action, json_output: bool) -> Result<(Output, i32)> {
    if let Action::Wait { run_dir: Some(dir), all, timeout, .. } = &action {
        let (store, id) = Store::from_run_dir(dir)?;
        let run = store.load(&id)?;
        if !matches!(run.launch, Launch::Failed { .. }) && matches!(run.launch.target(), Target::Codex | Target::Vibe) {
            crate::scripts::require_status_tools(ScriptSource::Embedded)?;
        }
        return wait(&store, &[id], None, *all, *timeout, json_output, ScriptSource::Embedded);
    }
    let store = Store::new()?;
    let scope = match &action {
        Action::Wait { repo, .. }
        | Action::Status { repo, .. }
        | Action::Report { repo, .. }
        | Action::Ls { repo, .. }
        | Action::Verify { repo, .. } => repo.as_deref().map(store::directory).transpose()?,
        _ => None,
    };
    let repo = scope.as_deref();
    let read = |selector: &str| -> Result<_> {
        let id = store.observation_id(selector, repo.is_some())?;
        let (locked, run) = store.read_for_observation(&id, repo, None)?.ok_or(ScopedNotFound)?;
        Ok((id, locked, run))
    };
    let (run, observation, detail, code) = match action {
        Action::ClaudeStopHook { dir } => {
            if let Err(error) = crate::claude::stop_hook(&dir) {
                eprintln!("Claude report hook: {error:#}");
            }
            std::process::exit(0);
        }
        Action::Doctor { executor, profile } => {
            return crate::doctor::run(executor, profile).map(|(doctor, code)| (Output::Doctor(doctor), code));
        }
        Action::Context { repo } => {
            return crate::context::read(&repo, &store.root).map(|context| (Output::Context(context), 0));
        }
        Action::Dispatch { args, checks, check_timeout } => {
            return dispatch(args, &store, false, checks, check_timeout);
        }
        Action::Handoff(args) => return dispatch(args, &store, true, vec![], None),
        Action::Wait { runs, all, timeout, .. } => {
            if repo.is_some() && timeout == 0 {
                return Ok((Output::Rejection("wait --repo requires a nonzero --timeout".into()), 2));
            }
            let ids = runs.iter().map(|id| store.observation_id(id, repo.is_some())).collect::<Result<Vec<_>>>()?;
            return wait(&store, &ids, repo, all, timeout, json_output, ScriptSource::Configured);
        }
        Action::Ls { goal, .. } => {
            let mut summary = BTreeMap::<&str, BTreeMap<&str, usize>>::new();
            let mut data = vec![];
            let mut incomplete = false;
            for id in store.runs()? {
                let id = match id {
                    Ok(id) => id,
                    Err(_) if repo.is_some() => {
                        incomplete = true;
                        continue;
                    }
                    Err(run_id) => {
                        data.push(ListEntry::Broken { run_id, error: "invalid run id".into() });
                        continue;
                    }
                };
                let entry = (|| -> Result<_> {
                    let Some((locked, mut run)) = store.read_for_observation(&id, repo, goal.as_deref())? else {
                        return Ok(None);
                    };
                    let observation = runs::observe(&mut run, locked.as_ref());
                    let executor = run.launch.target().executor().as_str();
                    let state = observation.state().as_str();
                    *summary.entry(executor).or_default().entry(state).or_default() += 1;
                    Ok(Some(ListEntry::Run { run: Box::new(run), observation }))
                })();
                match entry {
                    Ok(entry) => data.extend(entry),
                    Err(_) if repo.is_some() => incomplete = true,
                    Err(error) => data.push(ListEntry::Broken { run_id: id.to_string(), error: format!("{error:#}") }),
                }
            }
            if incomplete {
                data.push(ListEntry::Incomplete { outcome: "incomplete" });
            }
            return Ok((Output::Ls { runs: data, summary }, 0));
        }
        Action::Status { run, tail, .. } => {
            let (id, locked, mut run) = read(&run)?;
            let dir = store.dir(&id);
            let observation = runs::observe(&mut run, locked.as_ref());
            let tail = tail.map(|count| {
                if run.launch.target().executor() == Executor::Claude {
                    return Tail::NoStream("tail: none (Claude runs have no herder-recorded event stream)".into());
                }
                let path = dir.join(run.launch_events_name());
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let mut lines: Vec<_> = content
                            .lines()
                            .rev()
                            .take(usize::from(count))
                            .map(|line| match line.char_indices().nth(500) {
                                Some((cut, _)) => format!("{} ...[+{} bytes]", &line[..cut], line.len() - cut),
                                None => line.to_owned(),
                            })
                            .collect();
                        lines.reverse();
                        Tail::Lines(path, lines)
                    }
                    Err(error) => {
                        let message = if error.kind() == std::io::ErrorKind::NotFound {
                            format!("tail: stream does not exist yet: {}", path.display())
                        } else {
                            format!("tail: cannot read {}: {error}", path.display())
                        };
                        Tail::Unavailable(path, message)
                    }
                }
            });
            (run, observation, RunDetail::Status { dir, tail }, 0)
        }
        Action::Report { run, .. } => {
            let (id, locked, mut run) = read(&run)?;
            let observation = runs::observe(&mut run, locked.as_ref());
            let path = store.dir(&id).join(run.report_name());
            let content = store::report(&path)?;
            let code = i32::from(content.is_none());
            (run, observation, RunDetail::Report { path, content }, code)
        }
        Action::Stop { run } => {
            let locked = store.lock(&store.resolve(&run)?)?;
            let mut run = locked.load()?;
            // Preserve identity recovery before stop checks the current executor state.
            runs::observe(&mut run, Some(&locked));
            let observation = runs::stop(&mut run, &locked)?;
            (run, observation, RunDetail::Stop, 0)
        }
        Action::Resume { run, brief, request_id } => {
            let locked = store.lock(&store.resolve(&run)?)?;
            let mut run = locked.load()?;
            let observation = runs::resume(&mut run, &brief, request_id.as_deref(), &locked)?;
            (run, observation, RunDetail::Resume, 0)
        }
        Action::Verify { run, paths, checks, baseline, accept_with, .. } => {
            if let (Some(repo), Some(check)) = (repo, accept_with) {
                crate::guard::require_external_state(repo, &store.root)?;
                let slot = Slot::lock(&store, repo)?;
                let (_, locked, run) = read(&run)?;
                return crate::guard::verify(run, locked, &check, &paths, slot)
                    .map(|(result, code)| (Output::GuardVerification(result), code));
            }
            let (id, locked, mut run) = read(&run)?;
            if run.accept_with.is_some() && baseline.is_some() {
                return Ok((Output::Rejection("cannot replace the baseline of a protected run".into()), 2));
            }
            // Validate before observation, which can recover and save executor identity.
            let baseline = if let Some(rev) = baseline {
                let args = ["rev-parse", "--verify", &format!("{rev}^{{commit}}")];
                let commit = process::checked(std::process::Command::new("git").current_dir(&run.repo).args(args))?;
                Some(commit.trim().to_owned())
            } else {
                None
            };
            let observation = runs::observe(&mut run, locked.as_ref());
            let state = observation.state();
            if matches!(state, State::Starting | State::Running | State::Blocked) {
                let message = format!("cannot verify worker in state {}", state.as_str());
                return Ok((Output::Message(message), 2));
            }
            if paths.is_empty() && run.authorized_paths.is_empty() {
                return Ok((Output::Message("verify requires --path when none were recorded at dispatch".into()), 2));
            }
            drop(locked);
            let verification = crate::verify::run(&mut run, paths, checks, baseline, &store, &id, None)?;
            let code = i32::from(matches!(verification.verdict, Verdict::Fail));
            let locked = store.try_lock(&id)?;
            let mut run = store.load(&id)?;
            let observation = runs::observe(&mut run, locked.as_ref());
            (run, observation, RunDetail::Verify(verification), code)
        }
    };
    Ok((Output::Run { run: Box::new(run), observation, detail }, code))
}
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

fn dispatch(
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
    if accept_with.is_some() {
        crate::guard::require_external_state(&repo, &store.root)?;
    }
    let target = match executor {
        Executor::Claude => Target::Claude { profile: process::profile(profile)? },
        Executor::Vibe => Target::Vibe,
        Executor::Codex => Target::Codex,
    };
    let input = if handoff {
        // Validate the continuation document before any staging or launch side effect.
        let document = match crate::handoff::document(&brief) {
            Ok(document) => document,
            Err(error) => return reject(format!("{error:#}")),
        };
        Input::Generated(crate::handoff::prompt(&mode, &document).into_bytes())
    } else if request_id.is_some() {
        Input::Generated(std::fs::read(&brief).context("read dispatch brief")?)
    } else {
        process::lint(&brief, &mode)?;
        Input::Brief(brief)
    };
    let request = if let (Some(key), Input::Generated(bytes), Models::Single(model)) = (&request_id, &input, &models) {
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
            brief: bytes,
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
    let launch_run = |name: Option<String>, model: Option<String>| -> Result<_> {
        let mut baseline_commit = if accept_with.is_none() {
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
        let (id, mut locked) = store.create()?;
        let dir = store.dir(&id);
        let input_md = locked.dir.join("input.md");
        let mut may_be_claimed = false;
        let result = (|| -> Result<_> {
            match &input {
                Input::Brief(source) => {
                    store::copy(source, &input_md)?;
                }
                Input::Generated(bytes) => {
                    store::atomic(&input_md, bytes)?;
                    process::lint(&input_md, &mode)?;
                }
            }
            let slot = accept_with.is_some().then(|| Slot::lock(store, &repo)).transpose()?;
            if let Some(request) = &request
                && let Some(run_id) = request.existing()?
            {
                return Ok(InitialDispatch::Replayed(run_id));
            }
            if let Some(slot) = &slot {
                crate::guard::require_available(slot, store, &repo)?;
                crate::guard::require_clean(&repo, &paths)?;
                baseline_commit = expected_head.clone();
            }
            crate::guard::check_pins(&repo, store, expected_head.as_deref(), expected_branch.as_deref())?;
            let mut run = Run {
                name: name.unwrap_or_else(|| id.to_string()),
                id: id.to_string(),
                model,
                requested_effort: effort.clone(),
                extra: extra.clone(),
                repo: repo.clone(),
                baseline_commit,
                dispatch_dirty,
                brief: dir.join("input.md"),
                goal: goal.clone(),
                launch: Launch::Starting { target: target.clone() },
                attempt: NonZeroU32::MIN,
                resume_worker_dir: None,
                request_attempts: Default::default(),
                authorized_paths: paths.clone(),
                accept_with: accept_with.clone(),
                protected_verification: None,
                verifications: vec![],
                created_at: store::timestamp(),
            };
            if let Some(request) = &request {
                // A failed claim may have persisted before a subsequent sync failed.
                may_be_claimed = true;
                match request.claim(&run.id) {
                    Ok(Some(run_id)) => {
                        may_be_claimed = false;
                        return Ok(InitialDispatch::Replayed(run_id));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        may_be_claimed = !error.is::<crate::dispatch_request::Conflict>();
                        return Err(error);
                    }
                }
            }
            store.publish(&mut locked, &run, &dir)?;
            if let Some(slot) = slot {
                slot.assign(&run.id)?;
            }
            let outcome = match runs::launch(&mut run, LaunchPhase::Initial, &locked) {
                Err(error) => LaunchOutcome::LaunchFailed(format!("{error:#}")),
                Ok(()) if pane => match runs::pane(&run, &dir) {
                    Ok(()) => LaunchOutcome::Started,
                    Err(error) => LaunchOutcome::PaneFailed(format!("worker recorded; pane failed: {error:#}")),
                },
                Ok(()) => LaunchOutcome::Started,
            };
            let code = i32::from(!matches!(outcome, LaunchOutcome::Started));
            Ok(InitialDispatch::Launched(id, Launched { run: Box::new(run), outcome }, code))
        })();
        if locked.dir != dir && !may_be_claimed {
            let _ = std::fs::remove_dir_all(&locked.dir);
        }
        result
    };
    let models = match models {
        Models::Single(model) => {
            let (id, mut launched, code) = match launch_run(name, model)? {
                InitialDispatch::Launched(id, launched, code) => (id, launched, code),
                InitialDispatch::Replayed(run_id) => return Ok((Output::Replayed { run_id }, 0)),
            };
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
    let (mut runs, mut code, mut error) = (vec![], 0, None);
    for model in models {
        let name = name.as_ref().map(|name| format!("{name}-{model}"));
        match launch_run(name, Some(model.clone())) {
            Ok(InitialDispatch::Launched(_, launched, one_code)) => {
                runs.push(launched);
                code = code.max(one_code);
            }
            Ok(InitialDispatch::Replayed(_)) => unreachable!("multi-model dispatch cannot claim a request"),
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

enum InitialDispatch {
    Launched(RunId, Launched, i32),
    Replayed(String),
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

enum WaitRegistration {
    Script { _resume: crate::program_resume::Resume },
    AwaitingClaude(crate::program_resume::Terminal),
    Claude { resume: crate::program_resume::Resume, repo: PathBuf, native: Native },
}

fn update_claude_registration(
    registration: &mut Option<WaitRegistration>,
    run: &Run,
    observation: &Observation,
    refreshed: bool,
    dir: &Path,
) {
    let confirmed = match &run.launch {
        Launch::Launched {
            native: native @ Native::Claude { id, session_id, .. },
            intent: Intent::Running | Intent::Unconfirmed { .. },
        } if !id.is_empty() && !session_id.is_empty() => Some(native),
        _ => None,
    };
    if let Some(WaitRegistration::Claude { resume, repo, native }) = registration {
        if run.repo != *repo
            || confirmed != Some(native)
            || matches!(observation.cause(), Some(UnknownCause::IdentityMismatch))
        {
            resume.set_eligible(false);
        } else if refreshed && observation.state() == State::Running {
            resume.set_eligible(true);
        }
    }
    if refreshed
        && observation.state() == State::Running
        && let Some(native) = confirmed
        && matches!(registration, Some(WaitRegistration::AwaitingClaude(_)))
        && let Some(WaitRegistration::AwaitingClaude(terminal)) = registration.take()
    {
        *registration = terminal.register(dir).map(|resume| WaitRegistration::Claude {
            resume,
            repo: run.repo.clone(),
            native: native.clone(),
        });
    }
}

fn wait(
    store: &Store,
    ids: &[RunId],
    repo: Option<&Path>,
    all: bool,
    timeout: u64,
    json_output: bool,
    source: ScriptSource,
) -> Result<(Output, i32)> {
    let replay = if let [id] = ids
        && repo.is_none()
        && !all
        && !json_output
        && timeout == crate::DEFAULT_WAIT_TIMEOUT
        && let Some(terminal) = crate::program_resume::Terminal::open()
    {
        let dir = store.dir(id);
        (|| {
            let canonical = dir.canonicalize().ok()?;
            if canonical.file_name() != dir.file_name() {
                return None;
            }
            let (pinned, _) = Store::from_run_dir(&canonical).ok()?;
            let run = pinned.load(id).ok()?;
            let Some(executor) = crate::scripts::ScriptExecutor::for_executor(run.launch.target().executor()) else {
                return Some((pinned, WaitRegistration::AwaitingClaude(terminal)));
            };
            if source.override_dir().is_some() {
                return None;
            }
            crate::scripts::require_status_tools(source).ok()?;
            executor.status_dir(&run, &canonical).ok()?;
            Some((pinned, WaitRegistration::Script { _resume: terminal.register(&canonical)? }))
        })()
    } else {
        None
    };
    let (pinned, mut registration) = replay.unzip();
    let (observed_store, source) = pinned.as_ref().map_or((store, source), |pinned| (pinned, ScriptSource::Embedded));
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout);
    let deadline = if let Some(repo) = repo {
        for id in ids {
            store.load_in_repo(id, repo)?;
        }
        Some(started.checked_add(timeout).context("wait timeout is too large")?)
    } else {
        None
    };
    let interval = if repo.is_some() { Duration::from_secs(5).min(timeout / 3) } else { Duration::from_secs(5) };
    let mut previous = vec![None; ids.len()];
    let mut reported = vec![None; ids.len()];
    loop {
        let mut data = Vec::with_capacity(ids.len());
        let (mut settled, mut finished, mut unknown) = (0, 0, false);
        for (index, id) in ids.iter().enumerate() {
            // The guard is dropped at the end of this iteration, before sleeping.
            let (locked, mut run) = observed_store.read_for_observation(id, repo, None)?.ok_or(ScopedNotFound)?;
            let observation = runs::observe_until(&mut run, locked.as_ref(), deadline, source);
            let refreshed = locked.is_some();
            drop(locked);
            let dir = observed_store.dir(id);
            if matches!(registration, Some(WaitRegistration::AwaitingClaude(_) | WaitRegistration::Claude { .. })) {
                update_claude_registration(&mut registration, &run, &observation, refreshed, &dir);
            }
            let state = observation.state();
            if previous[index] != Some(state) {
                if !json_output {
                    crate::render::wait_progress(&run, state);
                }
                previous[index] = Some(state);
            }
            if !json_output
                && matches!(run.launch.target(), Target::Claude { .. })
                && (state == State::Running || reported[index].is_some())
            {
                let attempt = run.launch_attempt();
                let path = dir.join(run.launch_report_name());
                if store::nonempty_report_metadata(&path).is_none() {
                    reported[index] = None;
                } else if refreshed
                    && state == State::Running
                    && reported[index] != Some(attempt)
                    && store::report(&path).is_ok_and(|content| content.is_some_and(|text| !text.is_empty()))
                {
                    eprintln!("{id} attempt {attempt}: report available at {}", path.to_string_lossy().escape_debug());
                    reported[index] = Some(attempt);
                }
            }
            settled += usize::from(matches!(state, State::Finished | State::Failed | State::Stopped | State::Blocked));
            finished += usize::from(state == State::Finished);
            unknown |= state == State::Unknown;
            data.push((run, observation, dir, refreshed));
        }
        let met = if all { settled == ids.len() } else { settled > 0 };
        // A status query can time out under load; only an unknown that outlasts the wait is reported.
        let remaining = timeout.saturating_sub(started.elapsed());
        if !met && !remaining.is_zero() {
            std::thread::sleep(interval.min(remaining));
            if deadline.is_none_or(|end| Instant::now() < end) {
                continue;
            }
        }
        let message =
            (!met).then_some(if unknown { "unknown state at timeout: inspect run evidence" } else { "wait timed out" });
        let runs = data
            .into_iter()
            .map(|(run, observation, dir, refreshed)| {
                let state = observation.state();
                let progress = (!met && matches!(state, State::Starting | State::Running | State::Unknown))
                    .then(|| crate::progress::last(&run, &observation, &dir, refreshed));
                let report_path = dir.join(run.report_name());
                let attempt = repo.map(|_| run.launch_attempt());
                WaitEntry { id: run.id, name: run.name, state, attempt, report_path, progress }
            })
            .collect();
        return Ok((
            Output::Wait { runs, message, timed_out: repo.map(|_| !met) },
            i32::from(!met || finished != settled),
        ));
    }
}
