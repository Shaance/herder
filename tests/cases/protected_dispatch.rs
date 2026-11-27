use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn initial_dispatch_key_uses_exact_brief_and_route_without_relaunching() {
    let f = Fixture::new();
    let head = f.git(&["rev-parse", "HEAD"]);
    let branch = f.git(&["symbolic-ref", "--short", "HEAD"]);
    let dispatched = f.dispatch(
        "codex",
        &[
            "--mode",
            "review",
            "--request-id",
            "review/1",
            "--effort",
            "low",
            "--expected-head",
            &head,
            "--expected-branch",
            &branch,
        ],
        0,
    );
    let id = dispatched["data"]["run_id"].as_str().unwrap();
    fs::copy(f.root.join("brief.md"), f.root.join("copy.md")).unwrap();
    f.script("codex-implementer", "launch_classic", "touch \"$HERDER_STATE_DIR/launched-again\"\nexit 1");
    f.git(&["switch", "-qc", "other-branch"]);
    f.git(&["commit", "--allow-empty", "-qm", "new head"]);
    let run = |key: &str, effort: &str, code| {
        f.call(
            &[
                "dispatch",
                "--mode",
                "review",
                "--executor",
                "codex",
                "--repo",
                ".",
                "--brief",
                f.root.join("copy.md").to_str().unwrap(),
                "--request-id",
                key,
                "--effort",
                effort,
                "--expected-head",
                &head,
                "--expected-branch",
                &branch,
            ],
            code,
        )
    };
    assert_eq!(run("review/1", "low", 0)["data"], json!({"outcome":"replayed","run_id":id}));
    assert_eq!(run("review/2", "low", 1)["data"], json!({"outcome":"checkout_mismatch","reason":"branch"}));
    let conflict = json!({"ok":false,"data":{"outcome":"request_conflict"},"error":null});
    assert_eq!(run("review/1", "high", 1), conflict);
    f.write("copy.md", "different input\n");
    assert_eq!(run("review/1", "low", 1), conflict);
    assert!(!f.state.join("launched-again").exists());
    assert_eq!(fs::read_dir(f.state.join("runs")).unwrap().count(), 1);
}

fn external_check(f: &Fixture) -> PathBuf {
    let check = f.root.join("host check; literal");
    fs::write(&check, "#!/bin/sh\nset -eu\ntest \"$#\" -eq 0\nprintf '%s\\n' \"$PWD\" >> \"$HERDER_STATE_DIR/host-check-ran\"\necho verified\n").unwrap();
    fs::set_permissions(&check, fs::Permissions::from_mode(0o700)).unwrap();
    check
}

