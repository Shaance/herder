use super::*;
use std::{
    io::{BufRead, BufReader},
    process::Stdio,
    time::{Duration, Instant},
};

#[test]
fn wait_any_returns_first_settled_and_all_polls_until_every_run_settles() {
    let f = Fixture::new();
    let first = f.launch("vibe");
    let second = f.launch("codex");
    for (owner, suffix) in [("vibe-implementer", "vibe"), ("codex-implementer", "classic")] {
        f.script(owner, &format!("status_{suffix}"), "cat \"$1/test-status\"");
    }
    fs::write(f.dir(&first).join("test-status"), "state=exited pid=43210\nverdict: FAILED\n").unwrap();
    fs::write(f.dir(&second).join("test-status"), "state=alive pid=43210\n").unwrap();
    let any = f.call(&["wait", &second, &first, "--timeout", "0"], 0);
    assert_eq!(any["data"]["runs"][0]["state"], "running");
    assert_eq!(any["data"]["runs"][1]["state"], "failed");
    for (index, id) in [&second, &first].iter().enumerate() {
        assert_eq!(
            any["data"]["runs"][index],
            json!({"id":id,"name":id,"state":if index == 0 {"running"} else {"failed"},"report_path":f.dir(id).join("final.md")})
        );
    }
    let mut child =
        f.cmd().args(["wait", &first, &second, "--all", "--timeout", "15"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut initial = String::new();
    for _ in 0..2 {
        output.read_line(&mut initial).unwrap();
    }
    assert_eq!(initial, format!("{first}\t{first}\tfailed\n{second}\t{second}\trunning\n"));
    assert!(child.try_wait().unwrap().is_none(), "--all must keep waiting");
    // A concurrent reader still obtains fresh evidence while wait is asleep.
    let concurrent = f.call(&["status", &second], 0);
    assert_eq!(concurrent["data"]["observation"]["state"], "running");
    assert!(!concurrent["data"]["observation"]["evidence"].as_str().unwrap().contains("run busy"));
    let mut record = f.record(&second);
    record["attempt"] = json!(2);
    fs::write(f.dir(&second).join("run.json"), record.to_string()).unwrap();
    fs::write(f.dir(&second).join("test-status"), "state=exited pid=43210\nverdict: STOPPED\n").unwrap();
    use std::io::Read;
    let mut rest = String::new();
    output.read_to_string(&mut rest).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        rest,
        format!(
            "{second}\t{second}\tstopped\n{first}\t{first}\tfailed\t{}/final.md\n{second}\t{second}\tstopped\t{}/final2.md\n",
            f.dir(&first).display(),
            f.dir(&second).display()
        )
    );
}

#[test]
fn wait_unknown_and_timeout_are_operational_failures() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let started = Instant::now();
    let timeout = f.call(&["wait", &id, "--timeout", "1"], 1);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(timeout["data"]["message"], "wait timed out");
    assert_eq!(timeout["data"]["runs"][0]["state"], "running");
    let settled = f.launch("claude");
    f.agents("idle", "done", "native-session");
    fs::write(f.skills.join("query-fail"), "").unwrap();
    // Fail only script queries, leaving a settled Claude run alongside unknown.
    let agents_script =
        fs::read_to_string(f.bin.join("claude")).unwrap().replace("  test ! -f \"$CREW_SKILLS_DIR/query-fail\"", "  :");
    fs::write(f.bin.join("claude"), agents_script).unwrap();
    let started = Instant::now();
    let unknown = f.call(&["wait", &settled, &id, "--timeout", "10"], 1);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(unknown["data"]["runs"][0]["state"], "finished");
    assert_eq!(unknown["data"]["runs"][1]["state"], "unknown");
    assert_eq!(unknown["data"]["message"], "unknown state: inspect run evidence");
    fs::remove_file(f.skills.join("query-fail")).unwrap();
    f.agents("idle", "blocked", "native-session");
    assert_eq!(f.call(&["wait", &settled, "--all", "--timeout", "0"], 0)["data"]["runs"][0]["state"], "blocked");
    f.call(&["wait"], 2);
    f.call(&["wait", &id, "--timeout", "-1"], 2);
}

#[test]
fn script_launch_and_resume_do_not_inherit_rex_environment() {
    for (executor, owner, suffix) in [("vibe", "vibe-implementer", "vibe"), ("codex", "codex-implementer", "classic")] {
        let f = Fixture::new();
        for action in ["launch", "resume"] {
            let path = f.skills.join(owner).join("scripts").join(format!("{action}_{suffix}.sh"));
            let original = fs::read_to_string(&path).unwrap();
            fs::write(path, original.replace("set -eu\n", &format!("set -eu\nprintf '%s\\n' \"${{REX_SESSION-unset}}\" \"${{REX_BLOCK-unset}}\" \"${{REX_SERVER-unset}}\" > \"$CREW_STATE_DIR/{action}-env\"\n"))).unwrap();
        }
        let mut cmd = f.cmd();
        cmd.env("REX_SESSION", "parent-session").env("REX_SERVER", "parent-server");
        let out = cmd
            .args(["dispatch", "--executor", executor, "--brief"])
            .arg(f.root.join("brief.md"))
            .args(["--repo", ".", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = value["data"]["run_id"].as_str().unwrap();
        assert_eq!(fs::read_to_string(f.state.join("launch-env")).unwrap(), "unset\nunset\nunset\n");
        f.status("exited", "COMPLETED");
        let out = f
            .cmd()
            .env("REX_SESSION", "parent-session")
            .env("REX_SERVER", "parent-server")
            .args(["resume", id, "--brief"])
            .arg(f.root.join("brief.md"))
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(fs::read_to_string(f.state.join("resume-env")).unwrap(), "unset\nunset\nunset\n");
    }
}

#[test]
fn ls_models_and_summary_count_observed_states_with_goal_filter() {
    let f = Fixture::new();
    let first = f.dispatch("vibe", &["--goal", "goal-1", "--model", "model-x"], 0);
    f.dispatch("vibe", &["--goal", "goal-1"], 0);
    f.dispatch("codex", &["--goal", "other"], 0);
    f.launch("claude");
    f.status("exited", "COMPLETED");
    let list = f.call(&["ls"], 0);
    assert_eq!(list["data"]["summary"], json!({"vibe":{"finished":2},"codex":{"finished":1},"claude":{"running":1}}));
    assert_eq!(f.call(&["ls", "--goal", "goal-1"], 0)["data"]["summary"], json!({"vibe":{"finished":2}}));
    let out = f.cmd().args(["ls", "--goal", "goal-1"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let id = first["data"]["run_id"].as_str().unwrap();
    assert!(text.contains(&format!("{id}\t-\tgoal-1\tvibe\tfinished\t1\tmodel-x\n")), "{text}");
    assert_eq!(text.matches("\tfinished\t1\t-\n").count(), 1);
    assert!(text.ends_with("vibe: 2 runs (finished 2)\n"), "{text}");
    fs::write(f.skills.join("query-fail"), "").unwrap();
    assert_eq!(f.call(&["ls", "--goal", "goal-1"], 0)["data"]["summary"], json!({"vibe":{"unknown":2}}));
    assert_eq!(f.call(&["ls", "--goal", "missing"], 0)["data"]["summary"], json!({}));
}
