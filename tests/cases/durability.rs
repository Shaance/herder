use super::*;
use std::{
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

pub(super) fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(Instant::now() < deadline, "waiting for {}", path.display());
        thread::sleep(Duration::from_millis(10));
    }
}

const CODEX_STARTED: &str = "{\"type\":\"thread.started\",\"thread_id\":\"native-1\"}\n";
const WRONG_CODEX_STARTED: &str = "{\"type\":\"thread.started\",\"thread_id\":\"wrong-thread\"}\n";

fn resume_codex_before_thread_started(f: &Fixture) -> String {
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    let stub = stub_body(&f.skills.join("codex-implementer/scripts/resume_classic.sh"));
    let original = fs::read(&stub).unwrap();
    f.script(
        "codex-implementer",
        "resume_classic",
        "echo 43211 > \"$1/run.pid\"\necho 'state=alive pid=43211' > \"$HERDER_SKILLS_DIR/status\"\nexit 3",
    );
    f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    fs::write(stub, original).unwrap();
    id
}

#[test]
fn a1_resume_validation_and_pending_identity() {
    for (executor, suffix, gate) in
        [("codex", "classic", 1), ("vibe", "vibe", 1), ("codex", "classic", 3), ("vibe", "vibe", 3)]
    {
        let f = Fixture::new();
        let id = f.launch(executor);
        f.status("exited", "COMPLETED");
        let before = f.record(&id);
        f.write("delta.md", "");
        let delta = f.root.join("delta.md");
        f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], 1);
        assert_eq!(f.record(&id), before);
        f.write("delta.md", "valid delta");
        f.script(&format!("{executor}-implementer"), &format!("resume_{suffix}"), &format!("exit {gate}"));
        f.call(&["resume", &id, "--brief", delta.to_str().unwrap()], if gate == 3 { 0 } else { 1 });
        let pending = f.record(&id);
        assert_eq!(pending["attempt"], 1);
        assert_eq!(pending["launch"]["state"], "pending_resume");
        assert!(pending["launch"].get("native").is_none());
        let unknown = f.call(&["status", &id], 0);
        assert_eq!(unknown["data"]["observation"]["state"], "unknown");
        assert_eq!(unknown["data"]["observation"]["cause"], "unconfirmed");
        // The worker PID can arrive before the resume identity.
        fs::write(f.dir(&id).join("run.pid"), "43211").unwrap();
        fs::write(f.skills.join("status"), "state=alive pid=43211\nverdict: RUNNING\n").unwrap();
        if executor == "codex" {
            let stream = f.dir(&id).join("events2.jsonl");
            for contents in [None, Some(CODEX_STARTED.trim_end())] {
                if let Some(contents) = contents {
                    fs::write(&stream, contents).unwrap();
                }
                let pending = f.call(&["status", &id], 0);
                assert_eq!(pending["data"]["run"]["attempt"], 1);
                assert_eq!(pending["data"]["observation"]["state"], "starting");
            }
            fs::write(stream, CODEX_STARTED).unwrap();
        }
        let recovered = f.call(&["status", &id], 0);
        assert_eq!(recovered["data"]["run"]["attempt"], 2);
        assert_eq!(recovered["data"]["run"]["launch"]["native"]["pid"], 43211);
        assert_eq!(recovered["data"]["observation"]["state"], "running");
    }
}

