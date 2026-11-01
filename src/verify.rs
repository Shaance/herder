use crate::{
    domain::{Check, Run, Verdict, Verification},
    process,
    store::{Store, timestamp},
};
use anyhow::{Result, ensure};
use std::process::Command;

pub fn run(run: &mut Run, paths: Vec<String>, commands: Vec<String>, store: &Store) -> Result<()> {
    let paths = if paths.is_empty() { &run.authorized_paths } else { &paths };
    ensure!(!paths.is_empty(), "verify requires --path when no paths were recorded at dispatch");
    let mut jobs = vec![("collect_evidence.sh".into(), {
        let mut cmd = Command::new(process::script("implementer-core", "collect_evidence"));
        cmd.arg(store.dir(&run.id)?).arg(&run.repo).args(paths);
        cmd
    })];
    jobs.extend(commands.into_iter().map(|text| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &text]);
        (text, cmd)
    }));
    let mut checks = vec![];
    for (command, mut cmd) in jobs {
        let result = process::output(cmd.current_dir(&run.repo));
        let (exit_code, output) = result.unwrap_or_else(|e| (None, format!("{e:#}")));
        checks.push(Check { command, exit_code, output });
    }
    let verdict = if checks.iter().all(|c| c.exit_code == Some(0)) { Verdict::Pass } else { Verdict::Fail };
    run.verifications.push(Verification { verdict, checks, timestamp: timestamp()?, attempt: run.attempt });
    store.save(run)
}
