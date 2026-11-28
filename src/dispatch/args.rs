use crate::{DispatchArgs, domain::Executor};

pub(super) enum Models {
    Single(Option<String>),
    Review(Vec<String>),
}

fn codex_config_assignments(extra: &[String]) -> impl Iterator<Item = (&str, &str)> {
    let mut args = extra.iter().map(String::as_str);
    std::iter::from_fn(move || {
        while let Some(arg) = args.next() {
            let assignment = match arg {
                "--" => break,
                "-c" | "--config" => args.next(),
                _ => arg
                    .strip_prefix("--config=")
                    .or_else(|| arg.strip_prefix("-c").map(|value| value.strip_prefix('=').unwrap_or(value))),
            };
            if let Some((key, value)) = assignment.and_then(|value| value.split_once('=')) {
                return Some((key.trim(), value.trim()));
            }
        }
        None
    })
}

pub(super) fn validate(
    args: &mut DispatchArgs,
    handoff: bool,
    checks: &[String],
) -> Result<(Vec<String>, Models), String> {
    let mut model = std::mem::take(&mut args.model);
    let mut extra = std::mem::take(&mut args.extra);
    let DispatchArgs {
        ref mode,
        executor,
        ref effort,
        ref profile,
        pane,
        ref request_id,
        ref accept_with,
        ref expected_head,
        ref expected_branch,
        ..
    } = *args;
    if !checks.is_empty() && model.len() > 1 {
        return Err("--check requires a single dispatch".into());
    }
    if request_id.is_some() && (handoff || pane || !checks.is_empty() || model.len() > 1) {
        return Err("--request-id requires one dispatch without --pane or --check".into());
    }
    if (expected_head.is_some() || expected_branch.is_some())
        && accept_with.is_none()
        && (mode != "review" || handoff || model.len() > 1)
    {
        return Err("checkout pins require a single review dispatch or --accept-with".into());
    }
    if accept_with.is_some() {
        if mode != "direct" || executor != Executor::Codex || !extra.is_empty() {
            return Err("--accept-with requires a direct Codex dispatch without native extra arguments".into());
        }
        extra.extend(["--sandbox".into(), "workspace-write".into()]);
    }
    if checks.iter().any(|check| check.trim().is_empty()) {
        return Err("--check requires a nonempty shell command".into());
    }
    if executor != Executor::Claude && profile.is_some() {
        return Err("--profile applies only to Claude".into());
    }
    if effort.is_some() {
        let conflict = match executor {
            Executor::Vibe => {
                return Err(
                    "Vibe has no per-run effort; effort comes from the model alias's `thinking` setting in Vibe's config.toml, choose an alias with --model"
                        .into(),
                );
            }
            Executor::Agy => {
                return Err(
                    "agy has no per-run --effort; choose an effort in the --model slug, such as gemini-4-neon-high"
                        .into(),
                );
            }
            Executor::Codex => codex_config_assignments(&extra).any(|(key, _)| key == "model_reasoning_effort"),
            Executor::Claude => extra.iter().any(|arg| arg == "--effort" || arg.starts_with("--effort=")),
        };
        if conflict {
            return Err("--effort conflicts with native effort arguments after --".into());
        }
    }
    if executor == Executor::Agy {
        if !extra.is_empty() {
            return Err("agy does not accept extra arguments after --".into());
        }
        if model.is_empty() || model.iter().any(|slug| slug.trim().is_empty()) {
            return Err("agy requires --model with a model slug, such as gemini-4-neon-high".into());
        }
        if mode == "review" {
            extra.extend(["--mode".into(), "plan".into()]);
        }
    }
    if executor == Executor::Codex && mode == "review" {
        if codex_config_assignments(&extra).any(|(key, value)| {
            key == "sandbox_mode" && !matches!(value, "read-only" | r#""read-only""# | "'read-only'")
        }) {
            return Err("Codex review requires sandbox_mode to be read-only".into());
        }
        let mut sandbox_specified = false;
        let mut args = extra.iter_mut();
        while let Some(arg) = args.next() {
            let value = match arg.as_str() {
                "--" => return Err("Codex review extras cannot include a second -- separator".into()),
                "--dangerously-bypass-approvals-and-sandbox"
                | "--yolo"
                | "--full-auto"
                | "--approve-for-me"
                | "--not-so-yolo" => {
                    return Err(format!("Codex review requires a read-only sandbox and cannot use {arg}"));
                }
                "--sandbox" | "-s" => args.next().map(|value| value.as_str()),
                flag if flag.starts_with("--sandbox=") => flag.strip_prefix("--sandbox="),
                flag if flag.starts_with("-s") => {
                    flag.strip_prefix("-s").map(|value| value.strip_prefix('=').unwrap_or(value))
                }
                _ => continue,
            };
            if sandbox_specified {
                return Err("Codex review accepts only one explicit sandbox flag".into());
            }
            if value != Some("read-only") {
                return Err("Codex review requires --sandbox read-only".into());
            }
            if arg.starts_with("-s") && arg != "-s" {
                // The launchers only recognize the split short flag or the long flag.
                *arg = "--sandbox=read-only".into();
            }
            sandbox_specified = true;
        }
        if !sandbox_specified {
            extra.extend(["--sandbox".into(), "read-only".into()]);
        }
    }
    let models = match model.as_slice() {
        [] | [_] => Models::Single(model.pop()),
        _ if handoff => return Err("a handoff continues one conversation; pass --model at most once".into()),
        // Several writers in one worktree would edit the same files concurrently.
        _ if mode != "review" => {
            return Err("repeat --model only with --mode review; a writer runs one model".into());
        }
        _ if model.iter().enumerate().any(|(i, name)| model[..i].contains(name)) => {
            return Err("each repeated --model must name a different model".into());
        }
        _ => Models::Review(model),
    };
    Ok((extra, models))
}