#[test]
fn vibe_resume_rejects_a_different_session_on_observation_and_retry() {
    let f = Fixture::new();
    let id = f.launch("vibe");
    let dir = f.dir(&id);
    f.status("exited", "COMPLETED");
    f.script(
        "vibe-implementer",
        "resume_vibe",
        r#"echo call >> "$HERDER_STATE_DIR/resume-count"
echo 43211 > "$1/run.pid"
echo 'error: resumed stream reports session wrong-session, expected native-1' >&2
exit 1"#,
    );
    let brief = f.root.join("brief.md");
    let args = ["resume", &id, "--brief", brief.to_str().unwrap()];
    f.call(&args, 1);
    let pending = fs::read(dir.join("run.json")).unwrap();
    fs::write(dir.join("events2.jsonl"), "{\"sessionId\":\"wrong-session\"}\n").unwrap();
    fs::write(f.skills.join("status"), "state=alive pid=43211\nverdict: RUNNING\n").unwrap();

    let status = f.call(&["status", &id], 0);
    let observation = &status["data"]["observation"];
    assert_eq!(observation["state"], "unknown");
    assert_eq!(observation["cause"], "identity_mismatch");
    assert_eq!(status["data"]["run"]["attempt"], 1);
    assert_eq!(fs::read(dir.join("run.json")).unwrap(), pending);

    fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: DIED\n").unwrap();
    let retry = f.call(&args, 1);
    for message in [&observation["evidence"], &retry["error"]] {
        let message = message.as_str().unwrap();
        for detail in ["43211", "native-1", "wrong-session"] {
            assert!(message.contains(detail), "{message}");
        }
    }
    assert_eq!(fs::read(dir.join("run.json")).unwrap(), pending);
    assert_eq!(fs::read_to_string(f.state.join("resume-count")).unwrap(), "call\n");
}

#[test]
fn codex_resume_rejects_a_different_thread_immediately_and_on_late_observation() {
    for (immediate, gate) in [(true, 0), (true, 3), (false, 0)] {
        let f = Fixture::new();
        let id = f.launch("codex");
        let dir = f.dir(&id);
        f.status("exited", "COMPLETED");
        fs::write(dir.join("final.md"), "accepted report").unwrap();
        fs::write(f.skills.join("ps"), format!("codex -o {}/final2.md resume native-1\n", dir.display())).unwrap();
        let event = if immediate {
            format!("printf '%s' '{WRONG_CODEX_STARTED}' > \"$1/events$4.jsonl\"")
        } else {
            String::new()
        };
        f.script(
            "codex-implementer",
            "resume_classic",
            &format!(
                r#"echo 43211 > "$1/run.pid"
echo 'state=alive pid=43211' > "$HERDER_SKILLS_DIR/status"
echo 'wrong report' > "$1/final$4.md"
{event}
echo 'PID=43211 THREAD_ID=native-1'
exit {gate}"#
            ),
        );
        let brief = f.root.join("brief.md");
        let args = ["resume", &id, "--brief", brief.to_str().unwrap()];
        let check_mismatch = |message: &Value| {
            let message = message.as_str().unwrap();
            for value in ["native-1", "wrong-thread", "43211"] {
                assert!(message.contains(value), "{message}");
            }
        };
        let resumed = f.call(&args, i32::from(immediate));
        if immediate {
            check_mismatch(&resumed["error"]);
        } else {
            assert_eq!(resumed["data"]["observation"]["state"], "starting");
            fs::write(dir.join("events2.jsonl"), WRONG_CODEX_STARTED).unwrap();
        }
        let status = f.call(&["status", &id], 0);
        let observation = &status["data"]["observation"];
        assert_eq!(observation["state"], "unknown");
        assert_eq!(observation["cause"], "identity_mismatch");
        check_mismatch(&observation["evidence"]);
        assert_eq!(f.call(&["ls"], 0)["data"]["runs"][0]["observation"], *observation);
        let waited = f.call(&["wait", &id, "--timeout", "0"], 1);
        assert_eq!(waited["data"]["runs"][0]["progress"]["cause"], "identity_mismatch");

        assert_eq!(f.call(&["report", &id], 0)["data"]["report"]["content"], "accepted report");
        f.call(&["stop", &id], 1);
        assert!(!f.state.join("kill-args").exists());
        fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
        check_mismatch(&f.call(&args, 1)["error"]);
        assert_eq!(f.record(&id)["attempt"], 1);
        assert_eq!(f.record(&id)["launch"]["state"], "pending_resume");
    }
}