#[test]
fn protected_verification_replays_immutable_receipt_and_remains_separate_from_generic_history() {
    let f = Fixture::new();
    let check = external_check(&f);
    let check_path = check.to_str().unwrap();
    let head = f.git(&["rev-parse", "HEAD"]);
    let args = ["--request-id", "write/1", "--accept-with", check_path, "--expected-head", &head, "--path", "src"];
    let launched = f.dispatch("codex", &args, 0);
    let id = launched["data"]["run_id"].as_str().unwrap();
    assert_eq!(f.record(id)["extra"], json!(["--sandbox", "workspace-write"]));
    assert_eq!(f.record(id)["baseline_commit"], head);
    let verify = |code| {
        f.call(&["verify", "--repo", ".", "--accept-with", check_path, "--path", "src", "--", id], code)["data"].clone()
    };
    let host_marker = f.state.join("host-check-ran");
    let ordinary = f.launch("codex");
    let other_check = f.root.join("other executable");
    fs::copy(&check, &other_check).unwrap();
    for (id, path) in [(ordinary.as_str(), check_path), (id, other_check.to_str().unwrap())] {
        let denied = f.call(&["verify", "--repo", ".", "--accept-with", path, "--path", "src", "--", id], 1);
        assert_eq!(denied["data"]["outcome"], "not_eligible");
        assert!(!denied.to_string().contains(check_path));
    }
    let narrower = f.call(&["verify", "--repo", ".", "--accept-with", check_path, "--path", "src/subdir", "--", id], 1);
    assert_eq!(narrower["data"]["outcome"], "not_eligible");
    f.call(&["verify", "--repo", ".", "--accept-with", check_path, "--", id], 2);
    let broken = f.call(&["verify", "--repo", ".", "--accept-with", "/missing/check", "--path", "src", "--", id], 1);
    assert_eq!(broken["data"], Value::Null);
    assert!(!f.state.join("status-args").exists());
    assert_eq!(verify(0)["outcome"], "pending");
    assert!(!host_marker.exists());

    fs::create_dir(f.repo.join("src")).unwrap();
    fs::write(f.repo.join("src/worker.rs"), "authorized change").unwrap();
    assert_eq!(f.dispatch("codex", &args, 0)["data"], json!({"outcome":"replayed","run_id":id}));
    f.status("exited", "COMPLETED");
    f.call(&["verify", id, "--check", "true"], 0);
    assert_eq!(verify(1)["outcome"], "unverified");
    assert!(!host_marker.exists());

    fs::write(f.dir(id).join("events.jsonl"), "{\"type\":\"turn.completed\"}\n").unwrap();
    let passed = verify(0);
    assert_eq!(passed["outcome"], "verified");
    assert_eq!(passed["state"], "finished");
    assert_eq!(passed["scope"]["exit_code"], 0);
    assert_eq!(passed["host_check"]["exit_code"], 0);
    assert_eq!(passed["verdict"], "pass");
    assert_eq!(passed["attempt"], 1);
    let receipt = f.record(id)["protected_verification"].clone();
    assert_eq!(f.record(id)["protected_verification"]["host_check"], passed["host_check"]);
    assert_eq!(receipt["timestamp"], passed["verified_at"]);
    assert_eq!(fs::read_to_string(&host_marker).unwrap(), format!("{}\n", f.repo.display()));

    f.call(&["verify", id], 0);
    f.call(&["verify", id, "--baseline", &head], 2);
    assert_eq!(f.record(id)["baseline_commit"], head);
    fs::write(f.repo.join("outside.txt"), "unauthorized change").unwrap();
    assert_eq!(verify(0), passed);
    assert_eq!(f.record(id)["protected_verification"], receipt);
    assert_eq!(fs::read_to_string(&host_marker).unwrap().lines().count(), 1);

    fs::remove_file(f.repo.join("outside.txt")).unwrap();
    fs::remove_file(f.repo.join("src/worker.rs")).unwrap();
    let mut next_args = args;
    next_args[1] = "write/2";
    let next = f.dispatch("codex", &next_args, 0);
    assert_ne!(next["data"]["run_id"], id);
    let brief = f.root.join("brief.md");
    let resume = ["resume", id, "--brief", brief.to_str().unwrap()];
    for args in [resume.as_slice(), &[resume.as_slice(), &["--request-id", "new-resume"]].concat()] {
        let denied = f.call(args, 1);
        assert!(denied["error"].as_str().unwrap().contains("fresh keyed work order"));
    }
    assert_eq!(f.record(id)["attempt"], 1);

    // A stored resume receipt can still be read without starting another worker.
    let mut historic = f.record(id);
    historic["attempt"] = json!(2);
    historic["request_attempts"]["old-resume"] = json!(2);
    fs::copy(&brief, f.dir(id).join("input2.md")).unwrap();
    fs::write(f.dir(id).join("run.json"), historic.to_string()).unwrap();
    let replay = f.call(&[resume.as_slice(), &["--request-id", "old-resume"]].concat(), 0);
    assert_eq!(replay["data"]["run"]["attempt"], 2);
    assert!(!f.state.join("resume-args").exists());
    assert_eq!(verify(0), passed);
    assert_eq!(fs::read_to_string(host_marker).unwrap().lines().count(), 1);
}

