use crate::{
    domain::{Check, FreshVerification, Run, Verdict, Verification},
    process::{self, Script},
    store::{RunId, Store, timestamp},
};
use anyhow::{Result, ensure};
use std::{path::Path, process::Command, time::Instant};

const SCOPE_CHECK: &str = "baseline scope";
const EVIDENCE_CHECK: &str = "collect_evidence.sh";

pub fn run(
    run: &mut Run,
    paths: Vec<String>,
    mut commands: Vec<String>,
    baseline: Option<String>,
    store: &Store,
    id: &RunId,
    deadline: Option<Instant>,
) -> Result<FreshVerification> {
    let mut reused_from = None;
    if deadline.is_none() && commands.is_empty() {
        let is_user_check = |check: &Check| !matches!(check.command.as_str(), SCOPE_CHECK | EVIDENCE_CHECK);
        if let Some(previous) = run.verifications.iter().rfind(|v| v.checks.iter().any(is_user_check)) {
            commands
                .extend(previous.checks.iter().filter(|check| is_user_check(check)).map(|check| check.command.clone()));
            reused_from = Some(run.id.clone());
        } else if let Some(goal) = &run.goal
            && let Some((source, previous)) = store
                .runs()?
                .into_iter()
                .flatten()
                .filter_map(|id| store.load(&id).ok())
                .filter(|other| other.id != run.id && other.goal.as_ref() == Some(goal) && other.repo == run.repo)
                .filter_map(|other| {
                    other.verifications.into_iter().rfind(|v| v.checks.iter().any(is_user_check)).map(|v| (other.id, v))
                })
                .max_by(|(a_id, a), (b_id, b)| (&a.timestamp, a_id).cmp(&(&b.timestamp, b_id)))
        {
            commands.extend(previous.checks.into_iter().filter(is_user_check).map(|check| check.command));
            reused_from = Some(source);
        }
    }
    let transition = baseline
        .as_ref()
        .map(|new| format!("baseline {} -> {new}\n", run.baseline_commit.as_deref().unwrap_or("none")))
        .unwrap_or_default();
    let paths = if paths.is_empty() { &run.authorized_paths } else { &paths };
    let mut evidence = process::script(Script::CollectEvidence);
    evidence.arg(store.dir(id)).arg(&run.repo);
    let jobs = std::iter::once((EVIDENCE_CHECK.into(), evidence)).chain(commands.into_iter().map(|text| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &text]);
        (text, cmd)
    }));
    let mut scope = scope_check(run, paths, baseline.as_deref(), deadline);
    scope.output = transition + &scope.output;
    let mut commands = Vec::new();
    for (command, mut cmd) in jobs {
        cmd.current_dir(&run.repo);
        let result = if let Some(deadline) = deadline {
            ensure!(
                store.load(id)?.launch_attempt() == run.attempt,
                "worker attempt changed before verification ended"
            );
            process::bounded_output(&mut cmd, deadline).map(|(status, stdout, stderr)| match status {
                Some(status) => (status.code(), stdout + &stderr),
                None => (None, "command timed out".into()),
            })
        } else {
            process::output(&mut cmd)
        };
        let (exit_code, output) = result.unwrap_or_else(|e| (None, format!("{e:#}")));
        commands.push(Check { command, exit_code, output });
    }
    let checks = std::iter::once(&scope).chain(&commands);
    let verdict = if checks.clone().all(|c| c.exit_code == Some(0)) { Verdict::Pass } else { Verdict::Fail };
    let attempt = run.attempt;
    let verification = Verification { verdict, checks: checks.cloned().collect(), timestamp: timestamp(), attempt };
    let locked = store.lock_until(id, deadline)?;
    let mut current = locked.load()?;
    if let Some(deadline) = deadline {
        ensure!(current.launch_attempt() == attempt, "worker attempt changed before verification ended");
        ensure!(matches!(verdict, Verdict::Fail) || Instant::now() < deadline, "verification deadline elapsed");
    }
    if baseline.is_some() {
        current.baseline_commit = baseline;
    }
    current.verifications.push(verification);
    locked.save(&current)?;
    *run = current;
    Ok(FreshVerification { verdict, scope, commands, attempt, reused_from })
}