#[test]
fn codex_resume_accepts_the_opening_event_outside_the_progress_tail() {
    let f = Fixture::new();
    let id = resume_codex_before_thread_started(&f);
    assert_eq!(f.record(&id)["attempt"], 1);

    let events = "{\"type\":\"item.completed\"}\n".repeat(12_000);
    assert!(events.len() > 256 * 1024);
    fs::write(f.dir(&id).join("events2.jsonl"), format!("{CODEX_STARTED}{events}{WRONG_CODEX_STARTED}")).unwrap();
    let status = f.call(&["status", &id], 0);
    assert_eq!(status["data"]["run"]["attempt"], 2);
    assert_eq!(status["data"]["observation"]["state"], "running");
}

#[test]
fn codex_resume_blocks_verification_while_waiting_for_a_live_worker_to_open_its_thread() {
    let f = Fixture::new();
    let id = resume_codex_before_thread_started(&f);
    let verified = f.call(&["verify", &id, "--path", ".", "--check", "touch verify-ran"], 2);
    assert_eq!(verified["data"]["message"], "cannot verify worker in state starting");
    assert!(!f.repo.join("verify-ran").exists());
    let status = f.call(&["status", &id], 0);
    assert_eq!(status["data"]["observation"]["state"], "starting");
    assert_eq!(status["data"]["run"]["attempt"], 1);
}

#[test]
fn codex_resume_failure_before_thread_started_settles_wait_and_advances_the_attempt() {
    let f = Fixture::new();
    let id = resume_codex_before_thread_started(&f);
    fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: DIED\n").unwrap();

    let waited = f.call(&["wait", &id, "--timeout", "1"], 1);
    assert_eq!(waited["data"]["runs"][0]["state"], "failed");
    assert_eq!(waited["data"]["message"], Value::Null);
    assert_eq!(f.record(&id)["attempt"], 2);

    fs::write(f.skills.join("resume-pid"), "43212").unwrap();
    let resumed = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    assert_eq!(resumed["data"]["run"]["attempt"], 3);
    assert_eq!(resumed["data"]["run"]["launch"]["native"]["thread_id"], "native-1");
    assert!(f.dir(&id).join("input3.md").exists());
}

#[test]
fn codex_resume_completion_before_thread_started_keeps_the_previous_report() {
    let f = Fixture::new();
    let id = resume_codex_before_thread_started(&f);
    fs::write(f.dir(&id).join("final.md"), "accepted report").unwrap();
    fs::write(f.dir(&id).join("final2.md"), "unconfirmed report").unwrap();
    fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();

    let status = f.call(&["status", &id], 0);
    assert_eq!(status["data"]["observation"]["state"], "unknown");
    assert_eq!(status["data"]["observation"]["cause"], "unconfirmed");
    assert_eq!(status["data"]["run"]["attempt"], 1);
    assert_eq!(f.call(&["report", &id], 0)["data"]["report"]["content"], "accepted report");
}

#[test]
fn codex_resume_refuses_a_second_launch_before_the_first_publishes_its_pid() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    // The coordinator persisted the reservation and died before the launcher published its PID.
    let mut pending = f.record(&id);
    pending["launch"] =
        json!({"state":"pending_resume", "previous":pending["launch"]["native"], "attempt":2, "error":null});
    fs::write(f.dir(&id).join("run.json"), pending.to_string()).unwrap();
    let brief = f.root.join("brief.md");
    let args = ["resume", &id, "--brief", brief.to_str().unwrap()];
    fs::copy(&brief, f.dir(&id).join("input2.md")).unwrap();
    assert_eq!(fs::read_to_string(f.dir(&id).join("run.pid")).unwrap().trim(), "43210");

    f.call(&args, 1);
    assert_eq!(f.record(&id), pending);
    assert!(!f.state.join("resume-args").exists());
}

