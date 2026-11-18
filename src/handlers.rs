use crate::{
    Action, Dispatch,
    domain::*,
    process, runs,
    store::{self, RunId, Store},
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
    pub outcome: LaunchOutcome,
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
}

#[derive(Serialize)]
pub struct WaitEntry {
    pub id: String,
    pub name: String,
    pub state: State,
    pub report_path: PathBuf,
}

pub fn handle(action: Action, json_output: bool) -> Result<(Output, i32)> {
    let store = Store::new()?;
    let read = |selector: &str| -> Result<_> {
        let id = store.resolve(selector)?;
        let locked = store.try_lock(&id)?;
        let run = store.load(&id)?;
        Ok((id, locked, run))
    };
    let (run, observation, detail, code) = match action {
        Action::Doctor { executor, profile } => {
            return crate::doctor::run(executor, profile).map(|(doctor, code)| (Output::Doctor(doctor), code));
        }
        Action::Dispatch(args) => return dispatch(args, &store, false),
        Action::Handoff(args) => return dispatch(args, &store, true),
        Action::Wait { runs, all, timeout } => {
            let ids = runs.iter().map(|id| store.resolve(id)).collect::<Result<Vec<_>>>()?;
            return wait(&store, &ids, all, timeout, json_output);
        }
        Action::Ls { goal } => {
            let mut summary = BTreeMap::<&str, BTreeMap<&str, usize>>::new();
            let mut data = vec![];
            for id in store.runs()? {
                let id = match id {
                    Ok(id) => id,
                    Err(run_id) => {
                        data.push(ListEntry::Broken { run_id, error: "invalid run id".into() });
                        continue;
                    }
                };
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
                    Ok(entry) => data.extend(entry),
                    Err(error) => data.push(ListEntry::Broken { run_id: id.to_string(), error: format!("{error:#}") }),
                }
            }
            return Ok((Output::Ls { runs: data, summary }, 0));
        }
        Action::Status { run, tail } => {
            let (id, locked, mut run) = read(&run)?;
            let dir = store.dir(&id);
            let observation = runs::observe(&mut run, locked.as_ref());
            let tail = tail.map(|count| {
                if run.launch.target().executor() == Executor::Claude {
                    return Tail::NoStream("tail: none (Claude runs have no herder-recorded event stream)".into());
                }
                let attempt = run.launch_attempt().get();
                let suffix = if attempt == 1 { String::new() } else { attempt.to_string() };
                let path = dir.join(format!("events{suffix}.jsonl"));
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
        Action::Report { run } => {
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
            runs::resume(&mut run, &brief, request_id.as_deref(), &locked)?;
            let observation = runs::observe(&mut run, Some(&locked));
            (run, observation, RunDetail::Resume, 0)
        }
        Action::Verify { run, paths, checks, baseline } => {
            let (id, locked, mut run) = read(&run)?;
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
            let verification = crate::verify::run(&mut run, paths, checks, baseline, &store, &id)?;
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

fn dispatch(args: Dispatch, store: &Store, handoff: bool) -> Result<(Output, i32)> {
    let Dispatch { mode, executor, brief, repo, mut model, goal, name, profile, pane, paths, extra } = args;
    let reject = |message: String| Ok((Output::Rejection(message), 2));
    if executor != Executor::Claude && profile.is_some() {
        return reject("--profile applies only to Claude".into());
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
    let input = if handoff {
        // Validate the continuation document before any staging or launch side effect.
        let document = match crate::handoff::document(&brief) {
            Ok(document) => document,
            Err(error) => return reject(format!("{error:#}")),
        };
        Input::Generated(crate::handoff::prompt(&mode, &document).into_bytes())
    } else {
        process::lint(&brief, &mode)?;
        Input::Brief(brief)
    };
    let launch_run = |name: Option<String>, model: Option<String>| -> Result<_> {
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
        let dir = store.dir(&id);
        let input_md = locked.dir.join("input.md");
        match &input {
            Input::Brief(source) => {
                store::copy(source, &input_md)?;
            }
            Input::Generated(bytes) => {
                store::atomic(&input_md, bytes)?;
                if let Err(error) = process::lint(&input_md, &mode) {
                    // Rejected handoff input must leave neither staging nor a published run behind.
                    let _ = std::fs::remove_dir_all(&locked.dir);
                    return Err(error);
                }
            }
        }
        let target = match executor {
            Executor::Claude => Target::Claude { profile: process::profile(profile.clone())? },
            Executor::Vibe => Target::Vibe,
            Executor::Codex => Target::Codex,
        };
        let id = id.to_string();
        let mut run = Run {
            name: name.unwrap_or_else(|| id.clone()),
            id,
            model,
            extra: extra.clone(),
            repo: repo.clone(),
            baseline_commit,
            dispatch_dirty,
            brief: dir.join("input.md"),
            goal: goal.clone(),
            launch: Launch::Starting { target },
            attempt: NonZeroU32::MIN,
            request_attempts: Default::default(),
            authorized_paths: paths.clone(),
            verifications: vec![],
            created_at: store::timestamp(),
        };
        store.publish(&mut locked, &run, &dir)?;
        let outcome = match runs::launch(&mut run, &dir, None, &locked) {
            Err(error) => LaunchOutcome::LaunchFailed(format!("{error:#}")),
            Ok(()) if pane => match runs::pane(&run) {
                Ok(()) => LaunchOutcome::Started,
                Err(error) => LaunchOutcome::PaneFailed(format!("worker recorded; pane failed: {error:#}")),
            },
            Ok(()) => LaunchOutcome::Started,
        };
        let code = i32::from(!matches!(outcome, LaunchOutcome::Started));
        Ok((Launched { run: Box::new(run), outcome }, code))
    };
    let models = match models {
        Models::Single(model) => {
            return launch_run(name, model).map(|(launched, code)| (Output::Launch(launched), code));
        }
        Models::Review(models) => models,
    };
    let (mut runs, mut code, mut error) = (vec![], 0, None);
    for model in models {
        let name = name.as_ref().map(|name| format!("{name}-{model}"));
        match launch_run(name, Some(model.clone())) {
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

enum Input {
    /// Dispatch byte-copies the already linted brief file into the run staging directory.
    Brief(PathBuf),
    /// Handoff stages the generated prompt as input.md and lints it inside the staging directory.
    Generated(Vec<u8>),
}

fn wait(store: &Store, ids: &[RunId], all: bool, timeout: u64, json_output: bool) -> Result<(Output, i32)> {
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
            let report_path = store.dir(id).join(run.report_name());
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
