use super::*;
use std::time::{Duration, Instant};

fn launch_codex(f: &Fixture) -> String {
    let id = f.launch("codex");
    f.write("skills/ps", &format!("codex -o {}\n", f.dir(&id).join("final.md").display()));
    id
}

fn signals(f: &Fixture) -> String {
    fs::read_to_string(f.state.join("kill-args")).unwrap()
}

#[test]
fn stop_signals_tree_and_shared_group_once_and_ignores_esrch() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.write(
        "skills/ps-snapshot",
        "4243 4242 4242 S Sat Nov 28 16:00:03 2026\n\
         4242 43210 4242 S Sat Nov 28 16:00:02 2026\n\
         43210 1 43210 S Sat Nov 28 16:00:01 2026\n\
         4244 43210 4244 Z Sat Nov 28 16:00:04 2026\n\
         4245 43210 4245 S Sat Nov 28 16:00:05 2026\n\
         4247 43210 9999 S Sat Nov 28 16:00:07 2026\n\
         4248 43210 9999 S Sat Nov 28 16:00:08 2026\n\
         1 43210 1 S Sat Nov 28 15:00:00 2026\n",
    );
    f.executable(
        &f.bin.join("kill"),
        r#"printf '%s\n' "$@" >> "$HERDER_STATE_DIR/kill-args"
printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
if test "$3" = 4247; then
    : > "$HERDER_SKILLS_DIR/ps-snapshot"
    printf 'kill: %s: No such process\n' "$3" >&2
    exit 1
fi"#,
    );
    let stopped = f.call(&["stop", &id], 0);
    assert_eq!(stopped["data"]["observation"]["state"], "stopped");
    assert_eq!(signals(&f), "-TERM\n--\n-43210\n-TERM\n--\n-4242\n-4245\n-TERM\n--\n4247\n4248\n");
}

#[test]
fn stop_checks_start_time_before_signalling_and_kills_only_matching_descendants_after_grace() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.write(
        "skills/ps-snapshot",
        "43210 1 43210 S Sat Nov 28 16:00:01 2026\n\
         4242 43210 4242 S Sat Nov 28 16:00:02 2026\n\
         4243 43210 43210 S Sat Nov 28 16:00:03 2026\n",
    );
    f.write(
        "skills/ps-after-term",
        "43210 1 43210 S Sat Nov 28 16:00:01 2026\n\
         4242 1 4242 S Sat Nov 28 16:00:04 2026\n\
         4243 43210 43210 S Sat Nov 28 16:00:03 2026\n",
    );
    f.executable(
        &f.bin.join("kill"),
        r#"printf '%s\n' "$@" >> "$HERDER_STATE_DIR/kill-args"
if test "$3" = -43210; then cp "$HERDER_SKILLS_DIR/ps-after-term" "$HERDER_SKILLS_DIR/ps-snapshot"; fi
if test "$1" = -KILL; then
    grep '^4242 ' "$HERDER_SKILLS_DIR/ps-after-term" > "$HERDER_SKILLS_DIR/ps-snapshot"
    printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
fi"#,
    );
    let began = Instant::now();
    let result = f.call(&["stop", &id], 0);
    assert!(began.elapsed() >= Duration::from_millis(1500));
    assert_eq!(result["data"]["observation"]["state"], "stopped");
    assert_eq!(signals(&f), "-TERM\n--\n-43210\n-KILL\n--\n4243\n");
}

