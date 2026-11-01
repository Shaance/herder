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
