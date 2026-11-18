use crate::{
    domain::{Check, FreshVerification, Run, Verdict, Verification},
    process::{self, Script},
    store::{RunId, Store, timestamp},
};
use anyhow::Result;
use std::{path::Path, process::Command};

pub fn run(
    run: &mut Run,
    paths: Vec<String>,
    commands: Vec<String>,
    baseline: Option<String>,
    store: &Store,
    id: &RunId,
) -> Result<FreshVerification> {
    let transition = baseline
        .as_ref()
        .map(|new| format!("baseline {} -> {new}\n", run.baseline_commit.as_deref().unwrap_or("none")))
        .unwrap_or_default();
    let paths = if paths.is_empty() { &run.authorized_paths } else { &paths };
    let mut evidence = process::script(Script::CollectEvidence);
    evidence.arg(store.dir(id)).arg(&run.repo);
    let jobs = std::iter::once(("collect_evidence.sh".into(), evidence)).chain(commands.into_iter().map(|text| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &text]);
        (text, cmd)
    }));
    let scope = (|| -> Result<String> {
        let baseline = baseline
            .as_deref()
            .or(run.baseline_commit.as_deref())
            .ok_or_else(|| anyhow::anyhow!("no dispatch baseline; scope cannot be verified"))?;
        let mut files = dirty_paths(&run.repo)?;
        let text = process::checked(Command::new("git").current_dir(&run.repo).args([
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            baseline,
            "HEAD",
            "--",
        ]))?;
        files.extend(text.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
        let mut pre_existing = Vec::new();
        for path in &files {
            if let Some(hash) = run.dispatch_dirty.get(path)
                && &content_hash(&run.repo, path)? == hash
            {
                pre_existing.push(path.clone());
            }
        }
        let violations: Vec<_> = files
            .iter()
            .filter(|f| {
                !pre_existing.contains(f)
                    && !paths.iter().any(|p| {
                        let prefix = Path::new(p);
                        Path::new(f).starts_with(prefix.strip_prefix(".").unwrap_or(prefix))
                    })
            })
            .collect();
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
    let scope = Check { command: "baseline scope".into(), exit_code: Some(exit_code), output: transition + &output };
    let mut commands = Vec::new();
    for (command, mut cmd) in jobs {
        let result = process::output(cmd.current_dir(&run.repo));
        let (exit_code, output) = result.unwrap_or_else(|e| (None, format!("{e:#}")));
        commands.push(Check { command, exit_code, output });
    }
    let checks = std::iter::once(&scope).chain(&commands);
    let verdict = if checks.clone().all(|c| c.exit_code == Some(0)) { Verdict::Pass } else { Verdict::Fail };
    let attempt = run.attempt;
    let verification = Verification { verdict, checks: checks.cloned().collect(), timestamp: timestamp(), attempt };
    let locked = store.lock(id)?;
    *run = locked.load()?;
    if baseline.is_some() {
        run.baseline_commit = baseline;
    }
    run.verifications.push(verification);
    locked.save(run)?;
    Ok(FreshVerification { verdict, scope, commands, attempt })
}

pub fn dirty_paths(repo: &Path) -> Result<std::collections::BTreeSet<String>> {
    let mut files = std::collections::BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "--no-renames", "-z", "HEAD", "--"],
        vec!["diff", "--cached", "--name-only", "--no-renames", "-z", "HEAD", "--"],
        vec!["ls-files", "--others", "--exclude-standard", "-z"],
    ] {
        let text = process::checked(Command::new("git").current_dir(repo).args(args))?;
        files.extend(text.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
    }
    Ok(files)
}

pub fn content_hash(repo: &Path, path: &str) -> Result<Option<String>> {
    match std::fs::symlink_metadata(repo.join(path)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        // git hash-object follows symlinks and fails on a link to a directory.
        Ok(meta) if meta.file_type().is_symlink() => {
            Ok(Some(format!("symlink:{}", std::fs::read_link(repo.join(path))?.display())))
        }
        Ok(_) => {
            process::checked(Command::new("git").current_dir(repo).args(["hash-object", "--no-filters", "--", path]))
                .map(|hash| Some(hash.trim().to_owned()))
        }
    }
}
