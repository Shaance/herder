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
    // An unknown run neither settles nor aborts the wait; the settled run satisfies it.
    let started = Instant::now();
    let unknown = f.call(&["wait", &settled, &id, "--timeout", "10"], 0);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(unknown["data"]["runs"][0]["state"], "finished");
    assert_eq!(unknown["data"]["runs"][1]["state"], "unknown");
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

#[test]
fn old_run_without_extra_loads() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let mut record = f.record(&id);
    record.as_object_mut().unwrap().remove("extra");
    fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
    assert_eq!(f.call(&["status", &id], 0)["data"]["run"]["extra"], json!([]));
}

#[test]
fn names_resolve_once_for_run_commands_and_ids_take_precedence() {
    let f = Fixture::new();
    let first = f.dispatch("codex", &["--name", "review worker"], 0);
    let id = first["data"]["run_id"].as_str().unwrap();
    assert_eq!(f.call(&["status", "review worker"], 0)["data"]["run"]["id"], id);
    fs::write(f.dir(id).join("final.md"), "review").unwrap();
    assert_eq!(f.call(&["report", "review worker"], 0)["data"]["report"]["content"], "review");
    f.status("exited", "COMPLETED");
    f.call(&["verify", "review worker", "--path", "src/"], 0);
    f.status("alive", "RUNNING");
    fs::write(f.skills.join("ps"), format!("codex -o {}\n", f.dir(id).join("final.md").display())).unwrap();
    f.call(&["stop", "review worker"], 0);
    assert_eq!(f.call(&["wait", "review worker", "--all"], 0)["data"]["runs"][0]["id"], id);
    f.call(&["resume", "review worker", "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    f.dispatch("codex", &["--name", id], 0);
    assert_eq!(f.call(&["status", id], 0)["data"]["run"]["id"], id);
    let second = f.dispatch("codex", &["--name", "review worker"], 0);
    let other = second["data"]["run_id"].as_str().unwrap();
    for command in ["status", "report", "stop", "resume", "verify", "wait"] {
        for name in ["review worker", "unknown"] {
            let mut args = vec![command, name];
            let brief = f.root.join("brief.md");
            if command == "resume" {
                args.extend(["--brief", brief.to_str().unwrap()]);
            }
            let error = f.call(&args, 1);
            let error = error["error"].as_str().unwrap();
            if name == "unknown" {
                assert!(error.contains("crew ls") && !error.contains(id), "{error}");
            } else {
                assert!(error.contains("candidates:") && error.contains(id) && error.contains(other), "{error}");
            }
        }
    }
    let similar = f.call(&["status", "review"], 1);
    let similar = similar["error"].as_str().unwrap();
    assert!(similar.contains("similar names:") && similar.contains(id) && similar.contains(other), "{similar}");
    let out = f
        .cmd()
        .args(["dispatch", "--executor", "codex", "--brief"])
        .arg(f.root.join("brief.md"))
        .args(["--repo", ".", "--name", "printed-name"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().ends_with("\tprinted-name\n"));
}

#[test]
fn baseline_override_after_rebase_narrows_scope_and_persists() {
    let f = Fixture::new();
    let id = f.dispatch("codex", &["--path", "src/"], 0)["data"]["run_id"].as_str().unwrap().to_owned();
    let old = f.git(&["rev-parse", "HEAD"]);
    f.git(&["checkout", "-qb", "upstream"]);
    fs::write(f.repo.join("upstream.txt"), "upstream").unwrap();
    f.git(&["add", "."]);
    f.git(&["commit", "-qm", "upstream"]);
    let new = f.git(&["rev-parse", "HEAD"]);
    f.git(&["checkout", "-qb", "worker", &old]);
    fs::create_dir(f.repo.join("src")).unwrap();
    fs::write(f.repo.join("src/worker.rs"), "worker").unwrap();
    f.git(&["add", "."]);
    f.git(&["commit", "-qm", "worker"]);
    f.git(&["rebase", "upstream"]);
    assert_eq!(f.git(&["merge-base", &old, "HEAD"]), old);
    f.status("exited", "COMPLETED");
    f.call(&["verify", &id], 1);
    let before = fs::read(f.dir(&id).join("run.json")).unwrap();
    for invalid in ["does-not-exist", "HEAD:src/worker.rs"] {
        f.call(&["verify", &id, "--baseline", invalid], 1);
        assert_eq!(fs::read(f.dir(&id).join("run.json")).unwrap(), before);
    }
    let result = f.call(&["verify", &id, "--baseline", "upstream"], 0);
    let scope = result["data"]["run"]["verifications"][1]["checks"][0]["output"].as_str().unwrap();
    assert!(scope.contains(&format!("baseline {old} -> {new}")), "{scope}");
    assert!(scope.contains("src/worker.rs") && !scope.contains("upstream.txt"));
    assert_eq!(f.record(&id)["baseline_commit"], new);
    f.call(&["verify", &id], 0);
    fs::write(f.repo.join("outside"), "outside").unwrap();
    f.call(&["verify", &id, "--baseline", "HEAD"], 1);
    assert_eq!(f.record(&id)["baseline_commit"], f.git(&["rev-parse", "HEAD"]));
}

#[test]
fn review_mode_is_explicit_and_direct_remains_default() {
    let f = Fixture::new();
    f.script("implementer-core", "check_brief", r#"printf '%s\n' "$2" > "$CREW_STATE_DIR/lint-mode""#);
    f.dispatch("codex", &[], 0);
    assert_eq!(fs::read_to_string(f.state.join("lint-mode")).unwrap(), "direct\n");
    let result = f.dispatch("codex", &["--mode", "review"], 0);
    assert_eq!(fs::read_to_string(f.state.join("lint-mode")).unwrap(), "review\n");
    assert!(f.record(result["data"]["run_id"].as_str().unwrap()).get("mode").is_none());
    f.dispatch("codex", &["--mode", "lead"], 2);
}