#[test]
fn codex_resume_recovers_a_late_matching_event_and_launches_the_next_attempt() {
    let f = Fixture::new();
    let id = resume_codex_before_thread_started(&f);
    fs::write(f.dir(&id).join("events2.jsonl"), CODEX_STARTED).unwrap();
    fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
    fs::write(f.skills.join("resume-pid"), "43212").unwrap();

    let resumed = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    assert_eq!(resumed["data"]["run"]["attempt"], 3);
    assert_eq!(resumed["data"]["run"]["launch"]["native"]["pid"], 43212);
    assert_eq!(f.call(&["report", &id], 0)["data"]["report"]["content"], "attempt 3\n");
}

#[test]
fn codex_resume_with_a_recorded_launch_error_can_retry_unless_the_thread_conflicts() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let dir = f.dir(&id);
    let stub = stub_body(&f.skills.join("codex-implementer/scripts/resume_classic.sh"));
    let original = fs::read(&stub).unwrap();
    f.status("exited", "COMPLETED");
    f.script(
        "codex-implementer",
        "resume_classic",
        "echo 43211 > \"$1/run.pid\"\necho 'state=exited pid=43211\nverdict: DIED' > \"$HERDER_SKILLS_DIR/status\"\nexit 1",
    );
    let brief = f.root.join("brief.md");
    let args = ["resume", &id, "--brief", brief.to_str().unwrap()];
    f.call(&args, 1);
    let pending = f.record(&id);
    assert_eq!(pending["attempt"], 1);
    assert!(pending["launch"]["error"].is_object());
    fs::write(stub, original).unwrap();
    fs::write(f.skills.join("resume-pid"), "43212").unwrap();
    let stream = dir.join("events2.jsonl");
    fs::write(&stream, WRONG_CODEX_STARTED).unwrap();
    let rejected = f.call(&args, 1);
    assert!(rejected["error"].as_str().unwrap().contains("wrong-thread"), "{rejected}");
    assert_eq!(f.record(&id), pending);
    assert!(!f.state.join("resume-args").exists());

    fs::remove_file(stream).unwrap();
    let retried = f.call(&args, 0);
    assert_eq!(retried["data"]["run"]["attempt"], 2);
    assert_eq!(retried["data"]["run"]["launch"]["native"]["pid"], 43212);
    assert!(!dir.join("input3.md").exists());
}

#[test]
fn a2_claude_acceptance_survives_coordinator_death() {
    let f = Fixture::new();
    let cli = stub_body(&f.bin.join("claude"));
    let stub = fs::read_to_string(&cli).unwrap().replace(
        "agents)",
        "agents)\nif test -f \"$HERDER_STATE_DIR/pause\"; then touch \"$HERDER_STATE_DIR/entered\"; while test -f \"$HERDER_STATE_DIR/pause\"; do sleep 0.02; done; fi",
    );
    fs::write(&cli, stub).unwrap();
    fs::write(f.state.join("pause"), "").unwrap();
    let mut child = f
        .cmd()
        .args(["dispatch", "--executor", "claude", "--repo", ".", "--brief"])
        .arg(f.root.join("brief.md"))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_file(&f.state.join("entered"));
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_file(f.state.join("pause")).unwrap();
    let id = fs::read_dir(f.state.join("runs")).unwrap().next().unwrap().unwrap().file_name().into_string().unwrap();
    assert_eq!(f.record(&id)["launch"]["id"], "abc123");
    assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "running");
}

#[test]
fn a3_verify_reloads_after_concurrent_resume_without_lost_updates() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    let mut verify = f
        .cmd()
        .args([
            "verify",
            &id,
            "--path",
            "src/",
            "--check",
            "touch \"$HERDER_STATE_DIR/entered\"; while test ! -f \"$HERDER_STATE_DIR/release\"; do sleep 0.02; done",
        ])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_file(&f.state.join("entered"));
    let lock = fs::OpenOptions::new().write(true).open(f.dir(&id).join("run.lock")).unwrap();
    let lock_probe = lock.try_lock();
    drop(lock);
    let mut resume =
        f.cmd().args(["resume", &id, "--brief"]).arg(f.root.join("brief.md")).stdout(Stdio::null()).spawn().unwrap();
    let resumed = resume.wait().unwrap();
    fs::write(f.state.join("release"), "").unwrap();
    let verified = verify.wait().unwrap();
    assert!(lock_probe.is_ok(), "verification held the lock while executing checks: {lock_probe:?}");
    assert!(resumed.success());
    assert!(verified.success());
    let record = f.record(&id);
    assert_eq!(record["attempt"], 2);
    assert_eq!(record["verifications"][0]["attempt"], 1);
    assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "running");
}

