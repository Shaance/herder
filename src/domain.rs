use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, path::PathBuf};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    Claude,
    Vibe,
    Codex,
}
impl Executor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Vibe => "vibe",
            Self::Codex => "codex",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "executor", rename_all = "snake_case")]
pub enum Target {
    Claude { profile: Option<PathBuf> },
    Vibe,
    Codex,
}
impl Target {
    pub fn executor(&self) -> Executor {
        match self {
            Self::Claude { .. } => Executor::Claude,
            Self::Vibe => Executor::Vibe,
            Self::Codex => Executor::Codex,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "executor", rename_all = "snake_case")]
pub enum Native {
    Claude { id: String, session_id: String, profile: Option<PathBuf> },
    Vibe { session_id: String, pid: NonZeroU32 },
    Codex { thread_id: String, pid: NonZeroU32 },
}
impl Native {
    pub fn target(&self) -> Target {
        match self {
            Self::Claude { profile, .. } => Target::Claude { profile: profile.clone() },
            Self::Vibe { .. } => Target::Vibe,
            Self::Codex { .. } => Target::Codex,
        }
    }
    pub fn process(&self) -> Option<(NonZeroU32, &str, &str)> {
        match self {
            Self::Vibe { session_id, pid } => Some((*pid, "session-id", session_id)),
            Self::Codex { thread_id, pid } => Some((*pid, "thread-id", thread_id)),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchError {
    pub message: String,
    pub remediation: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Intent {
    #[serde(alias = "starting")]
    Running,
    Unconfirmed {
        error: LaunchError,
    },
    Failed {
        error: LaunchError,
    },
    Stopped,
    StopSent,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Launch {
    Starting { target: Target },
    PendingResume { previous: Native, attempt: NonZeroU32, error: Option<LaunchError> },
    Accepted { id: String, profile: Option<PathBuf>, error: Option<LaunchError> },
    Failed { target: Target, error: LaunchError },
    Launched { native: Native, intent: Intent },
}
impl Launch {
    pub fn target(&self) -> Target {
        match self {
            Self::Starting { target } | Self::Failed { target, .. } => target.clone(),
            Self::Accepted { profile, .. } => Target::Claude { profile: profile.clone() },
            Self::PendingResume { previous, .. } => previous.target(),
            Self::Launched { native, .. } => native.target(),
        }
    }
    pub fn native(&self) -> Option<&Native> {
        match self {
            Self::Launched { native, .. } => Some(native),
            _ => None,
        }
    }
    pub fn state(&self) -> State {
        match self {
            Self::Starting { .. } => State::Starting,
            Self::Failed { .. } | Self::Launched { intent: Intent::Failed { .. }, .. } => State::Failed,
            Self::Launched { intent: Intent::Stopped, .. } => State::Stopped,
            Self::Accepted { .. } | Self::PendingResume { .. } => State::Unknown,
            Self::Launched { .. } => State::Running,
        }
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Running,
    Blocked,
    Finished,
    Failed,
    Stopped,
    Unknown,
}
impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
}
impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Verification {
    pub verdict: Verdict,
    pub checks: Vec<Check>,
    pub timestamp: String,
    #[serde(default = "first_attempt")]
    pub attempt: NonZeroU32,
}
pub struct FreshVerification {
    pub verdict: Verdict,
    pub scope: Check,
    pub commands: Vec<Check>,
    pub attempt: NonZeroU32,
}
fn first_attempt() -> NonZeroU32 {
    NonZeroU32::MIN
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub extra: Vec<String>,
    pub repo: PathBuf,
    pub baseline_commit: Option<String>,
    #[serde(default)]
    pub dispatch_dirty: std::collections::BTreeMap<String, Option<String>>,
    pub brief: PathBuf,
    pub goal: Option<String>,
    pub name: String,
    pub launch: Launch,
    pub attempt: NonZeroU32,
    #[serde(default)]
    pub request_attempts: std::collections::BTreeMap<String, NonZeroU32>,
    pub authorized_paths: Vec<String>,
    pub verifications: Vec<Verification>,
    pub created_at: String,
}
impl Run {
    pub fn launch_attempt(&self) -> NonZeroU32 {
        match self.launch {
            Launch::PendingResume { attempt, .. } => attempt,
            _ => self.attempt,
        }
    }
    pub fn suffix(&self) -> String {
        if self.attempt.get() == 1 { String::new() } else { self.attempt.to_string() }
    }
    pub fn report_name(&self) -> String {
        format!("final{}.md", self.suffix())
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownCause {
    QueryFailed,
    IdentityMismatch,
    RecordDisagrees,
    UnrecognizedStatus,
    Unconfirmed,
}
impl UnknownCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueryFailed => "query_failed",
            Self::IdentityMismatch => "identity_mismatch",
            Self::RecordDisagrees => "record_disagrees",
            Self::UnrecognizedStatus => "unrecognized_status",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}
#[derive(Debug)]
pub struct UnknownError(pub UnknownCause, pub String);
impl std::fmt::Display for UnknownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for UnknownError {}

#[derive(Debug, Serialize)]
pub struct Observation {
    state: State,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<UnknownCause>,
}
impl Observation {
    pub fn new(state: State, evidence: String, unknown_cause: UnknownCause) -> Self {
        Self { state, evidence, cause: (state == State::Unknown).then_some(unknown_cause) }
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn cause(&self) -> Option<UnknownCause> {
        self.cause
    }
}
impl std::fmt::Display for Observation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "state {:?}\n{}", self.state, self.evidence.trim_end())
    }
}
pub fn reconcile(recorded: State, observed: Observation) -> Observation {
    let agrees = match recorded {
        State::Running => true,
        State::Starting => observed.state == State::Starting,
        State::Stopped => matches!(observed.state, State::Stopped | State::Finished | State::Failed),
        _ => recorded == observed.state,
    };
    if agrees {
        Observation { state: if recorded == State::Stopped { State::Stopped } else { observed.state }, ..observed }
    } else {
        Observation::new(
            State::Unknown,
            format!("recorded={recorded:?}; executor={:?}: {}", observed.state, observed.evidence),
            if recorded == State::Starting { UnknownCause::Unconfirmed } else { UnknownCause::RecordDisagrees },
        )
    }
}