#[test]
fn protected_dispatch_rejects_stale_dirty_or_escaping_configurations_before_launch() {
    let f = Fixture::new();
    let check = external_check(&f);
    let head = f.git(&["rev-parse", "HEAD"]);
    let options = |expected, path| {
        vec![
            "--request-id",
            "write/1",
            "--accept-with",
            check.to_str().unwrap(),
            "--expected-head",
            expected,
            "--path",
            path,
        ]
    };
    let stale = f.dispatch("codex", &options("wrong-head", "src"), 1);
    assert_eq!(stale["data"], json!({"outcome":"checkout_mismatch","reason":"head"}));
    let escaping = f.dispatch("codex", &options(&head, "../outside"), 1);
    assert!(escaping["error"].as_str().unwrap().contains("non-escaping relative"));
    let mut native = options(&head, "src");
    native.extend(["--", "--sandbox", "danger-full-access"]);
    f.dispatch("codex", &native, 2);

    let alias = f.root.join("checkout-alias");
    symlink(&f.repo, &alias).unwrap();
    let inside = alias.join("state-store");
    let brief = f.root.join("brief.md");
    let out = f
        .cmd()
        .env("HERDER_STATE_DIR", &inside)
        .args(["--json", "dispatch", "--executor", "codex", "--repo", ".", "--brief", brief.to_str().unwrap()])
        .args(options(&head, "src"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let blocked: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(blocked["error"].as_str().unwrap().contains("state directory must be outside"));
    assert!(!inside.exists());

    symlink(&f.root, f.repo.join("escape")).unwrap();
    let rejected = f.dispatch("codex", &options(&head, "escape/new.rs"), 1);
    assert!(rejected["error"].as_str().unwrap().contains("resolves outside the checkout"));
    let dirty = f.dispatch("codex", &options(&head, "src"), 1);
    assert!(dirty["error"].as_str().unwrap().contains("requires a clean Git checkout"));
    assert!(!f.state.join("launch-args").exists());
    assert!(!f.state.join("requests").exists());
    assert_eq!(fs::read_dir(f.state.join("runs")).unwrap().count(), 0);
}

#[test]
fn protected_owner_fences_new_work_and_checker_changes_are_in_the_saved_verdict() {
    let f = Fixture::new();
    let check = external_check(&f);
    let check_path = check.to_str().unwrap();
    fs::write(&check, "#!/bin/sh\nset -eu\necho unauthorized > outside.txt\necho private-host-output\n").unwrap();
    let head = f.git(&["rev-parse", "HEAD"]);
    let dispatch = |key, code| {
        f.dispatch(
            "codex",
            &["--request-id", key, "--accept-with", check_path, "--expected-head", &head, "--path", "src"],
            code,
        )["data"]
            .clone()
    };
    let first = dispatch("first", 0);
    let first = first["run_id"].as_str().unwrap();
    let verify = |id, code| {
        f.call(&["verify", "--repo", ".", "--accept-with", check_path, "--path", "src", "--", id], code)["data"].clone()
    };
    let busy = dispatch("second", 1);
    assert_eq!(busy["outcome"], "checkout_busy");
    assert_eq!(busy["run_id"], first);

    f.status("exited", "COMPLETED");
    fs::write(f.dir(first).join("events.jsonl"), "{\"type\":\"turn.completed\"}\n").unwrap();
    let awaiting_verify = dispatch("second", 1);
    assert_eq!(awaiting_verify["run_id"], first);
    assert!(awaiting_verify["message"].as_str().unwrap().contains("verify the finished"));
    f.call(&["verify", first, "--check", "true"], 0);
    let status = || {
        let output = f.cmd().args(["status", first]).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(status().contains("protected acceptance: not yet recorded"));
    let failed = verify(first, 1);
    assert_eq!(failed["host_check"]["exit_code"], 0);
    assert_eq!(failed["scope"]["exit_code"], 1);
    assert_eq!(failed["verdict"], "fail");
    let displayed = status();
    let verified_at = failed["verified_at"].as_str().unwrap();
    assert!(displayed.contains("worker state: finished"), "{displayed}");
    assert!(
        displayed.contains(&format!("protected acceptance: FAIL (attempt 1, verified_at {verified_at})")),
        "{displayed}"
    );
    assert!(displayed.contains("latest ordinary verification verdict: pass"), "{displayed}");
    assert!(!displayed.contains("private-host-output"), "{displayed}");
    let local_json = f.call(&["status", first], 0);
    assert!(
        local_json["data"]["run"]["protected_verification"]["host_check"]["output"]
            .as_str()
            .unwrap()
            .contains("private-host-output")
    );
    fs::remove_file(f.repo.join("outside.txt")).unwrap();
    let second = dispatch("second", 0);
    let second = second["run_id"].as_str().unwrap();
    assert_ne!(first, second);
    assert_eq!(verify(first, 1), failed);
    assert!(!f.repo.join("outside.txt").exists());

    // A finished worker without turn.completed has nothing this host can accept.
    assert_eq!(verify(second, 1)["outcome"], "unverified");
    let third = dispatch("third", 0);
    assert_ne!(third["run_id"], second);
    assert_eq!(verify(second, 0)["outcome"], "superseded");
}
