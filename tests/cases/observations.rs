use super::*;
use std::time::{Duration, Instant};

#[test]
fn status_and_listing_identify_the_observed_attempt() {
    let f = Fixture::new();
    let id = f.launch("codex");
    let check = |launch_state: &str, accepted: u32, observed: u32| {
        let status = f.call(&["status", "--repo", ".", "--", &id], 0);
        let listing = f.call(&["ls", "--repo", "."], 0);
        for entry in [&status["data"], &listing["data"]["runs"][0]] {
            assert_eq!(entry["run"]["id"], id);
            assert_eq!(entry["run"]["launch"]["state"], launch_state);
            assert_eq!(entry["run"]["attempt"], accepted);
            assert_eq!(entry["observed_attempt"], observed);
        }
        assert!(f.record(&id).get("observed_attempt").is_none());
    };
    check("launched", 1, 1);

    f.status("exited", "COMPLETED");
    f.script("codex-implementer", "resume_classic", "exit 3");
    f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
    assert_eq!(f.record(&id)["launch"]["attempt"], 2);
    check("pending_resume", 1, 2);

    fs::write(f.dir(&id).join("run.pid"), "43211").unwrap();
    fs::write(f.dir(&id).join("events2.jsonl"), "{\"type\":\"thread.started\",\"thread_id\":\"native-1\"}\n").unwrap();
    fs::write(f.skills.join("status"), "state=alive pid=43211\nverdict: RUNNING\n").unwrap();
    check("launched", 2, 2);
}