#[test]
fn a6_listing_isolates_broken_runs_and_dispatch_publishes_atomically() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let broken = f.state.join("runs/broken");
    fs::create_dir(&broken).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            fs::write(broken.join("run.json"), "{").unwrap();
        }
        let listed = f.call(&["ls"], 0);
        let entries = listed["data"]["runs"].as_array().unwrap();
        assert!(entries.iter().any(|r| r["run"]["id"] == id));
        assert!(entries.iter().any(|r| r["run_id"] == "broken" && r["error"].is_string()));
        assert_eq!(f.call(&["ls", "--goal", "other"], 0)["data"]["runs"][0]["run_id"], "broken");
    }
    let human = f.cmd().arg("ls").output().unwrap();
    let human = String::from_utf8_lossy(&human.stdout);
    let row = human.lines().find(|line| line.starts_with("broken\tbroken\t")).unwrap();
    let columns: Vec<_> = row.split('\t').collect();
    assert_eq!(columns.len(), 3);
    assert!(columns[2].contains("run.json"), "{row}");
}

#[test]
fn a6_staging_failure_after_lint_leaves_no_published_run() {
    let f = Fixture::new();
    f.script("implementer-core", "check_brief", r#"printf 'lint completed\n' > "$HERDER_STATE_DIR/runs""#);
    let result = f.dispatch("codex", &[], 1);
    let runs = f.state.join("runs");
    assert_eq!(fs::read(&runs).unwrap(), b"lint completed\n");
    let staging_error = fs::create_dir_all(&runs).unwrap_err().to_string();
    assert_eq!(result["error"], staging_error);
    assert!(!f.state.join("launch-args").exists());
}

#[test]
fn a6_rejection_after_staging_leaves_no_pending_run() {
    let f = Fixture::new();
    f.executable(
        &f.bin.join("git"),
        r#"for pending in "$HERDER_STATE_DIR"/runs/.*.pending/input.md; do
  if test -f "$pending"; then touch "$HERDER_STATE_DIR/pending-observed"; fi
done
exec /usr/bin/git "$@""#,
    );
    let result = f.dispatch("codex", &["--mode", "review", "--expected-head", "stale"], 1);
    assert_eq!(result["data"], json!({"outcome":"checkout_mismatch","reason":"head"}));
    assert!(f.state.join("pending-observed").is_file());
    assert!(fs::read_dir(f.state.join("runs")).unwrap().next().is_none());
    assert!(!f.state.join("launch-args").exists());
}

#[test]
fn codex_resume_shows_the_pending_attempt_beside_its_state() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    f.script(
        "codex-implementer",
        "resume_classic",
        "echo 43211 > \"$1/run.pid\"\necho 'state=alive pid=43211' > \"$HERDER_SKILLS_DIR/status\"\nexit 3",
    );
    let out = f.cmd().args(["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id} attempt 2: starting\n"));
    let ls = String::from_utf8(f.cmd().arg("ls").output().unwrap().stdout).unwrap();
    assert!(ls.contains(&format!("{id}\t-\t-\tcodex\tstarting\t2\t")), "{ls}");
    let status = String::from_utf8(f.cmd().args(["status", &id]).output().unwrap().stdout).unwrap();
    // The state belongs to the pending attempt; the report is still the accepted attempt's.
    assert!(status.contains("attempt: 2\n") && status.contains("/final.md\n"), "{status}");
}
