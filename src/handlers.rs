use crate::process::{self, ScriptSource};
use crate::store::{self, ScopedNotFound, Store};
use crate::{Action, domain::*, output::*, runs, writer_slot::Slot};
use anyhow::Result;
use std::collections::BTreeMap;

pub fn handle(action: Action, json_output: bool) -> Result<(Output, i32)> {
    if let Action::Wait { run_dir: Some(dir), all, timeout, .. } = &action {
        let (store, id) = Store::from_run_dir(dir)?;
        let run = store.load(&id)?;
        if !matches!(run.launch, Launch::Failed { .. }) && !matches!(run.launch.target(), Target::Claude { .. }) {
            crate::scripts::require_status_tools(ScriptSource::Embedded)?;
        }
        return crate::wait::wait(&store, &[id], None, *all, *timeout, json_output, ScriptSource::Embedded);
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
            return crate::dispatch::dispatch(args, &store, false, checks, check_timeout);
        }
        Action::Handoff(args) => return crate::dispatch::dispatch(args, &store, true, vec![], None),
        Action::Wait { runs, all, timeout, .. } => {
            if repo.is_some() && timeout == 0 {
                return Ok((Output::Rejection("wait --repo requires a nonzero --timeout".into()), 2));
            }
            let ids = runs.iter().map(|id| store.observation_id(id, repo.is_some())).collect::<Result<Vec<_>>>()?;
            return crate::wait::wait(&store, &ids, repo, all, timeout, json_output, ScriptSource::Configured);
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
                    let observed_attempt = run.launch_attempt();
                    Ok(Some(ListEntry::Run { run: Box::new(run), observation, observed_attempt }))
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