pub fn scope_check(run: &Run, paths: &[String], baseline: Option<&str>, deadline: Option<Instant>) -> Check {
    let scope = (|| -> Result<String> {
        let baseline = baseline
            .or(run.baseline_commit.as_deref())
            .ok_or_else(|| anyhow::anyhow!("no dispatch baseline; scope cannot be verified"))?;
        let mut files = dirty_paths(&run.repo, deadline)?;
        let text = process::checked_until(
            Command::new("git").current_dir(&run.repo).args([
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                baseline,
                "HEAD",
                "--",
            ]),
            deadline,
        )?;
        files.extend(text.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
        let mut pre_existing = Vec::new();
        for path in &files {
            if let Some(hash) = run.dispatch_dirty.get(path)
                && &content_hash(&run.repo, path, deadline)? == hash
            {
                pre_existing.push(path.clone());
            }
        }
        let violations: Vec<_> =
            files.iter().filter(|f| !pre_existing.contains(f) && !paths.iter().any(|p| under_prefix(f, p))).collect();
        let allowed = paths.iter().map(|path| path.escape_debug().to_string()).collect::<Vec<_>>().join(", ");
        let changed = files.iter().map(|path| path.escape_debug().to_string()).collect::<Vec<_>>().join(", ");
        let unchanged = pre_existing.iter().map(|path| path.escape_debug().to_string()).collect::<Vec<_>>().join(", ");
        let short_baseline: String = baseline.chars().take(7).collect();
        let details = format!(
            "Changed files: {}.\nPre-existing and unchanged: {}.",
            if changed.is_empty() { "none" } else { &changed },
            if unchanged.is_empty() { "none" } else { &unchanged }
        );
        anyhow::ensure!(
            violations.is_empty(),
            "{} changed outside allowed paths ({allowed}) since {short_baseline}.\n{details}",
            violations.iter().map(|path| path.escape_debug().to_string()).collect::<Vec<_>>().join(", ")
        );
        Ok(format!("Allowed paths: {allowed}. Baseline: {short_baseline}.\n{details}"))
    })();
    let (exit_code, output) = match scope {
        Ok(output) => (0, output),
        Err(error) => (1, format!("{error:#}")),
    };
    Check { command: SCOPE_CHECK.into(), exit_code: Some(exit_code), output }
}

pub fn under_prefix(path: &str, prefix: &str) -> bool {
    let prefix = Path::new(prefix);
    Path::new(path).starts_with(prefix.strip_prefix(".").unwrap_or(prefix))
}

pub fn dirty_paths(repo: &Path, deadline: Option<Instant>) -> Result<std::collections::BTreeSet<String>> {
    let mut files = std::collections::BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "--no-renames", "-z", "HEAD", "--"],
        vec!["diff", "--cached", "--name-only", "--no-renames", "-z", "HEAD", "--"],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        let text = process::checked_until(Command::new("git").current_dir(repo).args(args), deadline)?;
        files.extend(text.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
    }
    Ok(files)
}

pub fn content_hash(repo: &Path, path: &str, deadline: Option<Instant>) -> Result<Option<String>> {
    match std::fs::symlink_metadata(repo.join(path)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        // git hash-object follows symlinks and fails on a link to a directory.
        Ok(meta) if meta.file_type().is_symlink() => {
            Ok(Some(format!("symlink:{}", std::fs::read_link(repo.join(path))?.display())))
        }
        Ok(_) => process::checked_until(
            Command::new("git").current_dir(repo).args(["hash-object", "--no-filters", "--", path]),
            deadline,
        )
        .map(|hash| Some(hash.trim().to_owned())),
    }
}