#[test]
fn stop_tracks_reparented_survivors_across_wait_verify_resume_and_repeated_stop() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    let child = "4242 43210 4242 S Sat Nov 28 16:00:02 2026\n";
    f.write("skills/ps-snapshot", &format!("43210 1 43210 S Sat Nov 28 16:00:01 2026\n{child}"));
    f.write("skills/ps-after-term", &child.replace(" 43210 ", " 1 "));
    f.executable(
        &f.bin.join("kill"),
        r#"printf '%s\n' "$@" >> "$HERDER_STATE_DIR/kill-args"
if test "$3" = -43210; then
    cp "$HERDER_SKILLS_DIR/ps-after-term" "$HERDER_SKILLS_DIR/ps-snapshot"
    printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
fi
if test "$1" = -TERM && test "$3" = -4242 && test -f "$HERDER_SKILLS_DIR/repeated"; then
    : > "$HERDER_SKILLS_DIR/ps-snapshot"
fi"#,
    );
    let result = f.call(&["stop", &id], 0);
    assert_eq!(result["data"]["observation"]["state"], "running");
    let intent = &f.record(&id)["launch"]["intent"];
    assert_eq!(intent["state"], "stop_sent");
    assert_eq!(intent["descendants"], json!([{"pid": 4242, "lstart": "Sat Nov 28 16:00:02 2026"}]));
    let status = f.call(&["status", &id], 0);
    assert_eq!(status["data"]["observation"]["state"], "running");
    assert!(status["data"]["observation"]["evidence"].as_str().unwrap().contains("still running: 4242"));
    assert_eq!(f.call(&["wait", &id, "--timeout", "1"], 1)["data"]["runs"][0]["state"], "running");
    let verify = f.call(&["verify", &id, "--path", "src/"], 2);
    assert_eq!(verify["data"]["message"], "cannot verify worker in state running");
    let resume = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 1);
    assert!(resume["error"].as_str().unwrap().contains("cannot resume"));
    assert_eq!(f.record(&id)["attempt"], 1);

    f.write(
        "skills/ps-snapshot",
        &format!("43210 1 43210 S Sat Nov 28 16:00:01 2026\n{}", child.replace(" 43210 ", " 1 ")),
    );
    f.executable(
        &f.bin.join("ps"),
        r#"case "$1" in
-p) exit 1;;
-Ao) cat "$HERDER_SKILLS_DIR/ps-snapshot";;
*) exit 90;;
esac"#,
    );
    f.write("skills/repeated", "");
    assert_eq!(f.call(&["stop", &id], 0)["data"]["observation"]["state"], "stopped");
    assert_eq!(signals(&f), "-TERM\n--\n-43210\n-TERM\n--\n-4242\n-KILL\n--\n4242\n-TERM\n--\n-4242\n");
}

#[test]
fn stop_refuses_a_worker_replaced_between_argv_check_and_snapshot() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.write("skills/ps-snapshot", "43210 1 43210 S Sat Nov 28 16:00:02 2026\n");
    let before = f.record(&id);
    let refused = f.call(&["stop", &id], 1);
    assert!(refused["error"].as_str().unwrap().contains("start time changed; refusing TERM"));
    assert!(!f.state.join("kill-args").exists());
    assert_eq!(f.record(&id), before);
}

#[test]
fn stop_preserves_normal_verdict_and_explains_missing_children_if_worker_exits_before_snapshot() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.executable(
        &f.bin.join("ps"),
        r#"case "$1" in
-p) printf 'S Sat Nov 28 16:00:01 2026 '; cat "$HERDER_SKILLS_DIR/ps";;
-Ao) printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status";;
*) exit 90;;
esac"#,
    );
    let stopped = f.call(&["stop", &id], 0);
    let observation = &stopped["data"]["observation"];
    assert_eq!(observation["state"], "failed", "{stopped}");
    assert!(
        observation["evidence"]
            .as_str()
            .unwrap()
            .contains("Child processes could not be checked because the worker had already exited"),
        "{stopped}"
    );
    assert!(!f.state.join("kill-args").exists());
    assert_eq!(f.record(&id)["launch"]["intent"]["state"], "running");
}

#[test]
fn stop_handles_a_reaped_or_zombie_worker_during_confirmation() {
    for response in ["exit 1", "echo 'Z Sat Nov 28 16:00:01 2026 <defunct>'", "echo 'permission denied' >&2; exit 1"] {
        let f = Fixture::new();
        let id = launch_codex(&f);
        f.executable(
            &f.bin.join("ps"),
            &format!(
                "test \"$1\" = -p\ntest \"$4\" = stat=,lstart=,args=\n\
                 printf 'state=exited pid=43210\\nverdict: COMPLETED\\n' > \"$HERDER_SKILLS_DIR/status\"\n{response}"
            ),
        );
        let denied = response.contains("permission denied");
        let reply = f.call(&["stop", &id], i32::from(denied));
        if denied {
            assert!(reply["error"].as_str().unwrap().contains("permission denied"), "{reply}");
        } else {
            let observation = &reply["data"]["observation"];
            assert_eq!(observation["state"], "finished", "{reply}");
            assert!(observation["evidence"].as_str().unwrap().contains("worker had already exited"), "{reply}");
        }
        assert!(!f.state.join("kill-args").exists());
    }
}

