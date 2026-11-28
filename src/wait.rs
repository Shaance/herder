use crate::domain::{Intent, Launch, Native, Observation, Run, State, Target, UnknownCause};
use crate::output::{Output, WaitEntry};
use crate::process::ScriptSource;
use crate::runs;
use crate::store::{self, RunId, ScopedNotFound, Store};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

pub(crate) fn wait(
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
