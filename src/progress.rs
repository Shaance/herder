use crate::{
    domain::{Executor, Observation, Run, State, UnknownCause},
    store,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    num::NonZeroU32,
    path::Path,
    time::SystemTime,
};

#[derive(Serialize)]
pub struct Progress {
    pub attempt: NonZeroU32,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<UnknownCause>,
}

pub fn last(run: &Run, observation: &Observation, dir: &Path, refreshed: bool) -> Progress {
    let attempt = run.launch_attempt();
    let executor = run.launch.target().executor();
    let (kind, modified) = if executor == Executor::Claude {
        match store::nonempty_report_metadata(&dir.join(run.launch_report_name())) {
            Some(report) => ("report_updated", report.modified().ok()),
            _ if refreshed && observation.state() == State::Running && run.launch.native().is_some() => {
                ("executor_reports_working", None)
            }
            _ => ("no_progress_observed", None),
        }
    } else {
        stream(&dir.join(run.launch_events_name()), executor).unwrap_or(("no_progress_observed", None))
    };
    let (updated_at, age_seconds) = modified
        .filter(|time| *time >= SystemTime::UNIX_EPOCH)
        .and_then(|time| {
            let age = SystemTime::now().duration_since(time).ok()?;
            Some((store::timestamp_at(time), age.as_secs()))
        })
        .map_or((None, None), |(stamp, age)| (Some(stamp), Some(age)));
    Progress { attempt, kind, updated_at, age_seconds, cause: observation.cause() }
}

pub fn codex_turn_completed(path: &Path) -> bool {
    EventTail::read(path).is_some_and(|events| {
        events
            .records()
            .filter_map(|event| match event["type"].as_str() {
                Some("turn.completed") => Some(true),
                Some("turn.started" | "turn.failed") => Some(false),
                _ => None,
            })
            .last()
            == Some(true)
    })
}

struct EventTail {
    bytes: Vec<u8>,
    starts_midstream: bool,
    modified: Option<SystemTime>,
}

impl EventTail {
    fn read(path: &Path) -> Option<Self> {
        const MAX_BYTES: u64 = 256 * 1024;
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() == 0 {
            return None;
        }
        let start = meta.len().saturating_sub(MAX_BYTES);
        let mut file = File::open(path).ok()?;
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
        Some(Self { bytes, starts_midstream: start != 0, modified: meta.modified().ok() })
    }

    fn records(&self) -> impl Iterator<Item = Value> + '_ {
        self.bytes
            .split_inclusive(|&byte| byte == b'\n')
            .skip(usize::from(self.starts_midstream))
            .filter(|line| line.ends_with(b"\n"))
            .filter_map(|line| serde_json::from_slice(line).ok())
    }
}

fn stream(path: &Path, executor: Executor) -> Option<(&'static str, Option<SystemTime>)> {
    let events = EventTail::read(path)?;
    let kind = events
        .records()
        .map(|event| match (executor, event["type"].as_str(), event["item"]["type"].as_str()) {
            (Executor::Codex, Some("thread.started"), _) => "session_opened",
            (Executor::Codex, Some("turn.started"), _) => "turn_started",
            (Executor::Codex, Some("turn.completed"), _) => "turn_completed",
            (Executor::Codex, Some("turn.failed"), _) => "turn_failed",
            (Executor::Codex, Some("item.started"), Some("command_execution")) => "command_started",
            (Executor::Codex, Some("item.completed"), Some("command_execution")) => "command_completed",
            (Executor::Codex, Some("item.started" | "item.updated" | "item.completed"), _) => "item_updated",
            (Executor::Vibe, Some("message"), _) => "message",
            (Executor::Vibe, Some("callback"), _) => "callback",
            (Executor::Vibe, None | Some("session_start" | "session_started"), _)
                if event["sessionId"].as_str().is_some() =>
            {
                "session_opened"
            }
            _ => "event_stream_updated",
        })
        .last()
        .unwrap_or("event_stream_updated");
    Some((kind, events.modified))
}
