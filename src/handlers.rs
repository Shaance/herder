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
            for mut run in store.runs()?.into_iter().filter(|r| goal.is_none() || r.goal == goal) {
                let observation = runs::observe(&mut run, &store);
                data.push(json!({"run":run,"observation":observation}));
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
                IssueAction::Resolve { id, note } => json!({"issue":issues::resolve(&store, id, note)?}),
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
            let mut run = store.load(id)?;
            let mut observation = runs::observe(&mut run, &store);
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
                    observation = runs::stop(&mut run, &store)?;
                    if observation.state == State::Unknown {
                        code = 1;
                    }
                }
                Action::Resume { brief, .. } => {
                    runs::resume(&mut run, &brief, &store)?;
                    observation = runs::observe(&mut run, &store);
                }
                Action::Verify { paths, checks, .. } => {
                    if paths.is_empty() && run.authorized_paths.is_empty() {
                        return Ok((
                            json!({"message":"verify requires --path when none were recorded at dispatch"}),
                            2,
                        ));
                    }
                    crate::verify::run(&mut run, paths, checks, &store)?;
                    if matches!(run.verifications.last().map(|v| &v.verdict), Some(Verdict::Fail)) {
                        code = 1;
                    }
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
    let (id, dir) = store.create()?;
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
        brief: store::copy(&args.brief, &dir.join("input.md"))?,
        goal: args.goal,
        launch: Launch::Starting { target },
        attempt: NonZeroU32::MIN,
        authorized_paths: args.paths,
        verifications: vec![],
        created_at: store::timestamp()?,
    };
    store.save(&run)?;
    let result = runs::launch(&mut run, &dir, None, &args.extra, store);
    let error = result.err().map(|e| format!("{e:#}"));
    let pane_error = if args.pane && error.is_none() {
        runs::pane(&run, &dir).err().map(|e| format!("worker recorded; pane failed: {e:#}"))
    } else {
        None
    };
    let code = if error.is_some() || pane_error.is_some() { 1 } else { 0 };
    Ok((json!({"run_id":run.id,"run":run,"launch_error":error,"pane_error":pane_error}), code))
}
