use super::*;
use std::{
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

#[test]
fn c1_busy_reads_return_recorded_state_and_mutations_time_out() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let before = f.record(&id);
    fs::write(f.dir(&id).join("final.md"), "existing report").unwrap();
    let mut holder = Command::new("/usr/bin/python3").args(["-c",
        "import fcntl,sys,time,pathlib; f=open(sys.argv[1],'w'); fcntl.flock(f,fcntl.LOCK_EX); pathlib.Path(sys.argv[2]).touch(); time.sleep(6)"])
        .arg(f.dir(&id).join("run.lock")).arg(f.state.join("held")).spawn().unwrap();
    super::durability::wait_file(&f.state.join("held"));
    let mut outputs = vec![];
    for args in [vec!["status", &id], vec!["report", &id], vec!["ls"], vec!["stop", &id]] {
        let mut child = f.cmd().args(&args).arg("--json").stdout(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_millis(if args[0] == "stop" { 3000 } else { 1000 });
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let finished = child.try_wait().unwrap().is_some();
        if !finished {
            child.kill().unwrap();
        }
        outputs.push((args[0], finished, child.wait_with_output().unwrap()));
    }
    let _ = holder.kill();
    holder.wait().unwrap();
    for (command, finished, output) in outputs {
        assert!(finished, "{command} blocked on run.lock");
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        if command == "stop" {
            assert_eq!(output.status.code(), Some(1));
            assert!(result["error"].as_str().unwrap().contains("timed out after 2s"));
        } else {
            assert!(output.status.success());
            let data = if command == "ls" { &result["data"]["runs"][0] } else { &result["data"] };
            assert_eq!(data["observation"]["state"], "running");
            assert_eq!(data["observation"]["evidence"], "run busy: another crew command holds the lock");
            if command == "report" {
                assert_eq!(data["report"]["content"], "existing report");
            }
        }
    }
    assert_eq!(f.record(&id), before);
    let lock = fs::OpenOptions::new().write(true).open(f.dir(&id).join("run.lock")).unwrap();
    lock.try_lock().unwrap();
    for launch in [
        json!({"state":"accepted", "id":"abc123", "profile":null, "error":null}),
        json!({"state":"pending_resume", "previous":before["launch"]["native"], "attempt":2, "error":null}),
    ] {
        let mut record = before.clone();
        record["launch"] = launch;
        fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
        let status = f.call(&["status", &id], 0);
        assert_eq!(status["data"]["observation"]["state"], "unknown");
        assert_eq!(status["data"]["observation"]["cause"], "unconfirmed");
    }
}

#[test]
fn c2_legacy_starting_and_untagged_verdict_load() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let mut record = f.record(&id);
    record["launch"]["intent"]["state"] = json!("starting");
    record["verifications"] = json!([{"verdict":"pass","checks":[],"timestamp":"legacy"}]);
    record.as_object_mut().unwrap().remove("baseline_commit");
    fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
    let result = f.call(&["status", &id], 0);
    assert_eq!(result["data"]["run"]["launch"]["intent"]["state"], "running");
    assert_eq!(result["data"]["run"]["verifications"][0]["attempt"], 1);
    assert_eq!(f.call(&["ls"], 0)["data"]["runs"][0]["run"]["id"], id);
}

#[test]
fn c3_scope_prefixes_match_components() {
    let f = Fixture::new();
    let id = f.launch("codex");
    fs::create_dir(f.repo.join("srcx")).unwrap();
    fs::write(f.repo.join("srcx/evil"), "outside").unwrap();
    f.status("exited", "COMPLETED");
    f.call(&["verify", &id, "--path", "src"], 1);
    fs::rename(f.repo.join("srcx"), f.repo.join("src")).unwrap();
    for prefix in ["src", "src/", "./src"] {
        f.call(&["verify", &id, "--path", prefix], 0);
    }
}

#[test]
fn c4_failed_pending_resume_retries_same_attempt() {
    for (executor, suffix, key) in [("codex", "classic", "THREAD_ID"), ("vibe", "vibe", "SESSION_ID")] {
        let f = Fixture::new();
        let id = f.launch(executor);
        f.status("exited", "COMPLETED");
        let path = f.skills.join(format!("{executor}-implementer/scripts/resume_{suffix}.sh"));
        let success = fs::read_to_string(&path).unwrap();
        f.script(&format!("{executor}-implementer"), &format!("resume_{suffix}"), "exit 1");
        let delta = f.root.join("brief.md");
        f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], 1);
        let pending = f.record(&id);
        assert_eq!(pending["launch"]["state"], "pending_resume");
        fs::write(&path, success).unwrap();
        f.status("alive", "RUNNING");
        f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], 1);
        assert_eq!(f.record(&id), pending);
        f.status("exited", "DIED");
        let resumed = f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], 0);
        assert_eq!(resumed["data"]["run"]["attempt"], 2);
        assert_eq!(resumed["data"]["run"]["launch"]["native"]["pid"], 43211);
        assert!(fs::read_to_string(f.dir(&id).join("launch2.log")).unwrap().contains(key));
    }
}

