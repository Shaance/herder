use crate::{domain::Executor, process, scripts};
use anyhow::Result;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

pub fn run(executor: Option<Executor>, profile: Option<PathBuf>) -> Result<(Value, i32)> {
    let profile = process::profile(profile)?;
    let mut executors = vec![];
    let mut healthy = true;
    for e in [Executor::Claude, Executor::Vibe, Executor::Codex] {
        if executor.is_some_and(|wanted| wanted != e) {
            continue;
        }
        let name = match e {
            Executor::Claude => "claude",
            Executor::Vibe => "vibe",
            Executor::Codex => "codex",
        };
        let mut checks = vec![];
        for exe in if e == Executor::Claude { vec![name] } else { vec![name, "bash", "jq"] } {
            let present = process::on_path(exe);
            healthy &= present;
            checks.push(json!({"item":exe,"ok":present,"fix":if present {None} else {Some(format!("install {exe} and put its executable on PATH"))}}));
        }
        let mut paths = vec![
            process::script("implementer-core", "check_brief"),
            process::script("implementer-core", "collect_evidence"),
        ];
        if e != Executor::Claude {
            paths.extend(["launch", "status", "resume"].map(|a| scripts::path(e, a)));
        }
        for path in paths {
            let present = process::executable(&path);
            healthy &= present;
            checks.push(json!({"item":path,"ok":present,"fix":if present {None} else {Some(format!("restore executable {} or set CREW_SKILLS_DIR to the skills directory", path.display()))}}));
        }
        executors.push(json!({"executor":e,"checks":checks}));
    }
    let rex = process::checked(Command::new("rex").args(["--autostart=false", "whoami", "--json"]));
    healthy &= rex.is_ok();
    let data = json!({"executors":executors,"claude_profile":profile,"profile_resolution":"--profile > CLAUDE_CONFIG_DIR > none", "rex":{"ok":rex.is_ok(),"detail":match rex {Ok(s)=>s,Err(e)=>format!("{e:#}")},"fix":"start Rex and set REX_SERVER to its reachable control socket"},"pane_loop":{"watch_available":process::on_path("watch"),"fix":"install watch to view Vibe/Codex panes"}});
    Ok((data, if healthy { 0 } else { 1 }))
}
