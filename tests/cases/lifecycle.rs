use super::*;
use std::time::{Duration, Instant};

#[test]
fn a4_scope_includes_committed_staged_unstaged_and_untracked_paths() {
    let f = Fixture::new();
    let id = f.launch("codex");
    fs::write(f.repo.join("committed"), "committed").unwrap();
    f.git(&["add", "committed"]);
    f.git(&["commit", "-qm", "worker change"]);
    f.status("exited", "COMPLETED");
    let verified = f.call(&["verify", &id, "--path", "nothing/"], 1);
    assert!(verified["data"]["run"]["verifications"][0]["checks"].to_string().contains("committed"));
    fs::write(f.repo.join("staged"), "staged").unwrap();
    f.git(&["add", "staged"]);
    fs::write(f.repo.join("untracked\nfile"), "untracked").unwrap();
    fs::remove_file(f.repo.join("committed")).unwrap();
    let verified = f.call(&["verify", &id, "--path", "committed"], 1);
    let checks = verified["data"]["run"]["verifications"][1]["checks"].to_string();
    assert!(checks.contains("staged") && checks.contains("untracked"));
    f.call(&["verify", &id, "--path", "committed", "--path", "staged", "--path", "untracked\nfile"], 0);
    let mut old = f.record(&id);
    old.as_object_mut().unwrap().remove("baseline_commit");
    fs::write(f.dir(&id).join("run.json"), old.to_string()).unwrap();
    assert!(f.call(&["verify", &id, "--path", ""], 1).to_string().contains("no dispatch baseline"));
}

#[test]
fn a5_current_attempt_does_not_inherit_previous_verdict() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    f.call(&["verify", &id, "--path", "src/"], 0);
    f.status("exited", "COMPLETED");
    f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    let status = f.cmd().args(["status", &id]).output().unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).contains("latest verdict: none"));
    assert_eq!(f.record(&id)["verifications"][0]["attempt"], 1);
}

#[test]
fn a7_queries_timeout_but_existing_reports_remain_readable() {
    for executor in ["codex", "vibe", "claude"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        if executor == "claude" {
            f.executable(&f.bin.join("claude"), "echo query-entered; sleep 4 & wait");
        } else {
            f.script(
                &format!("{executor}-implementer"),
                if executor == "codex" { "status_classic" } else { "status_vibe" },
                "echo query-entered; sleep 4 & wait",
            );
        }
        fs::write(f.dir(&id).join("final.md"), "existing report").unwrap();
        for command in ["status", "report", "ls"] {
            let start = Instant::now();
            let result = f.call(&if command == "ls" { vec![command] } else { vec![command, &id] }, 0);
            assert!(start.elapsed() < Duration::from_millis(3500));
            let data = if command == "ls" { &result["data"]["runs"][0] } else { &result["data"] };
            assert_eq!(data["observation"]["state"], "unknown");
            assert!(data["observation"]["evidence"].as_str().unwrap().contains("timed out"));
            assert!(data["observation"]["evidence"].as_str().unwrap().contains("query-entered"));
            if command == "report" {
                assert_eq!(data["report"]["content"], "existing report");
            }
        }
    }
}

#[test]
fn a8_stop_waits_or_reports_stop_sent_without_failure() {
    for delayed in [true, false] {
        let f = Fixture::new();
        let id = f.launch("codex");
        fs::write(f.skills.join("ps"), format!("codex -o {}\n", f.dir(&id).join("final.md").display())).unwrap();
        f.executable(&f.bin.join("kill"), "touch \"$HERDER_STATE_DIR/signalled\"");
        if delayed {
            f.executable(&f.bin.join("kill"), "(sleep 0.3; printf 'state=exited pid=43210\\nverdict: STOPPED\\n' > \"$HERDER_SKILLS_DIR/status\") </dev/null >/dev/null 2>&1 &");
        }
        let stopped = f.call(&["stop", &id], 0);
        assert_eq!(stopped["data"]["observation"]["state"], if delayed { "stopped" } else { "running" });
        if !delayed {
            assert!(stopped["data"]["observation"]["evidence"].as_str().unwrap().contains("stop sent"));
            f.status("exited", "DIED");
            assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "stopped");
        }
    }
}

#[test]
fn a9_claude_report_does_not_establish_terminal_success() {
    let f = Fixture::new();
    let id = f.launch("claude");
    fs::write(f.dir(&id).join("final.md"), "partial report").unwrap();
    for (state, expected) in [("working", "blocked"), ("failed", "failed"), ("done", "finished")] {
        f.agents("idle", state, "native-session");
        let report = f.call(&["report", &id], 0);
        assert_eq!(report["data"]["observation"]["state"], expected);
        assert_eq!(report["data"]["report"]["content"], "partial report");
    }
}
