use crate::{
    Action, Dispatch,
    domain::*,
    process, runs,
    store::{self, Store},
};
use anyhow::Result;
use serde::Serialize;
use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf};

pub enum Output {
    Doctor(crate::doctor::Doctor),
    Ls {
        runs: Vec<ListEntry>,
        summary: BTreeMap<&'static str, BTreeMap<&'static str, usize>>,
    },
    Wait {
        runs: Vec<WaitEntry>,
        message: Option<&'static str>,
    },
    Launch(Launched),
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
    pub launch_error: Option<String>,
    pub pane_error: Option<String>,
}

pub enum RunDetail {
    Status { dir: PathBuf, tail: Option<Tail> },
    Report { path: PathBuf, content: Option<String> },
    Stop,
    Resume,
    Verify,
}

#[derive(Serialize)]
pub struct Tail {
    pub path: Option<PathBuf>,
    pub lines: Vec<String>,
    pub message: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ListEntry {
    Run { run: Box<Run>, observation: Observation },
    Broken { run_id: String, error: String },
}

#[derive(Serialize)]
pub struct WaitEntry {
    pub id: String,
    pub name: String,
    pub state: State,
    pub report_path: PathBuf,
}

pub fn handle(mut action: Action, json_output: bool) -> Result<(Output, i32)> {
    let store = Store::new()?;
    match &mut action {
        Action::Status { run, .. }
        | Action::Report { run }
        | Action::Stop { run }
        | Action::Resume { run, .. }
        | Action::Verify { run, .. } => *run = store.resolve(run)?,
        Action::Wait { runs, .. } => {
            *runs = runs.iter().map(|id| store.resolve(id)).collect::<Result<_>>()?;
        }
        _ => {}
    }
    match action {
        Action::Doctor { executor, profile } => {
            crate::doctor::run(executor, profile).map(|(doctor, code)| (Output::Doctor(doctor), code))
        }
        Action::Dispatch(args) => dispatch(args, &store),
        Action::Handoff(args) => handoff(args, &store),
        Action::Wait { runs, all, timeout } => wait(&store, &runs, all, timeout, json_output),
        Action::Ls { goal } => {
            let mut summary = BTreeMap::<&str, BTreeMap<&str, usize>>::new();
            let mut data = vec![];
            for id in store.runs()? {
                let entry = (|| -> Result<_> {
                    let locked = store.try_lock(&id)?;
                    let mut run = store.load(&id)?;
                    if goal.is_some() && run.goal != goal {
                        return Ok(None);
                    }
                    let observation = runs::observe(&mut run, locked.as_ref());
                    let executor = run.launch.target().executor().as_str();
                    let state = observation.state().as_str();
                    *summary.entry(executor).or_default().entry(state).or_default() += 1;
                    Ok(Some(ListEntry::Run { run: Box::new(run), observation }))
                })();
                match entry {
                    Ok(Some(entry)) => data.push(entry),
                    Ok(None) => {}
                    Err(error) => data.push(ListEntry::Broken { run_id: id, error: format!("{error:#}") }),
                }
            }
            Ok((Output::Ls { runs: data, summary }, 0))
        }
        action => {
            let id = match &action {
                Action::Status { run, .. }
                | Action::Report { run }
                | Action::Stop { run }
                | Action::Resume { run, .. }
                | Action::Verify { run, .. } => run,
                _ => unreachable!(),
            };
            let mut locked = match action {
                Action::Stop { .. } | Action::Resume { .. } => Some(store.lock(id)?),
                _ => store.try_lock(id)?,
            };
            let dir = store.dir(id)?;
            let mut run = store::load(&dir)?;
            // Validate before observation, which can recover and save executor identity.
            let baseline = if let Action::Verify { baseline: Some(rev), .. } = &action {
                Some(
                    process::checked(std::process::Command::new("git").current_dir(&run.repo).args([
                        "rev-parse",
                        "--verify",
                        &format!("{rev}^{{commit}}"),
                    ]))?
                    .trim()
                    .to_owned(),
                )
            } else {
                None
            };
            let mut observation = if matches!(action, Action::Resume { .. }) {
                Observation::new(run.launch.state(), String::new(), UnknownCause::Unconfirmed)
            } else {
                runs::observe(&mut run, locked.as_ref())
            };
            let mut code = 0;
            let detail = match action {
                Action::Report { .. } => {
                    let path = dir.join(run.report_name());
                    let content = store::report(&path)?;
                    if content.is_none() {
                        code = 1;
                    }
                    RunDetail::Report { path, content }
                }
                Action::Stop { .. } => {
                    observation = runs::stop(&mut run, locked.as_ref().expect("stop acquired lock"))?;
                    RunDetail::Stop
                }
                Action::Resume { brief, request_id, .. } => {
                    runs::resume(
                        &mut run,
                        &brief,
                        request_id.as_deref(),
                        locked.as_ref().expect("resume acquired lock"),
                    )?;
                    observation = runs::observe(&mut run, locked.as_ref());
                    RunDetail::Resume
                }
                Action::Verify { paths, checks, .. } => {
                    if matches!(observation.state(), State::Starting | State::Running | State::Blocked) {
                        return Ok((
                            Output::Message(format!("cannot verify worker in state {}", observation.state().as_str())),
                            2,
                        ));
                    }
                    if paths.is_empty() && run.authorized_paths.is_empty() {
                        return Ok((
                            Output::Message("verify requires --path when none were recorded at dispatch".into()),
                            2,
                        ));
                    }
                    drop(locked.take());
                    crate::verify::run(&mut run, paths, checks, baseline, &store)?;
                    if matches!(run.verifications.last().map(|v| &v.verdict), Some(Verdict::Fail)) {
                        code = 1;
                    }
                    locked = store.try_lock(&run.id)?;
                    run = store.load(&run.id)?;
                    observation = runs::observe(&mut run, locked.as_ref());
                    RunDetail::Verify
                }
                Action::Status { tail, .. } => RunDetail::Status {
                    tail: tail.map(|count| {
                        if run.launch.target().executor() == Executor::Claude {
                            return Tail {
                                path: None,
                                lines: vec![],
                                message: Some("tail: none (Claude runs have no herder-recorded event stream)".into()),
                            };
                        }
                        let attempt = run.launch_attempt().get();
                        let suffix = if attempt == 1 { String::new() } else { attempt.to_string() };
                        let path = dir.join(format!("events{suffix}.jsonl"));
                        let mut lines = vec![];
                        let message = match std::fs::read_to_string(&path) {
                            Ok(content) => {
                                lines = content
                                    .lines()
                                    .rev()
                                    .take(usize::from(count))
                                    .map(|line| match line.char_indices().nth(500) {
                                        Some((cut, _)) => format!("{} ...[+{} bytes]", &line[..cut], line.len() - cut),
                                        None => line.to_owned(),
                                    })
                                    .collect();
                                lines.reverse();
                                None
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                Some(format!("tail: stream does not exist yet: {}", path.display()))
                            }
                            Err(error) => Some(format!("tail: cannot read {}: {error}", path.display())),
                        };
                        Tail { path: Some(path), lines, message }
                    }),
                    dir,
                },
                _ => unreachable!(),
            };
            Ok((Output::Run { run: Box::new(run), observation, detail }, code))
        }
    }
}
fn misuse(args: &Dispatch, handoff: bool) -> Option<&'static str> {
    if args.executor != Executor::Claude && args.profile.is_some() {
        Some("--profile applies only to Claude")
    } else if args.model.len() < 2 {
        None
    } else if handoff {
        Some("a handoff continues one conversation; pass --model at most once")
    } else if args.mode != "review" {
        // Several writers in one worktree would edit the same files concurrently.
        Some("repeat --model only with --mode review; a writer runs one model")
    } else if args.model.iter().enumerate().any(|(i, model)| args.model[..i].contains(model)) {
        Some("each repeated --model must name a different model")
    } else {
        None
    }
}
fn dispatch(mut args: Dispatch, store: &Store) -> Result<(Output, i32)> {
    if let Some(message) = misuse(&args, false) {
        return Ok((Output::Rejection(message.into()), 2));
    }
    let repo = store::repo(&args.repo)?;
    process::lint(&args.brief, &args.mode)?;
    let brief = args.brief.clone();
    if args.model.len() < 2 {
        let (launched, code) = launch_run(store, args, repo, Input::Brief(brief))?;
        return Ok((Output::Launch(launched), code));
    }
    let (mut runs, mut code, mut error) = (vec![], 0, None);
    for model in std::mem::take(&mut args.model) {
        let mut one = args.clone();
        one.name = args.name.as_ref().map(|name| format!("{name}-{model}"));
        one.model = vec![model.clone()];
        match launch_run(store, one, repo.clone(), Input::Brief(brief.clone())) {
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
fn handoff(args: Dispatch, store: &Store) -> Result<(Output, i32)> {
    if let Some(message) = misuse(&args, true) {
        return Ok((Output::Rejection(message.into()), 2));
    }
    let repo = store::repo(&args.repo)?;
    // Validate the continuation document before any staging or launch side effect.
    let document = match crate::handoff::document(&args.brief) {
        Ok(document) => document,
        Err(error) => return Ok((Output::Rejection(format!("{error:#}")), 2)),
    };
    let prompt = crate::handoff::prompt(&args.mode, &document);
    let (launched, code) = launch_run(store, args, repo, Input::Generated(prompt.into_bytes()))?;
    Ok((Output::Launch(launched), code))
}
enum Input {
    /// Dispatch byte-copies the already linted brief file into the run staging directory.
    Brief(PathBuf),
    /// Handoff stages the generated prompt as input.md and lints it inside the staging directory.
    Generated(Vec<u8>),
}
/// Launches one run; `misuse` has already limited `args.model` to at most one entry.
fn launch_run(store: &Store, args: Dispatch, repo: PathBuf, input: Input) -> Result<(Launched, i32)> {
    let baseline_commit =
        process::checked(std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "HEAD"]))
            .ok()
            .map(|s| s.trim().to_owned());
    let mut dispatch_dirty = std::collections::BTreeMap::new();
    if baseline_commit.is_some() {
        for path in crate::verify::dirty_paths(&repo)? {
            let hash = crate::verify::content_hash(&repo, &path)?;
            dispatch_dirty.insert(path, hash);
        }
    }
    let (id, mut locked) = store.create()?;
    let dir = store.dir(&id)?;
    let input_md = locked.dir.join("input.md");
    match &input {
        Input::Brief(source) => {
            store::copy(source, &input_md)?;
        }
        Input::Generated(bytes) => {
            store::atomic(&input_md, bytes)?;
            if let Err(error) = process::lint(&input_md, &args.mode) {
                // Rejected handoff input must leave neither staging nor a published run behind.
                let _ = std::fs::remove_dir_all(&locked.dir);
                return Err(error);
            }
        }
    }
    let target = match args.executor {
        Executor::Claude => Target::Claude { profile: process::profile(args.profile)? },
        Executor::Vibe => Target::Vibe,
        Executor::Codex => Target::Codex,
    };
    let mut run = Run {
        name: args.name.unwrap_or_else(|| id.clone()),
        id,
        model: args.model.into_iter().next(),
        extra: args.extra,
        repo,
        baseline_commit,
        dispatch_dirty,
        brief: dir.join("input.md"),
        goal: args.goal,
        launch: Launch::Starting { target },
        attempt: NonZeroU32::MIN,
        request_attempts: Default::default(),
        authorized_paths: args.paths,
        verifications: vec![],
        created_at: store::timestamp(),
    };
    store.publish(&mut locked, &run)?;
    let result = runs::launch(&mut run, &dir, None, &locked);
    let error = result.err().map(|e| format!("{e:#}"));
    let pane_error = if args.pane && error.is_none() {
        runs::pane(&run).err().map(|e| format!("worker recorded; pane failed: {e:#}"))
    } else {
        None
    };
    let code = if error.is_some() || pane_error.is_some() { 1 } else { 0 };
    Ok((Launched { run: Box::new(run), launch_error: error, pane_error }, code))
}

fn wait(store: &Store, ids: &[String], all: bool, timeout: u64, json_output: bool) -> Result<(Output, i32)> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout);
    let mut previous = vec![None; ids.len()];
    loop {
        let mut data = Vec::with_capacity(ids.len());
        let (mut settled, mut unknown) = (0, false);
        for (index, id) in ids.iter().enumerate() {
            // The guard is dropped at the end of this iteration, before sleeping.
            let locked = store.try_lock(id)?;
            let mut run = store.load(id)?;
            let observation = runs::observe(&mut run, locked.as_ref());
            let state = observation.state();
            if previous[index] != Some(state) {
                if !json_output {
                    crate::render::wait_progress(&run, state);
                }
                previous[index] = Some(state);
            }
            settled += usize::from(matches!(state, State::Finished | State::Failed | State::Stopped | State::Blocked));
            unknown |= state == State::Unknown;
            let report_path = store.dir(id)?.join(run.report_name());
            data.push(WaitEntry { id: run.id, name: run.name, state, report_path });
        }
        let met = if all { settled == ids.len() } else { settled > 0 };
        // A status query can time out under load; only an unknown that outlasts the wait is reported.
        let message = if met {
            None
        } else if started.elapsed() >= timeout {
            Some(if unknown { "unknown state at timeout: inspect run evidence" } else { "wait timed out" })
        } else {
            std::thread::sleep(Duration::from_secs(5).min(timeout.saturating_sub(started.elapsed())));
            continue;
        };
        return Ok((Output::Wait { runs: data, message }, i32::from(message.is_some())));
    }
}
