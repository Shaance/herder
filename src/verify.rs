use crate::{
    domain::{Check, Run, Verdict, Verification},
    process,
    store::{Store, timestamp},
};
use anyhow::Result;
use std::{path::Path, process::Command};

pub fn run(run: &mut Run, paths: Vec<String>, commands: Vec<String>, store: &Store) -> Result<()> {
    let paths = if paths.is_empty() { &run.authorized_paths } else { &paths };
    let mut jobs = vec![("collect_evidence.sh".into(), {
        let mut cmd = Command::new(process::script("implementer-core", "collect_evidence"));
        cmd.arg(store.dir(&run.id)?).arg(&run.repo);
        cmd
    })];
    jobs.extend(commands.into_iter().map(|text| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &text]);
        (text, cmd)
    }));
    let scope = (|| -> Result<String> {
        let baseline = run
            .baseline_commit
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no dispatch baseline; scope cannot be verified"))?;
        let mut files = std::collections::BTreeSet::new();
        for args in [
            vec!["diff", "--name-only", "--no-renames", "-z", baseline, "HEAD", "--"],
            vec!["diff", "--name-only", "--no-renames", "-z", "HEAD", "--"],
            vec!["diff", "--cached", "--name-only", "--no-renames", "-z", "HEAD", "--"],
            vec!["ls-files", "--others", "--exclude-standard", "-z"],
        ] {
            let text = process::checked(Command::new("git").current_dir(&run.repo).args(args))?;
            files.extend(text.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
        }
        let violations: Vec<_> = files
            .iter()
            .filter(|f| {
                !paths.iter().any(|p| {
                    let prefix = Path::new(p);
                    Path::new(f).starts_with(prefix.strip_prefix(".").unwrap_or(prefix))
                })
            })
            .collect();
        anyhow::ensure!(
            violations.is_empty(),
            "outside authorized paths since {baseline}: {violations:?}; files={files:?}"
        );
        Ok(format!("baseline={baseline}; files={files:?}"))
    })();
    let mut checks = vec![match scope {
        Ok(output) => Check { command: "baseline scope".into(), exit_code: Some(0), output },
        Err(error) => Check { command: "baseline scope".into(), exit_code: Some(1), output: format!("{error:#}") },
    }];
    for (command, mut cmd) in jobs {
        let result = process::output(cmd.current_dir(&run.repo));
        let (exit_code, output) = result.unwrap_or_else(|e| (None, format!("{e:#}")));
        checks.push(Check { command, exit_code, output });
    }
    let verdict = if checks.iter().all(|c| c.exit_code == Some(0)) { Verdict::Pass } else { Verdict::Fail };
    let verification = Verification { verdict, checks, timestamp: timestamp()?, attempt: run.attempt };
    let locked = store.lock(&run.id)?;
    *run = locked.load()?;
    run.verifications.push(verification);
    locked.save(run)
}
