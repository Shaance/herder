use super::{CLI_PROTOCOL_ERROR, Companion};
use crate::ExpectedCheckout;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CheckoutMismatch {
    Root,
    Branch,
    Head,
}

impl CheckoutMismatch {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Root => {
                "The configured directory does not match the intended Git worktree root. Run checkout, set --repo to the intended root, and reconnect"
            }
            Self::Branch => {
                "The checkout branch differs from expected_checkout.branch. Run checkout and confirm the intended branch before dispatching"
            }
            Self::Head => {
                "The checkout HEAD differs from expected_checkout.head. Run checkout and confirm the intended commit before dispatching"
            }
        }
    }

    pub(super) fn response(self) -> Value {
        json!({"outcome": "checkout_mismatch", "run_id": null, "reason": self, "error": self.message()})
    }
}

#[derive(Deserialize)]
struct Checkout {
    configured_dir: PathBuf,
    repo: Option<PathBuf>,
    branch: Option<String>,
    head: Option<String>,
    checkout_state: CheckoutState,
    state_dir: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckoutState {
    Attached,
    Detached,
    Unborn,
    NonGit,
}

impl Companion {
    pub fn read_checkout(&self) -> Result<Value> {
        let checkout = self.checkout_info()?;
        Ok(json!({
            "configured_dir": checkout.configured_dir, "repo": checkout.repo, "branch": checkout.branch,
            "head": checkout.head, "checkout_state": checkout.checkout_state, "state_dir": checkout.state_dir,
            "herder": self.herder, "executor": "codex", "model": self.model, "effort": self.effort,
            "write_paths": self.write_paths,
        }))
    }

    pub(super) fn matches_checkout_root(&self, expected: &Path) -> Result<bool> {
        ensure!(expected.is_absolute(), "expected_checkout.repo must be an absolute worktree path");
        Ok(expected.canonicalize().ok().as_ref() == Some(&self.repo))
    }

    pub(super) fn checkout_mismatch(&self, expected: &ExpectedCheckout) -> Result<Option<CheckoutMismatch>> {
        if !self.matches_checkout_root(&expected.repo)? {
            return Ok(Some(CheckoutMismatch::Root));
        }
        let checkout = self.checkout_info()?;
        if checkout.repo.as_ref().is_some_and(|repo| *repo != checkout.configured_dir) {
            Ok(Some(CheckoutMismatch::Root))
        } else if expected.branch.as_ref().is_some_and(|branch| checkout.branch.as_ref() != Some(branch)) {
            Ok(Some(CheckoutMismatch::Branch))
        } else if expected.head.as_ref().is_some_and(|head| checkout.head.as_ref() != Some(head)) {
            Ok(Some(CheckoutMismatch::Head))
        } else {
            Ok(None)
        }
    }

    fn checkout_info(&self) -> Result<Checkout> {
        let reply = self.observation_cli(&["context".as_ref(), "--repo".as_ref(), self.repo.as_os_str()])?;
        ensure!(reply.ok, CLI_PROTOCOL_ERROR);
        let checkout: Checkout = reply.decode()?;
        let has_branch = checkout.branch.as_ref().is_some_and(|branch| !branch.is_empty());
        let has_head = checkout.head.as_ref().is_some_and(|head| !head.is_empty());
        let has_repo = checkout.repo.as_ref().is_some_and(|repo| repo.is_absolute() && self.repo.starts_with(repo));
        let shape_matches = match checkout.checkout_state {
            CheckoutState::Attached => has_repo && has_branch && has_head,
            CheckoutState::Detached => has_repo && checkout.branch.is_none() && has_head,
            CheckoutState::Unborn => has_repo && has_branch && checkout.head.is_none(),
            CheckoutState::NonGit => checkout.repo.is_none() && checkout.branch.is_none() && checkout.head.is_none(),
        };
        ensure!(
            checkout.configured_dir == self.repo && checkout.state_dir.is_absolute() && shape_matches,
            CLI_PROTOCOL_ERROR
        );
        Ok(checkout)
    }
}
