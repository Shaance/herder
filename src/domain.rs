use serde::{Deserialize, Serialize};
use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    Claude,
    Vibe,
    Codex,
    Agy,
}
impl Executor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Vibe => "vibe",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "executor", rename_all = "snake_case")]
pub enum Target {
    Claude { profile: Option<PathBuf> },
    Vibe,
    Codex,
    Agy,
}
impl Target {
    pub fn executor(&self) -> Executor {
        match self {
            Self::Claude { .. } => Executor::Claude,
            Self::Vibe => Executor::Vibe,
            Self::Codex => Executor::Codex,
            Self::Agy => Executor::Agy,
        }
    }
    pub fn profile(&self) -> Option<&Path> {
        match self {
            Self::Claude { profile } => profile.as_deref(),
            Self::Vibe | Self::Codex | Self::Agy => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "executor", rename_all = "snake_case")]
pub enum Native {
    Claude { id: String, session_id: String, profile: Option<PathBuf> },
    Vibe { session_id: String, pid: NonZeroU32 },
    Codex { thread_id: String, pid: NonZeroU32 },
    Agy { conversation_id: String, pid: NonZeroU32 },
}
impl Native {
    pub fn target(&self) -> Target {
        match self {
            Self::Claude { profile, .. } => Target::Claude { profile: profile.clone() },
            Self::Vibe { .. } => Target::Vibe,
            Self::Codex { .. } => Target::Codex,
            Self::Agy { .. } => Target::Agy,
        }
    }
    pub fn process(&self) -> Option<(NonZeroU32, &str, &str)> {
        match self {
            Self::Vibe { session_id, pid } => Some((*pid, "session-id", session_id)),
            Self::Codex { thread_id, pid } => Some((*pid, "thread-id", thread_id)),
            Self::Agy { conversation_id, pid } => Some((*pid, "conversation-id", conversation_id)),
            _ => None,
        }
    }
}
pub enum LaunchPhase<'a> {
    Initial,
    Resume { previous: &'a Native, delta: &'a std::path::Path },
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
    pub reused_from: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtectedVerification {
    pub attempt: NonZeroU32,
    pub timestamp: String,
    pub scope: Check,
    pub host_check: Option<Check>,
    pub verdict: Verdict,
}
fn first_attempt() -> NonZeroU32 {
    NonZeroU32::MIN
}

fn no_staged_paths(staged: &Option<std::collections::BTreeMap<String, StagedState>>) -> bool {
    staged.as_ref().is_none_or(|staged| staged.is_empty())
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedState {
    MatchesHead,
    // Mode, object ID, and stage for each index entry. Empty for a staged deletion.
    IndexEntries(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub requested_effort: Option<String>,
    #[serde(default)]
    pub extra: Vec<String>,
    pub repo: PathBuf,
    pub baseline_commit: Option<String>,
    #[serde(default)]
    pub dispatch_dirty: std::collections::BTreeMap<String, Option<String>>,
    /// Staged state relative to HEAD for each `dispatch_dirty` path; absent in older records.
    /// Empty is omitted: with no dirty paths there is nothing to exempt either way.
    #[serde(default, skip_serializing_if = "no_staged_paths")]
    pub dispatch_staged: Option<std::collections::BTreeMap<String, StagedState>>,
    pub brief: PathBuf,
    pub goal: Option<String>,
    pub name: String,
    pub launch: Launch,
    pub attempt: NonZeroU32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_worker_dir: Option<PathBuf>,
    #[serde(default)]
    pub request_attempts: std::collections::BTreeMap<String, NonZeroU32>,
    pub authorized_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_with: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_verification: Option<ProtectedVerification>,
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
    pub fn confirm_identity(&mut self, native: Native) {
        self.attempt = self.launch_attempt();
        let intent = match &mut self.launch {
            Launch::Accepted { error, .. } => {
                error.take().map_or(Intent::Running, |error| Intent::Unconfirmed { error })
            }
            _ => Intent::Running,
        };
        self.launch = Launch::Launched { native, intent };
    }
    pub fn suffix(&self) -> String {
        if self.attempt.get() == 1 { String::new() } else { self.attempt.to_string() }
    }
    pub fn launch_events_name(&self) -> String {
        let attempt = self.launch_attempt();
        let suffix = if attempt == NonZeroU32::MIN { String::new() } else { attempt.to_string() };
        format!("events{suffix}.jsonl")
    }
    pub fn launch_report_name(&self) -> String {
        match self.launch {
            Launch::PendingResume { attempt, .. } => format!("final{attempt}.md"),
            _ => self.report_name(),
        }
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
    evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<UnknownCause>,
}
impl Observation {
    pub fn known(state: State, evidence: String) -> Self {
        assert_ne!(state, State::Unknown);
        Self { state, evidence, cause: None }
    }
    pub fn unknown(cause: UnknownCause, evidence: String) -> Self {
        Self { state: State::Unknown, evidence, cause: Some(cause) }
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
    pub fn prefix_evidence(&mut self, prefix: &str) {
        self.evidence.insert_str(0, prefix);
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
    match recorded {
        State::Running => observed,
        State::Stopped if matches!(observed.state, State::Finished | State::Failed) => {
            Observation::known(State::Stopped, observed.evidence)
        }
        _ if recorded == observed.state => observed,
        _ => Observation::unknown(
            if recorded == State::Starting { UnknownCause::Unconfirmed } else { UnknownCause::RecordDisagrees },
            format!("recorded={recorded:?}; executor={:?}: {}", observed.state, observed.evidence),
        ),
    }
}