#[test]
fn c5_delta_format_does_not_require_dispatch_contract() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    f.write(
        "delta.md",
        "<task>Fix it.</task>\n<verification_loop>Test it.</verification_loop>\n<constraints>No publish.</constraints>",
    );
    let actual = PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".ai-skills/skills/implementer-core/scripts/check_brief.sh");
    fs::copy(actual, f.skills.join("implementer-core/scripts/check_brief.sh")).unwrap();
    let delta = f.root.join("delta.md");
    let result = f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], 0);
    assert_eq!(result["data"]["run"]["attempt"], 2);
}

#[test]
fn checked_output_keeps_stderr_out_of_baselines_and_scope() {
    let f = Fixture::new();
    let out = f
        .cmd()
        .env("GIT_TRACE", "1")
        .args(["--json", "dispatch", "--executor", "codex", "--brief"])
        .arg(f.root.join("brief.md"))
        .args(["--repo", ".", "--path", "src/"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = value["data"]["run_id"].as_str().unwrap();
    let head = f.git(&["rev-parse", "HEAD"]);
    assert_eq!(f.record(id)["baseline_commit"], head);
    f.status("exited", "COMPLETED");
    fs::create_dir(f.repo.join("src")).unwrap();
    fs::write(f.repo.join("src/file"), "worker").unwrap();
    let out = f
        .cmd()
        .env("GIT_TRACE", "1")
        .args(["--json", "verify", id, "--baseline", "HEAD", "--check", "echo stdout; echo stderr >&2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let checks = &value["data"]["run"]["verifications"][0]["checks"];
    assert!(!checks[0]["output"].as_str().unwrap().contains("trace:"));
    assert_eq!(checks[2]["output"], "stdout\nstderr\n");
    assert_eq!(f.record(id)["baseline_commit"], head);
    f.script("implementer-core", "check_brief", "echo stdout-diagnostic; echo stderr-diagnostic >&2; exit 1");
    let error = f.dispatch("codex", &[], 1);
    let error = error["error"].as_str().unwrap();
    assert!(error.contains("stdout-diagnostic") && error.contains("stderr-diagnostic"));
}

#[test]
fn scope_excludes_only_unchanged_dispatch_dirty_contents() {
    let f = Fixture::new();
    for path in ["unstaged", "staged", "deleted"] {
        fs::write(f.repo.join(path), "original").unwrap();
    }
    f.git(&["add", "."]);
    f.git(&["commit", "-qm", "tracked"]);
    fs::write(f.repo.join("unstaged"), "user edit").unwrap();
    fs::write(f.repo.join("staged"), "staged edit").unwrap();
    f.git(&["add", "staged"]);
    fs::remove_file(f.repo.join("deleted")).unwrap();
    fs::write(f.repo.join("brief\nfile"), "user brief").unwrap();
    let id = f.launch("codex");
    let snapshot = f.record(&id)["dispatch_dirty"].clone();
    assert_eq!(snapshot.as_object().unwrap().len(), 4);
    assert_eq!(snapshot["deleted"], Value::Null);
    f.status("exited", "COMPLETED");
    let result = f.call(&["verify", &id, "--path", "src/"], 0);
    let scope = result["data"]["run"]["verifications"][0]["checks"][0]["output"].as_str().unwrap();
    assert!(
        scope.contains("pre-existing and unchanged=") && scope.contains("unstaged") && scope.contains("brief\\nfile")
    );
    for path in ["unstaged", "staged", "deleted", "brief\nfile"] {
        fs::write(f.repo.join(path), "worker edit").unwrap();
        f.call(&["verify", &id, "--path", "src/"], 1);
        if path == "deleted" {
            fs::remove_file(f.repo.join(path)).unwrap();
        } else {
            fs::write(
                f.repo.join(path),
                if path == "staged" {
                    "staged edit"
                } else if path == "unstaged" {
                    "user edit"
                } else {
                    "user brief"
                },
            )
            .unwrap();
        }
    }
    let mut old = f.record(&id);
    old.as_object_mut().unwrap().remove("dispatch_dirty");
    fs::write(f.dir(&id).join("run.json"), old.to_string()).unwrap();
    let result = f.call(&["verify", &id, "--path", "src/"], 1);
    assert_eq!(result["data"]["run"]["dispatch_dirty"], json!({}));
}

#[test]
fn verify_refuses_live_workers_without_recording_checks() {
    let f = Fixture::new();
    let id = f.dispatch("claude", &["--path", "src/"], 0)["data"]["run_id"].as_str().unwrap().to_owned();
    for (status, state, expected) in [("busy", "working", "running"), ("idle", "waiting_for_permission", "blocked")] {
        f.agents(status, state, "native-session");
        let result = f.call(&["verify", &id, "--check", "touch ran-check"], 2);
        assert!(result["data"]["message"].as_str().unwrap().contains(expected));
        assert_eq!(f.record(&id)["verifications"], json!([]));
        assert!(!f.repo.join("ran-check").exists());
    }
    for (state, expected) in [("done", "finished"), ("failed", "failed"), ("stopped", "stopped")] {
        f.agents("idle", state, "native-session");
        assert_eq!(f.call(&["verify", &id], 0)["data"]["observation"]["state"], expected);
    }
    fs::write(f.skills.join("query-fail"), "").unwrap();
    assert_eq!(f.call(&["verify", &id], 0)["data"]["observation"]["state"], "unknown");
}
