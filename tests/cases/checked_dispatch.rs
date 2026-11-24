use super::{durability::wait_file, *};
use std::{
    process::Stdio,
    time::{Duration, Instant},
};

const MARK: &str = "printf checked >> \"$HERDER_STATE_DIR/checked\"";

fn completed_codex(f: &Fixture) {
    let launcher = stub_body(&f.skills.join("codex-implementer/scripts/launch_classic.sh"));
    let body = fs::read_to_string(&launcher).unwrap();
    fs::write(launcher, format!("{body}printf '%s\\n' '{{\"type\":\"turn.completed\"}}' > \"$3/events.jsonl\"\n"))
        .unwrap();
}

#[test]
fn finished_dispatch_runs_ordered_checks_in_repo_with_closed_stdin() {
    for executor in ["codex", "vibe", "claude"] {
        let f = Fixture::new();
        completed_codex(&f);
        f.status("exited", "COMPLETED");
        f.agents("idle", "done", "native-session");
        let out = f
            .cmd()
            .args(["--json", "dispatch", "--executor", executor, "--repo", ".", "--brief"])
            .arg(f.root.join("brief.md"))
            .args(["--check", "test \"$(pwd)\" = \"$(git rev-parse --show-toplevel)\" && ! read line", "--check", MARK])
            .stdin(Stdio::piped())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        let data = &result["data"];
        let id = data["run_id"].as_str().unwrap();
        let verification = &data["verification"];
        assert_eq!(verification["verdict"], "pass", "{result}");
        assert_eq!(verification["scope"]["exit_code"], 0);
        assert_eq!(verification["commands"][1]["exit_code"], 0);
        assert_eq!(verification["commands"][2]["command"], MARK);
        assert_eq!(f.record(id)["verifications"][0]["attempt"], 1);
        assert_eq!(fs::read_to_string(f.state.join("checked")).unwrap(), "checked");

        if executor == "codex" {
            // A later attempt and passive commands cannot replay the armed verifier.
            f.call(&["resume", id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
            fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
            f.call(&["wait", id], 0);
        }
        f.call(&["status", id], 0);
        f.call(&["ls"], 0);
        assert_eq!(fs::read_to_string(f.state.join("checked")).unwrap(), "checked");
        assert_eq!(f.record(id)["verifications"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn checked_dispatch_scope_and_check_failure_are_operational_failures() {
    let f = Fixture::new();
    completed_codex(&f);
    f.script("codex-implementer", "status_classic", "touch unauthorized; cat \"$HERDER_SKILLS_DIR/status\"");
    f.status("exited", "COMPLETED");
    let failed = f.dispatch("codex", &["--check", "exit 7"], 1);
    let verify = &failed["data"]["verification"];
    assert_eq!(verify["verdict"], "fail");
    assert!(verify["scope"]["output"].as_str().unwrap().contains("unauthorized"));
    assert_eq!(verify["commands"][1]["exit_code"], 7);
}

#[test]
fn checked_dispatch_refuses_fanout_and_invalid_checks_before_launch() {
    let f = Fixture::new();
    let fanout = f.dispatch("vibe", &["--mode", "review", "--model", "one", "--model", "two", "--check", MARK], 2);
    assert!(fanout["data"]["message"].as_str().unwrap().contains("single dispatch"));
    f.dispatch("codex", &["--check-timeout", "1"], 2);
    f.dispatch("codex", &["--check", " "], 2);
    assert!(!f.state.join("runs").exists());
}

#[test]
fn unsuccessful_worker_or_worker_timeout_runs_no_verifier() {
    let f = Fixture::new();
    // The Vibe adapter can print stderr after the real DIED verdict.
    fs::write(f.skills.join("status"), "state=exited pid=43210\nverdict: DIED\nverdict: COMPLETED\n").unwrap();
    let failed = f.dispatch("vibe", &["--check", MARK], 1);
    assert_eq!(failed["data"]["verification"]["verdict"], Value::Null);
    assert!(failed["data"]["verification"]["message"].as_str().unwrap().contains("state failed"));
    f.status("alive", "RUNNING");
    let timed_out = f.dispatch("codex", &["--check", MARK, "--check-timeout", "1"], 1);
    assert!(timed_out["data"]["verification"]["message"].as_str().unwrap().contains("timed out"));
    let id = timed_out["data"]["run_id"].as_str().unwrap();
    assert_eq!(f.record(id)["verifications"], json!([]));
    assert!(!f.state.join("checked").exists());
}

#[test]
fn hung_status_query_respects_checked_dispatch_deadline() {
    let f = Fixture::new();
    f.script("codex-implementer", "status_classic", "sleep 60 & wait");
    let start = Instant::now();
    let result = f.dispatch("codex", &["--check", MARK, "--check-timeout", "1"], 1);
    assert!(start.elapsed() < Duration::from_secs(5), "{result}");
    assert!(result["data"]["verification"]["message"].as_str().unwrap().contains("timed out"));
    assert!(!f.state.join("checked").exists());
}

#[test]
fn check_timeout_closes_stdin_and_terminates_the_process_group() {
    let f = Fixture::new();
    completed_codex(&f);
    f.status("exited", "COMPLETED");
    let start = Instant::now();
    let result = f.dispatch(
        "codex",
        &[
            "--check",
            "if read line; then exit 7; fi; (sleep 3; touch \"$HERDER_STATE_DIR/escaped\") & wait",
            "--check-timeout",
            "2",
            "--check",
            MARK,
        ],
        1,
    );
    assert!(start.elapsed() < Duration::from_secs(5), "{result}");
    let verification = &result["data"]["verification"];
    assert_eq!(verification["verdict"], "fail", "{result}");
    assert_eq!(verification["commands"][1]["exit_code"], Value::Null);
    assert_eq!(verification["commands"][1]["output"], "command timed out");
    assert_eq!(verification["commands"][2]["output"], "command deadline elapsed");
    assert!(!f.state.join("checked").exists());
    std::thread::sleep(Duration::from_secs(2));
    assert!(!f.state.join("escaped").exists());
}

#[test]
fn resume_during_verification_invalidates_the_original_checked_dispatch() {
    for repeat_check in [false, true] {
        let f = Fixture::new();
        completed_codex(&f);
        f.status("exited", "COMPLETED");
        let mut dispatch = f.cmd();
        dispatch
            .args(["--json", "dispatch", "--executor", "codex", "--name", "checked-worker", "--repo", ".", "--brief"])
            .arg(f.root.join("brief.md"))
            .args([
                "--check",
                "touch \"$HERDER_STATE_DIR/entered\"; while test ! -f \"$HERDER_STATE_DIR/release\"; do sleep 0.02; done",
                "--check-timeout",
                "15",
            ]);
        if repeat_check {
            dispatch.args(["--check", MARK]);
        }
        let child = dispatch.stdout(Stdio::piped()).spawn().unwrap();
        wait_file(&f.state.join("entered"));
        let resumed = f.call(&["resume", "checked-worker", "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
        fs::write(f.state.join("release"), "").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["data"]["verification"]["verdict"], Value::Null, "{result}");
        assert!(result["data"]["verification"]["message"].as_str().unwrap().contains("attempt changed"));
        let id = resumed["data"]["run"]["id"].as_str().unwrap();
        assert_eq!(f.record(id)["attempt"], 2);
        assert_eq!(f.record(id)["verifications"], json!([]));
        assert!(!f.state.join("checked").exists());
    }
}

#[test]
fn verification_save_uses_the_remaining_deadline_while_run_lock_is_busy() {
    let f = Fixture::new();
    completed_codex(&f);
    f.status("exited", "COMPLETED");
    let child = f
        .cmd()
        .args(["--json", "dispatch", "--executor", "codex", "--repo", ".", "--brief"])
        .arg(f.root.join("brief.md"))
        .args([
            "--check",
            "touch \"$HERDER_STATE_DIR/entered\"; while test ! -f \"$HERDER_STATE_DIR/release\"; do sleep 0.02; done",
            "--check-timeout",
            "3",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    wait_file(&f.state.join("entered"));
    let id = fs::read_dir(f.state.join("runs")).unwrap().next().unwrap().unwrap().file_name();
    let lock = fs::OpenOptions::new().write(true).open(f.state.join("runs").join(&id).join("run.lock")).unwrap();
    lock.try_lock().unwrap();
    std::thread::sleep(Duration::from_millis(2100));
    let released_check = Instant::now();
    f.write("state/release", "");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(released_check.elapsed() < Duration::from_millis(1750), "save waited beyond the deadline: {out:?}");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    let verification = &result["data"]["verification"];
    assert_eq!(verification["verdict"], Value::Null, "{result}");
    assert!(verification["message"].as_str().unwrap().contains("verification deadline elapsed waiting for run.lock"));
    assert_eq!(f.record(id.to_str().unwrap())["verifications"], json!([]));
}
