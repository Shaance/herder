use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, path::PathBuf, str::FromStr};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    Claude,
    Vibe,
    Codex,
}
impl FromStr for Executor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(Self::Claude),
            "vibe" => Ok(Self::Vibe),
            "codex" => Ok(Self::Codex),
            _ => Err("expected claude, vibe, or codex".into()),
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
    Starting,
    Running,
    Unconfirmed { error: LaunchError },
    Failed { error: LaunchError },
    Stopped,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Launch {
    Starting { target: Target },
    Accepted { id: String, profile: Option<PathBuf>, error: Option<LaunchError> },
    Failed { target: Target, error: LaunchError },
    Launched { native: Native, intent: Intent },
}
impl Launch {
    pub fn target(&self) -> Target {
        match self {
            Self::Starting { target } | Self::Failed { target, .. } => target.clone(),
            Self::Accepted { profile, .. } => Target::Claude { profile: profile.clone() },
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
            Self::Starting { .. } | Self::Launched { intent: Intent::Starting, .. } => State::Starting,
            Self::Failed { .. } | Self::Launched { intent: Intent::Failed { .. }, .. } => State::Failed,
            Self::Launched { intent: Intent::Stopped, .. } => State::Stopped,
            Self::Accepted { .. } => State::Unknown,
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
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
}
#[derive(Debug, Serialize, Deserialize)]
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
    pub attempt: NonZeroU32,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub model: Option<String>,
    pub repo: PathBuf,
    pub brief: PathBuf,
    pub goal: Option<String>,
    pub name: String,
    pub launch: Launch,
    pub attempt: NonZeroU32,
    pub authorized_paths: Vec<String>,
    pub verifications: Vec<Verification>,
    pub created_at: String,
}
impl Run {
    pub fn suffix(&self) -> String {
        if self.attempt.get() == 1 { String::new() } else { self.attempt.to_string() }
    }
    pub fn report_name(&self) -> String {
        format!("final{}.md", self.suffix())
    }
}
#[derive(Debug, Serialize)]
pub struct Observation {
    pub state: State,
    pub evidence: String,
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
        Observation {
            state: State::Unknown,
            evidence: format!("recorded={recorded:?}; executor={:?}: {}", observed.state, observed.evidence),
        }
    }
}
