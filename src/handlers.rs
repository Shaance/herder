use crate::{
    Action, Dispatch, IssueAction,
    domain::*,
    issues, process, runs,
    store::{self, Store},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::num::NonZeroU32;

pub fn handle(action: Action) -> Result<(Value, i32)> {
    let store = Store::new()?;
    match action {
        Action::Doctor { executor, profile } => crate::doctor::run(executor, profile),
        Action::Dispatch(args) => dispatch(args, &store),
        Action::Ls { goal } => {
            let mut data = vec![];
            for id in store.runs()? {
                let entry = (|| -> Result<_> {
                    let locked = store.try_lock(&id)?;
                    let mut run = store.load(&id)?;
                    if goal.is_some() && run.goal != goal {
                        return Ok(None);
                    }
                    let observation = runs::observe(&mut run, locked.as_ref());
                    Ok(Some(json!({"run":run,"observation":observation})))
                })();
                match entry {
                    Ok(Some(entry)) => data.push(entry),
                    Ok(None) => {}
                    Err(error) => data.push(json!({"run_id":id,"error":format!("{error:#}")})),
                }
            }
            Ok((json!({"runs":data}), 0))
        }
        Action::Issue { command } => {
            let data = match command {
                IssueAction::Log { title, observed, expected, evidence, area, run } => {
                    if let Some(id) = &run {
                        store.load(id)?;
                    }
                    json!({"issue":issues::append(&store, issues::Event::Logged { id: issues::next_id(&store)?, title, observed, expected, evidence, area, run, timestamp: store::timestamp()? })?})
                }
                IssueAction::Ls { open } => json!({"issues":issues::list(&store, open)?}),
                IssueAction::Resolve { id, note } => {
                    json!({"issue":issues::resolve(&store, id, note)?})
                }
            };
            Ok((data, 0))
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
                Action::Resume { brief, .. } => {
                    runs::resume(&mut run, &brief, locked.as_ref().expect("resume acquired lock"))?;
                    observation = runs::observe(&mut run, locked.as_ref());
                }
                Action::Verify { paths, checks, .. } => {
                    if paths.is_empty() && run.authorized_paths.is_empty() {
                        return Ok((
                            json!({"message":"verify requires --path when none were recorded at dispatch"}),
                            2,
                        ));
                    }
                    drop(locked.take());
                    crate::verify::run(&mut run, paths, checks, &store)?;
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
    process::lint(&args.brief)?;
    let baseline_commit =
        process::checked(std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "HEAD"]))
            .ok()
            .map(|s| s.trim().to_owned());
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
        repo,
        baseline_commit,
        brief: dir.join("input.md"),
        goal: args.goal,
        launch: Launch::Starting { target },
        attempt: NonZeroU32::MIN,
        authorized_paths: args.paths,
        verifications: vec![],
        created_at: store::timestamp()?,
    };
    store.publish(&mut locked, &run)?;
    let result = runs::launch(&mut run, &dir, None, &args.extra, &locked);
    let error = result.err().map(|e| format!("{e:#}"));
    let pane_error = if args.pane && error.is_none() {
        runs::pane(&run, &dir).err().map(|e| format!("worker recorded; pane failed: {e:#}"))
    } else {
        None
    };
    let code = if error.is_some() || pane_error.is_some() { 1 } else { 0 };
    Ok((json!({"run_id":run.id,"run":run,"launch_error":error,"pane_error":pane_error}), code))
}