#[test]
fn scoped_observations_reject_foreign_and_noncanonical_ids_before_probing() {
    let mut f = Fixture::new();
    let checkout = f.repo.clone();
    f.repo = f.root.join("foreign checkout");
    fs::create_dir(&f.repo).unwrap();
    let foreign = f.dispatch("codex", &["--name", "foreign-review", "--goal", "reviews"], 0)["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.repo = checkout;
    let own = f.dispatch("codex", &["--name", "own-review", "--goal", "reviews"], 0)["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let foreign_lock = f.dir(&foreign).join("run.lock");
    fs::remove_file(&foreign_lock).unwrap();
    let foreign_record = fs::read(f.dir(&foreign).join("run.json")).unwrap();
    f.script(
        "codex-implementer",
        "status_classic",
        "printf '%s\\n' \"${1##*/}\" >> \"$HERDER_STATE_DIR/probes\"\ncat \"$HERDER_SKILLS_DIR/status\"",
    );
    let damaged = "damaged-review";
    fs::create_dir(f.dir(damaged)).unwrap();
    fs::write(f.dir(damaged).join("run.json"), "{").unwrap();
    let not_found = json!({"ok":false,"data":{"outcome":"not_found"},"error":null});
    for command in ["status", "report", "wait"] {
        for selector in [foreign.as_str(), "foreign", "own-review", "absent", "../run", "", damaged] {
            let mut args = vec![command, "--repo", "."];
            if command == "wait" {
                args.extend(["--timeout", "1"]);
            }
            args.extend(["--", selector]);
            assert_eq!(f.call(&args, 1), not_found);
        }
    }
    assert_eq!(f.call(&["wait", "--repo", ".", "--timeout", "1", "--", &own, &foreign], 1), not_found);
    assert!(!f.state.join("probes").exists());

    let list = f.call(&["ls", "--repo", ".", "--goal", "reviews"], 0);
    let entries = list["data"]["runs"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{list}");
    assert_eq!(entries[0]["run"]["id"], own);
    assert_eq!(entries[1], json!({"outcome":"incomplete"}));
    assert_eq!(fs::read_to_string(f.state.join("probes")).unwrap(), format!("{own}\n"));
    assert!(!foreign_lock.exists());
    assert_eq!(fs::read(f.dir(&foreign).join("run.json")).unwrap(), foreign_record);
    assert!(!f.dir(damaged).join("run.lock").exists());

    assert_eq!(f.call(&["status", "own-review"], 0)["data"]["run"]["id"], own);
    assert_eq!(f.call(&["status", "--repo", ".", "--", &own], 0)["data"]["run"]["id"], own);
    assert_eq!(f.call(&["report", "--repo", ".", "--", &own], 1)["data"]["report"]["message"], "no report yet");
    fs::write(f.dir(&own).join("final.md"), "saved review").unwrap();
    assert_eq!(f.call(&["report", "--repo", ".", "--", &own], 0)["data"]["report"]["content"], "saved review");
}

#[test]
fn scoped_observation_distinguishes_a_broken_backend_from_an_absent_record() {
    let f = Fixture::new();
    fs::write(f.state.join("runs"), "not a directory").unwrap();
    let bad = f.call(&["status", "--repo", ".", "run-id"], 1);
    assert_eq!(bad["data"], Value::Null);
    assert!(bad["error"].as_str().unwrap().starts_with("cannot read Herder runs directory"));
}

#[test]
fn scoped_wait_deadline_bounds_a_hung_executor_probe() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.call(&["wait", &id, "--repo", ".", "--timeout", "0"], 2);
    f.script("codex-implementer", "status_classic", "touch \"$HERDER_STATE_DIR/probed\"\nexec sleep 8");
    let started = Instant::now();
    let waited = f.call(&["wait", "--repo", ".", "--timeout", "1", "--", &id], 1);
    assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
    assert!(f.state.join("probed").exists());
    assert_eq!(waited["data"]["timed_out"], true);
    assert_eq!(waited["data"]["message"], "unknown state at timeout: inspect run evidence");
    let entry = &waited["data"]["runs"][0];
    assert_eq!(entry["id"], id);
    assert_eq!(entry["attempt"], 1);
    assert_eq!(entry["state"], "unknown");
}

#[test]
fn context_reports_configured_directory_and_git_checkout_without_retargeting() {
    let f = Fixture::new();
    let branch = f.git(&["symbolic-ref", "--short", "HEAD"]);
    let head = f.git(&["rev-parse", "HEAD"]);
    let nested = f.repo.join("nested");
    fs::create_dir(&nested).unwrap();
    let read = |path: &Path| f.call(&["context", "--repo", path.to_str().unwrap()], 0)["data"].clone();
    assert_eq!(
        read(&nested),
        json!({"configured_dir":nested,"repo":f.repo,"branch":branch,"head":head,"checkout_state":"attached","state_dir":f.state})
    );
    f.git(&["switch", "--detach", "-q", &head]);
    assert_eq!(
        read(&f.repo),
        json!({"configured_dir":f.repo,"repo":f.repo,"branch":null,"head":head,"checkout_state":"detached","state_dir":f.state})
    );

    let plain = f.root.join("plain");
    fs::create_dir(&plain).unwrap();
    assert_eq!(
        read(&plain),
        json!({"configured_dir":plain,"repo":null,"branch":null,"head":null,"checkout_state":"non_git","state_dir":f.state})
    );
    assert!(Command::new("git").current_dir(&plain).args(["init", "-q", "-b", "first"]).status().unwrap().success());
    assert_eq!(
        read(&plain),
        json!({"configured_dir":plain,"repo":plain,"branch":"first","head":null,"checkout_state":"unborn","state_dir":f.state})
    );

    let broken = f.root.join("broken-gitdir");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: missing\n").unwrap();
    assert_eq!(f.call(&["context", "--repo", broken.to_str().unwrap()], 1)["data"], Value::Null);
    assert_eq!(f.call(&["context", "--repo", "missing-directory"], 1)["data"], Value::Null);

    let no_git = f.cmd().env("PATH", &f.bin).args(["--json", "context", "--repo", "."]).output().unwrap();
    assert_eq!(no_git.status.code(), Some(1));
    let no_git: Value = serde_json::from_slice(&no_git.stdout).unwrap();
    assert_eq!(no_git["data"], Value::Null);
    assert!(no_git["error"].as_str().unwrap().starts_with("run Git"));
}
