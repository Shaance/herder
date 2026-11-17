use crate::{
    domain::{Check, Run, Verdict, Verification},
    process,
    store::{Store, timestamp},
};
use anyhow::Result;
use std::{path::Path, process::Command};

pub fn run(
    run: &mut Run,
    paths: Vec<String>,
    commands: Vec<String>,
    baseline: Option<String>,
    store: &Store,
) -> Result<()> {
    let transition = baseline
        .as_ref()
        .map(|new| format!("baseline {} -> {new}\n", run.baseline_commit.as_deref().unwrap_or("none")))
        .unwrap_or_default();
    let paths = if paths.is_empty() { &run.authorized_paths } else { &paths };
    let mut jobs = vec![("collect_evidence.sh".into(), {
        let mut cmd = process::script("implementer-core", "collect_evidence");
        cmd.arg(store.dir(&run.id)?).arg(&run.repo);
        cmd
    })];
    jobs.extend(commands.into_iter().map(|text| {
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
        anyhow::ensure!(
            violations.is_empty(),
            "outside authorized paths since {baseline}: {violations:?}; files={files:?}; pre-existing and unchanged={pre_existing:?}"
        );
        Ok(format!("baseline={baseline}; files={files:?}; pre-existing and unchanged={pre_existing:?}"))
    })();
    let mut checks = vec![match scope {
        Ok(output) => Check { command: "baseline scope".into(), exit_code: Some(0), output },
        Err(error) => Check { command: "baseline scope".into(), exit_code: Some(1), output: format!("{error:#}") },
    }];
    checks[0].output.insert_str(0, &transition);
    for (command, mut cmd) in jobs {
        let result = process::output(cmd.current_dir(&run.repo));
        let (exit_code, output) = result.unwrap_or_else(|e| (None, format!("{e:#}")));
        checks.push(Check { command, exit_code, output });
    }
    let verdict = if checks.iter().all(|c| c.exit_code == Some(0)) { Verdict::Pass } else { Verdict::Fail };
    let verification = Verification { verdict, checks, timestamp: timestamp(), attempt: run.attempt };
    let locked = store.lock(&run.id)?;
    *run = locked.load()?;
    if baseline.is_some() {
        run.baseline_commit = baseline;
    }
    run.verifications.push(verification);
    locked.save(run)
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
