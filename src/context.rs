use crate::store;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{path::Path, process::Command};

#[derive(Serialize)]
pub struct Checkout {
    configured_dir: String,
    repo: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    checkout_state: CheckoutState,
    state_dir: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckoutState {
    Attached,
    Detached,
    Unborn,
    NonGit,
}

pub fn read(configured_dir: &Path, state_dir: &Path) -> Result<Checkout> {
    let configured_dir = store::directory(configured_dir)?;
    let path_text = |path: &Path| path.to_str().map(str::to_owned).context("checkout and state paths must be UTF-8");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&configured_dir)
            .env("LC_ALL", "C")
            .env_remove(crate::process::OSC7577_TOKEN)
            .args(args)
            .output()
            .context("run Git")
    };
    let text = |bytes| {
        let value = String::from_utf8(bytes).context("Git returned non-UTF-8 output")?;
        Ok::<_, anyhow::Error>(value.strip_suffix('\n').unwrap_or(&value).to_owned())
    };
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let configured = path_text(&configured_dir)?;
    let state_dir = path_text(state_dir)?;
    if !top.status.success() {
        if top.stderr.starts_with(b"fatal: not a git repository (or any ") {
            return Ok(Checkout {
                configured_dir: configured,
                repo: None,
                branch: None,
                head: None,
                checkout_state: CheckoutState::NonGit,
                state_dir,
            });
        }
        bail!("could not read the Git checkout");
    }
    let repo = store::directory(Path::new(&text(top.stdout)?))?;
    let branch = git(&["symbolic-ref", "--quiet", "HEAD"])?;
    let branch_ref = match branch.status.code() {
        Some(0) => Some(text(branch.stdout)?),
        Some(1) => None,
        _ => bail!("could not read the Git branch"),
    };
    let branch = branch_ref
        .as_deref()
        .map(|reference| reference.strip_prefix("refs/heads/").context("Git HEAD does not name a branch"))
        .transpose()?;
    let head = git(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    let (head, checkout_state) = if head.status.success() {
        (Some(text(head.stdout)?), if branch.is_some() { CheckoutState::Attached } else { CheckoutState::Detached })
    } else if let (Some(1), Some(reference)) = (head.status.code(), branch_ref.as_deref())
        && git(&["show-ref", "--verify", "--quiet", reference])?.status.code() == Some(1)
    {
        (None, CheckoutState::Unborn)
    } else {
        bail!("could not read the Git HEAD commit");
    };
    Ok(Checkout {
        configured_dir: configured,
        repo: Some(path_text(&repo)?),
        branch: branch.map(str::to_owned),
        head,
        checkout_state,
        state_dir,
    })
}
