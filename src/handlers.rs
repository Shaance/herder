use crate::{
    Action, Dispatch,
    domain::*,
    process, runs,
    store::{self, Store},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::num::NonZeroU32;

pub fn handle(mut action: Action, json_output: bool) -> Result<(Value, i32)> {
    let store = Store::new()?;
    match &mut action {
        Action::Status { run }
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
        Action::Doctor { executor, profile } => crate::doctor::run(executor, profile),
        Action::Dispatch(args) => dispatch(args, &store),
        Action::Wait { runs, all, timeout } => wait(&store, &runs, all, timeout, json_output),
        Action::Ls { goal } => {
            let mut summary = std::collections::BTreeMap::<String, std::collections::BTreeMap<String, usize>>::new();
            let mut data = vec![];
            for id in store.runs()? {
                let entry = (|| -> Result<_> {
                    let locked = store.try_lock(&id)?;
                    let mut run = store.load(&id)?;
                    if goal.is_some() && run.goal != goal {
                        return Ok(None);
                    }
                    let observation = runs::observe(&mut run, locked.as_ref());
                    let executor = serde_json::to_value(run.launch.target())?["executor"].as_str().unwrap().to_owned();
                    let state = serde_json::to_value(observation.state)?.as_str().unwrap().to_owned();
                    *summary.entry(executor).or_default().entry(state).or_default() += 1;
                    Ok(Some(json!({"run":run,"observation":observation})))
                })();
                match entry {
                    Ok(Some(entry)) => data.push(entry),
                    Ok(None) => {}
                    Err(error) => data.push(json!({"run_id":id,"error":format!("{error:#}")})),
                }
            }
            Ok((json!({"runs":data,"summary":summary}), 0))
        }
        action => {
            let id = match &action {
                Action::Status { run }
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
            let mut run = store.load(id)?;
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
                Observation { state: run.launch.state(), evidence: String::new() }
            } else {
                runs::observe(&mut run, locked.as_ref())
            };
            let mut code = 0;
            let mut report = Value::Null;
            match action {
                Action::Report { .. } => {
                    let path = store.dir(&run.id)?.join(run.report_name());
                    match store::report(&path)? {
                        Some(content) => report = json!({"path":path,"content":content}),
                        None => {
                            code = 1;
                            report = json!({"path":path,"message":"no report yet"});
                        }
                    }
                }
                Action::Stop { .. } => {
                    observation = runs::stop(&mut run, locked.as_ref().expect("stop acquired lock"))?;
                }
                Action::Resume { brief, request_id, .. } => {
                    runs::resume(
                        &mut run,
                        &brief,
                        request_id.as_deref(),
                        locked.as_ref().expect("resume acquired lock"),
                    )?;
                    observation = runs::observe(&mut run, locked.as_ref());
                }
                Action::Verify { paths, checks, .. } => {
                    if matches!(observation.state, State::Starting | State::Running | State::Blocked) {
                        return Ok((
                            json!({"message":format!("cannot verify worker in state {}", serde_json::to_value(observation.state)?.as_str().unwrap())}),
                            2,
                        ));
                    }
                    if paths.is_empty() && run.authorized_paths.is_empty() {
                        return Ok((
                            json!({"message":"verify requires --path when none were recorded at dispatch"}),
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
                }
                _ => {}
            }
            Ok((json!({"run":run,"observation":observation,"report":report}), code))
        }
    }
}
fn dispatch(args: Dispatch, store: &Store) -> Result<(Value, i32)> {
    if args.executor != Executor::Claude && args.profile.is_some() {
        return Ok((json!({"message":"--profile applies only to Claude"}), 2));
    }
    let repo = store::repo(&args.repo)?;
    process::lint(&args.brief, &args.mode)?;
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
    store::copy(&args.brief, &locked.dir.join("input.md"))?;
    let target = match args.executor {
        Executor::Claude => Target::Claude { profile: process::profile(args.profile)? },
        Executor::Vibe => Target::Vibe,
        Executor::Codex => Target::Codex,
    };
    let mut run = Run {
        name: args.name.unwrap_or_else(|| id.clone()),
        id,
        model: args.model,
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
    Ok((json!({"run_id":run.id,"run":run,"launch_error":error,"pane_error":pane_error}), code))
}

fn wait(store: &Store, ids: &[String], all: bool, timeout: u64, json_output: bool) -> Result<(Value, i32)> {
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
            let state = observation.state;
            if previous[index] != Some(state) {
                if !json_output {
                    println!(
                        "{}\t{}\t{}",
                        run.id,
                        run.name.escape_debug(),
                        serde_json::to_value(state)?.as_str().unwrap()
                    );
                }
                previous[index] = Some(state);
            }
            settled += usize::from(matches!(state, State::Finished | State::Failed | State::Stopped | State::Blocked));
            unknown |= state == State::Unknown;
            data.push(
                json!({"id":run.id,"name":run.name,"state":state,"report_path":store.dir(id)?.join(run.report_name())}),
            );
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
        return Ok((json!({"runs":data,"message":message}), i32::from(message.is_some())));
    }
}