#[test]
fn stop_retains_executor_completion_failure_and_identity_errors() {
    for (pid, verdict, state) in
        [(43210, "COMPLETED", "finished"), (43210, "FAILED", "failed"), (43211, "DIED", "unknown")]
    {
        let f = Fixture::new();
        let id = launch_codex(&f);
        f.executable(
            &f.bin.join("kill"),
            &format!("printf 'state=exited pid={pid}\\nverdict: {verdict}\\n' > \"$HERDER_SKILLS_DIR/status\""),
        );
        for command in ["stop", "status"] {
            let result = f.call(&[command, &id], 0);
            let observation = &result["data"]["observation"];
            assert_eq!(observation["state"], state, "{result}");
            if state == "unknown" {
                assert_eq!(observation["cause"], "identity_mismatch", "{result}");
            }
        }
    }
}

#[test]
fn stop_names_worker_and_children_and_reserves_time_to_kill_after_slow_status() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.write(
        "skills/ps-snapshot",
        "43210 1 43210 S Sat Nov 28 16:00:01 2026\n4242 43210 43210 S Sat Nov 28 16:00:02 2026\n",
    );
    f.executable(
        &f.bin.join("ps"),
        r#"case "$1" in
-p) printf 'S Sat Nov 28 16:00:01 2026 '; cat "$HERDER_SKILLS_DIR/ps";;
-Ao)
    # Outlast the 500 ms left after escalation but finish within the snapshot's own budget.
    if test -f "$HERDER_SKILLS_DIR/term-sent"; then sleep 0.55; fi
    cat "$HERDER_SKILLS_DIR/ps-snapshot";;
*) exit 90;;
esac"#,
    );
    f.executable(
        &f.bin.join("kill"),
        r#"printf '%s\n' "$@" >> "$HERDER_STATE_DIR/kill-args"
if test "$1" = -TERM; then
    touch "$HERDER_SKILLS_DIR/term-sent"
fi"#,
    );
    f.script(
        "codex-implementer",
        "status_classic",
        r#"if test -f "$HERDER_SKILLS_DIR/term-sent"; then
    touch "$HERDER_SKILLS_DIR/queried-after-term"
    sleep 4
fi
cat "$HERDER_SKILLS_DIR/status""#,
    );
    let stopped = f.call(&["stop", &id], 0);
    let observation = &stopped["data"]["observation"];
    assert_eq!(observation["state"], "running", "{stopped}");
    assert_eq!(signals(&f), "-TERM\n--\n-43210\n-KILL\n--\n4242\n");
    let evidence = observation["evidence"].as_str().unwrap();
    assert!(evidence.contains("worker 43210 still running"), "{evidence}");
    assert!(evidence.contains("1 child process still running: 4242"), "{evidence}");
    assert!(!f.skills.join("queried-after-term").exists());
}

#[test]
fn failed_stop_snapshot_reports_unknown_and_blocks_follow_up_actions() {
    let f = Fixture::new();
    let id = launch_codex(&f);
    f.executable(
        &f.bin.join("ps"),
        r#"case "$1" in
-p) printf 'S Sat Nov 28 16:00:01 2026 '; cat "$HERDER_SKILLS_DIR/ps";;
-Ao)
    if test -f "$HERDER_STATE_DIR/kill-args"; then
        echo 'ps: Operation not permitted' >&2
        exit 1
    fi
    echo '43210 1 43210 S Sat Nov 28 16:00:01 2026';;
*) exit 90;;
esac"#,
    );
    for command in ["stop", "status"] {
        let result = f.call(&[command, &id], 0);
        let observation = &result["data"]["observation"];
        assert_eq!(observation["state"], "unknown", "{result}");
        assert_eq!(observation["cause"], "query_failed", "{result}");
        let evidence = observation["evidence"].as_str().unwrap();
        assert!(evidence.starts_with("stop sent; could not check processes: "), "{evidence}");
        assert!(evidence.contains("Operation not permitted"), "{evidence}");
    }
    assert_eq!(f.record(&id)["launch"]["intent"]["state"], "stop_sent");
    assert_eq!(f.call(&["wait", &id, "--timeout", "1"], 1)["data"]["runs"][0]["state"], "unknown");
    let verify = f.call(&["verify", &id, "--path", "src/"], 2);
    assert_eq!(verify["data"]["message"], "cannot verify worker in state unknown");
    let resume = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 1);
    assert!(resume["error"].as_str().unwrap().contains("cannot resume"));
}
