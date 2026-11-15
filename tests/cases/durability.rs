use super::*;
use std::{
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

pub(super) fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "waiting for {}", path.display());
        thread::sleep(Duration::from_millis(10));
    }
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
        // A late identity after an interrupted/failed gate belongs to attempt 2.
        fs::write(f.dir(&id).join("run.pid"), "43211").unwrap();
        fs::write(f.skills.join("status"), "state=alive pid=43211\nverdict: RUNNING\n").unwrap();
        let recovered = f.call(&["status", &id], 0);
        assert_eq!(recovered["data"]["run"]["attempt"], 2);
        assert_eq!(recovered["data"]["run"]["launch"]["native"]["pid"], 43211);
        assert_eq!(recovered["data"]["observation"]["state"], "running");
    }
}

#[test]
fn a2_claude_acceptance_survives_coordinator_death() {
    let f = Fixture::new();
    let cli = f.bin.join("claude");
    let stub = fs::read_to_string(&cli).unwrap().replace(
        "agents)",
        "agents)\nif test -f \"$CREW_STATE_DIR/pause\"; then touch \"$CREW_STATE_DIR/entered\"; sleep 1; fi",
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
            "touch \"$CREW_STATE_DIR/entered\"; while test ! -f \"$CREW_STATE_DIR/release\"; do sleep 0.02; done",
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
    // Remove the input between successful lint and copy, before executor launch.
    f.script("implementer-core", "check_brief", "rm \"$1\"");
    f.dispatch("codex", &[], 1);
    for entry in fs::read_dir(f.state.join("runs")).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_name().to_string_lossy().starts_with('.') && entry.file_name() != "broken" {
            assert!(entry.path().join("run.json").is_file());
        }
    }
    let human = f.cmd().arg("ls").output().unwrap();
    assert!(String::from_utf8_lossy(&human.stdout).contains("broken\tbroken"));
}
