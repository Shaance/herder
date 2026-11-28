use crate::domain::{FreshVerification, Observation, Run, State};
use serde::Serialize;
use std::{collections::BTreeMap, num::NonZeroU32, path::PathBuf};

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
    Run { run: Box<Run>, observation: Observation, observed_attempt: NonZeroU32 },
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
