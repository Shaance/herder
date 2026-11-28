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
         1 43210 1 S Sat Nov 28 15:00:00 2026\n",
    );
    f.executable(
        &f.bin.join("kill"),
        r#"printf '%s\n' "$@" >> "$HERDER_STATE_DIR/kill-args"
printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
if test "$2" = 4242; then
    printf 'kill: %s: No such process\n' "$2" >&2
    exit 1
fi
if test "$2" = 4243; then : > "$HERDER_SKILLS_DIR/ps-snapshot"; fi"#,
    );
    let stopped = f.call(&["stop", &id], 0);
    assert_eq!(stopped["data"]["observation"]["state"], "stopped");
    let terms = signals(&f);
    let lines: Vec<_> = terms.lines().skip(2).collect();
    let (pairs, trailing) = lines.as_chunks::<2>();
    assert!(trailing.is_empty(), "{terms}");
    let mut pairs = pairs.to_vec();
    pairs.sort_unstable();
    assert!(terms.starts_with("-TERM\n43210\n"), "{terms}");
    assert_eq!(pairs, [["-TERM", "-4242"], ["-TERM", "4242"], ["-TERM", "4243"]]);
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
if test "$2" = 43210; then cp "$HERDER_SKILLS_DIR/ps-after-term" "$HERDER_SKILLS_DIR/ps-snapshot"; fi
if test "$1" = -KILL; then
    grep '^4242 ' "$HERDER_SKILLS_DIR/ps-after-term" > "$HERDER_SKILLS_DIR/ps-snapshot"
    printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
fi"#,
    );
    let began = Instant::now();
    let result = f.call(&["stop", &id], 0);
    assert!(began.elapsed() >= Duration::from_millis(1500));
    assert_eq!(result["data"]["observation"]["state"], "stopped");
    assert_eq!(signals(&f), "-TERM\n43210\n-TERM\n-43210\n-TERM\n4243\n-KILL\n4243\n");
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
if test "$2" = 43210; then
    cp "$HERDER_SKILLS_DIR/ps-after-term" "$HERDER_SKILLS_DIR/ps-snapshot"
    printf 'state=exited pid=43210\nverdict: DIED\n' > "$HERDER_SKILLS_DIR/status"
fi
if test "$1" = -TERM && test "$2" = 4242 && test -f "$HERDER_SKILLS_DIR/repeated"; then
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

    fs::remove_file(f.skills.join("ps")).unwrap();
    f.write("skills/repeated", "");
    assert_eq!(f.call(&["stop", &id], 0)["data"]["observation"]["state"], "stopped");
    assert_eq!(signals(&f), "-TERM\n43210\n-TERM\n-4242\n-TERM\n4242\n-KILL\n4242\n-TERM\n-4242\n-TERM\n4242\n");
}
