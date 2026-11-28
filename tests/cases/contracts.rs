use super::durability::wait_file;
use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

fn setup() -> (Fixture, PathBuf) {
    let f = Fixture::new();
    fs::remove_file(f.bin.join("ps")).unwrap();
    let skills = std::env::var_os("HERDER_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/ai-skills"));
    let skills = std::path::absolute(skills).unwrap();
    assert!(
        skills.join("codex-implementer/scripts/launch_classic.sh").is_file(),
        "contract tests require the vendored scripts or HERDER_SKILLS_DIR pointing to a skills directory"
    );
    let jq = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("jq"))
        .find(|p| p.is_file())
        .expect("contract tests require jq");
    fs::remove_file(f.bin.join("jq")).unwrap();
    std::os::unix::fs::symlink(jq, f.bin.join("jq")).unwrap();
    let brief = [
        "task",
        "workstream_contract",
        "implementation_authority",
        "action_safety",
        "default_follow_through_policy",
        "verification_loop",
        "economy_contract",
        "structured_output_contract",
    ]
    .map(|tag| format!("<{tag}>Test only. Authorized paths: none. Do not commit or push.</{tag}>\n"))
    .concat();
    f.write("brief.md", &brief);
    (f, skills)
}

fn call(f: &Fixture, skills: &Path, args: &[&str]) -> (i32, Value) {
    let out = f.cmd().env("HERDER_SKILLS_DIR", skills).args(args).arg("--json").output().unwrap();
    (out.status.code().unwrap(), serde_json::from_slice(&out.stdout).unwrap())
}

#[test]
fn embedded_scripts_run_dispatch_lint_launch_status_and_verify() {
    let (f, _) = setup();
    let home = f.root.join("empty-home");
    fs::create_dir(&home).unwrap();
    f.executable(
        &f.bin.join("codex"),
        r#"printf '%s\n' "$@" > "$HERDER_STATE_DIR/worker-args"
while [ "$#" -gt 0 ]; do
    if [ "$1" = -o ]; then shift; echo 'stub report' > "$1"; break; fi
    shift
done
echo '{"type":"thread.started","thread_id":"stub-thread"}'
echo '{"type":"turn.completed"}'"#,
    );
    let call = |args: &[&str]| {
        let out = f.cmd().env_remove("HERDER_SKILLS_DIR").env("HOME", &home).args(args).arg("--json").output().unwrap();
        (out.status.code().unwrap(), serde_json::from_slice::<Value>(&out.stdout).unwrap())
    };
    f.write("invalid.md", "A plain brief.\n");
    let invalid = f.root.join("invalid.md");
    let (code, rejected) =
        call(&["dispatch", "--executor", "codex", "--repo", ".", "--brief", invalid.to_str().unwrap()]);
    assert_eq!(code, 1, "{rejected}");
    assert!(rejected["error"].as_str().unwrap().contains("missing required blocks"), "{rejected}");

    let brief = f.root.join("brief.md");
    let (code, dispatch) = call(&[
        "dispatch",
        "--executor",
        "codex",
        "--mode",
        "review",
        "--repo",
        ".",
        "--brief",
        brief.to_str().unwrap(),
        "--path",
        ".",
        "--model",
        "stub-model",
        "--effort",
        "medium",
    ]);
    assert_eq!(code, 0, "{dispatch}");
    assert_eq!(dispatch["data"]["launch_error"], Value::Null, "{dispatch}");
    let id = dispatch["data"]["run_id"].as_str().unwrap();
    let worker_args = || fs::read_to_string(f.state.join("worker-args")).unwrap();
    assert_eq!(f.record(id)["extra"], json!(["--sandbox", "read-only"]));
    let settings = "--model\nstub-model\n-c\nmodel_reasoning_effort=medium\n--sandbox\nread-only\n";
    let args = worker_args();
    assert!(args.contains(settings), "{args}");
    assert_eq!(args.lines().filter(|arg| *arg == "--sandbox").count(), 1, "{args}");
    let (code, report) = call(&["report", id]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["observation"]["state"], "finished", "{report}");
    assert!(report["data"]["report"]["content"].as_str().unwrap().contains("stub report"));
    let (code, verified) = call(&["verify", id]);
    assert_eq!(code, 0, "{verified}");
    let check = &verified["data"]["run"]["verifications"][0]["checks"][1];
    assert_eq!(check["command"], "collect_evidence.sh");
    assert_eq!(check["exit_code"], 0);
    assert!(check["output"].as_str().unwrap().contains("worktree status"));

    for attempt in [2, 3] {
        let (code, resumed) = call(&["resume", id, "--brief", brief.to_str().unwrap()]);
        assert_eq!(code, 0, "{resumed}");
        assert_eq!(resumed["data"]["run"]["attempt"], attempt);
        assert_eq!(f.record(id)["requested_effort"], "medium");
        assert_eq!(f.record(id)["extra"], json!(["--sandbox", "read-only"]));
        let args = worker_args();
        assert!(
            args.contains(&format!(
                "{settings}-o\n{}/final{attempt}.md\nresume\nstub-thread\n-\n",
                f.dir(id).display()
            )),
            "{args}"
        );
        assert_eq!(args.lines().filter(|arg| *arg == "--sandbox").count(), 1, "{args}");
    }

    // Older Codex review records lack a sandbox in their saved extras.
    let mut legacy = f.record(id);
    legacy["extra"] = json!([]);
    legacy.as_object_mut().unwrap().remove("requested_effort");
    fs::write(f.dir(id).join("run.json"), serde_json::to_vec(&legacy).unwrap()).unwrap();
    let (code, resumed) = call(&["resume", id, "--brief", brief.to_str().unwrap()]);
    assert_eq!(code, 0, "{resumed}");
    let args = worker_args();
    assert!(args.contains("\n--sandbox\nworkspace-write\n"), "{args}");
}

#[test]
fn real_launch_and_status_scripts_accept_stub_executors() {
    for executor in ["codex", "vibe"] {
        let (f, skills) = setup();
        let body = if executor == "codex" {
            r#"import os,sys,pathlib
args=sys.argv
pathlib.Path(os.environ['HERDER_STATE_DIR'], 'worker-args').write_text('\n'.join(args))
pathlib.Path(args[args.index('-o')+1]).write_text('stub report')
print('{"type":"thread.started","thread_id":"stub-thread"}')
print('{"type":"turn.completed"}')"#
        } else {
            r#"import os,sys,pathlib,json
args=sys.argv; out=pathlib.Path(args[args.index('--output-dir')+1]); out.mkdir()
pathlib.Path(os.environ['HERDER_STATE_DIR'], 'worker-args').write_text('\n'.join(args))
(out/'export.json').write_text(json.dumps({'outcome':'finished','config':{'active_model':'stub','models':{'stub':{'name':'stub','thinking':'high'}}}}))
print('{"sessionId":"stub-session"}')
print('{"type":"message","role":"assistant","content":[{"type":"text","text":"stub report"}]}')"#
        };
        fs::write(stub_body(&f.bin.join(executor)), format!("#!/usr/bin/python3\n{body}\n")).unwrap();
        let mode = if executor == "codex" { "direct" } else { "review" };
        let brief = f.root.join("brief.md");
        let args =
            ["dispatch", "--executor", executor, "--mode", mode, "--repo", ".", "--brief", brief.to_str().unwrap()];
        let (code, dispatch) = call(&f, &skills, &args);
        assert_eq!(code, 0, "{dispatch}");
        let id = dispatch["data"]["run_id"].as_str().unwrap();
        assert_eq!(f.record(id)["extra"], json!([]));
        let args = fs::read_to_string(f.state.join("worker-args")).unwrap();
        if executor == "codex" {
            assert!(args.contains("\n--sandbox\nworkspace-write\n"), "{args}");
        } else {
            assert!(args.lines().any(|arg| arg == "--auto-approve"), "{args}");
            assert!(!args.contains("--sandbox"), "{args}");
        }
        let (code, report) = call(&f, &skills, &["report", id]);
        assert_eq!(code, 0, "{report}");
        assert_eq!(report["data"]["observation"]["state"], "finished");
        assert!(report["data"]["report"]["content"].as_str().unwrap().contains("stub report"));
    }
}

#[test]
fn real_codex_partial_stream_and_failed_report_preserve_truth() {
    let (f, skills) = setup();
    fs::write(
        stub_body(&f.bin.join("codex")),
        r#"#!/usr/bin/python3
import os,sys,time,pathlib
hold = pathlib.Path(os.environ['HERDER_STATE_DIR']) / 'hold-partial'
pathlib.Path(sys.argv[sys.argv.index('-o')+1]).write_text('partial report')
sys.stdout.write('{"type":"thread.started","thread_id":"stub-thread"}\n{"type":')
sys.stdout.flush()
while hold.exists():
    time.sleep(0.02)
if hold.parent.is_dir():
    print('"turn.failed","error":{"message":"failed after report"}}', flush=True)
    (hold.parent / 'stream-complete').touch()
"#,
    )
    .unwrap();
    f.write("state/hold-partial", "");
    let (code, dispatch) = call(
        &f,
        &skills,
        &["dispatch", "--executor", "codex", "--repo", ".", "--brief", f.root.join("brief.md").to_str().unwrap()],
    );
    assert_eq!(code, 0, "accepted partial JSONL must survive: {dispatch}");
    fs::remove_file(f.state.join("hold-partial")).unwrap();
    wait_file(&f.state.join("stream-complete"));
    let id = dispatch["data"]["run_id"].as_str().unwrap();
    let (code, report) = call(&f, &skills, &["report", id]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["observation"]["state"], "failed");
    assert_eq!(report["data"]["report"]["content"], "partial report");

    let recipe = include_str!("../../skill/SKILL.md")
        .split("\n## ")
        .find(|section| section.starts_with("Codex runs from a background coordinator\n"))
        .and_then(|section| section.split_once("\n```sh\n"))
        .and_then(|(_, fenced)| fenced.split_once("\n```\n"))
        .map(|(recipe, _)| recipe)
        .expect("background coordinator shell recipe");
    let herder_dir = Path::new(env!("CARGO_BIN_EXE_herder")).parent().unwrap();
    let path = std::env::join_paths([herder_dir, &f.bin, Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
    let out = Command::new("sh")
        .args(["-ec", recipe])
        .current_dir(&f.repo)
        .env("HERDER_STATE_DIR", &f.state)
        .env("HERDER_SKILLS_DIR", &skills)
        .env("PATH", path)
        .env("run", id)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("partial report"), "{stdout}");
}

#[test]
fn real_status_scripts_report_stream_age_and_running_command() {
    for executor in ["codex", "vibe"] {
        let (f, skills) = setup();
        let events = if executor == "codex" {
            r#"{"type":"thread.started","thread_id":"stub-thread"}\n{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"stub-long-command"}}"#
        } else {
            r#"{"sessionId":"stub-session"}"#
        };
        f.write("state/hold-worker", "");
        let body = format!(
            "import os,pathlib,time\nprint('{events}', flush=True)\n\
             hold = pathlib.Path(os.environ['HERDER_STATE_DIR']) / 'hold-worker'\n\
             while hold.exists():\n    time.sleep(0.02)"
        );
        fs::write(stub_body(&f.bin.join(executor)), format!("#!/usr/bin/python3\n{body}\n")).unwrap();
        let (code, dispatch) = call(
            &f,
            &skills,
            &["dispatch", "--executor", executor, "--repo", ".", "--brief", f.root.join("brief.md").to_str().unwrap()],
        );
        assert_eq!(code, 0, "{dispatch}");
        let id = dispatch["data"]["run_id"].as_str().unwrap();
        let (code, status) = call(&f, &skills, &["status", id]);
        let (wait_code, wait) = call(&f, &skills, &["wait", id, "--timeout", "0"]);
        fs::remove_file(f.state.join("hold-worker")).unwrap();
        assert_eq!(wait_code, 1, "{wait}");
        let entry = &wait["data"]["runs"][0];
        assert_eq!(entry["state"], "running");
        assert_eq!(entry["progress"]["kind"], if executor == "codex" { "command_started" } else { "session_opened" });
        assert!(entry["progress"]["age_seconds"].as_u64().is_some());
        assert!(!wait.to_string().contains("stub-long-command"));
        assert_eq!(code, 0, "{status}");
        assert_eq!(status["data"]["observation"]["state"], "running", "{status}");
        let evidence = status["data"]["observation"]["evidence"].as_str().unwrap();
        let age = evidence.split_whitespace().find_map(|t| t.strip_prefix("stream_age_s=")).unwrap_or("");
        assert!(age.parse::<u64>().is_ok(), "{evidence}");
        if executor == "codex" {
            assert!(evidence.contains("running_command: stub-long-command"), "{evidence}");
        }
    }
}

#[test]
fn bundled_status_requires_jq_before_touching_executor_evidence() {
    let f = Fixture::new();
    let tools = f.root.join("path without jq");
    fs::create_dir(&tools).unwrap();
    let host_path = std::env::var_os("PATH").unwrap();
    let host_tool = |name| {
        std::env::split_paths(&host_path)
            .map(|dir| dir.join(name))
            .find(|file| file.is_file())
            .unwrap_or_else(|| panic!("missing contract fixture tool {name}"))
    };
    for name in ["bash", "cat", "grep", "head", "perl", "ps", "sort", "tail", "tr", "uniq", "wc"] {
        std::os::unix::fs::symlink(host_tool(name), tools.join(name)).unwrap();
    }
    let call = |args: &[&str], use_override: bool| {
        let mut cmd = f.cmd();
        if !use_override {
            cmd.env_remove("HERDER_SKILLS_DIR");
        }
        let out = cmd.env("PATH", &tools).args(args).arg("--json").output().unwrap();
        (out.status.code(), serde_json::from_slice::<Value>(&out.stdout).unwrap())
    };
    let mut runs = Vec::new();
    for executor in ["codex", "vibe"] {
        let id = f.launch(executor);
        let dir = f.dir(&id);
        let final_path = dir.join("final.md");
        fs::write(&final_path, "saved report\n").unwrap();
        if executor == "codex" {
            fs::write(dir.join("events.jsonl"), "{\"type\":\"turn.failed\"}\n").unwrap();
        } else {
            fs::create_dir(dir.join("out")).unwrap();
            fs::write(dir.join("out/export.json"), r#"{"outcome":"finished","config":{"active_model":"stub","models":{"stub":{"name":"stub","thinking":"high"}}}}"#).unwrap();
            fs::write(dir.join("events.jsonl"), "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"saved report\"}]}\n").unwrap();
        }
        for args in [vec!["status", &id], vec!["status", &id, "--repo", "."]] {
            let (code, response) = call(&args, false);
            assert_eq!(code, Some(0), "{response}");
            let observation = &response["data"]["observation"];
            assert_eq!(observation["state"], "unknown", "{response}");
            assert_eq!(observation["cause"], "query_failed", "{response}");
            assert!(observation["evidence"].as_str().unwrap().contains("requires jq on PATH"), "{response}");
        }
        let (code, wait) = call(&["wait", "--run-dir", dir.to_str().unwrap(), "--timeout", "0"], true);
        assert_eq!(code, Some(1), "{wait}");
        assert!(wait["error"].as_str().unwrap().contains("requires jq on PATH"), "{wait}");
        assert_eq!(fs::read_to_string(final_path).unwrap(), "saved report\n");

        f.status("exited", "COMPLETED");
        let (code, overridden) = call(&["status", &id], true);
        assert_eq!(code, Some(0), "{overridden}");
        assert_eq!(overridden["data"]["observation"]["state"], "finished", "{overridden}");
        runs.push((id, if executor == "codex" { "failed" } else { "finished" }));
    }
    std::os::unix::fs::symlink(host_tool("jq"), tools.join("jq")).unwrap();
    for (id, expected) in runs {
        let (code, status) = call(&["status", &id], false);
        assert_eq!(code, Some(0), "{status}");
        assert_eq!(status["data"]["observation"]["state"], expected, "{status}");
    }
}

#[test]
fn real_codex_status_recovers_late_identity() {
    let (f, skills) = setup();
    fs::write(f.skills.join("gate"), "").unwrap();
    let id = f.launch("codex");
    fs::write(
        f.dir(&id).join("events.jsonl"),
        "{\"type\":\"thread.started\",\"thread_id\":\"late-thread\"}\n{\"type\":\"turn.completed\"}\n",
    )
    .unwrap();
    fs::write(f.dir(&id).join("stderr.log"), "").unwrap();
    fs::write(f.dir(&id).join("final.md"), "report").unwrap();
    let (code, status) = call(&f, &skills, &["status", &id]);
    assert_eq!(code, 0);
    assert_eq!(status["data"]["run"]["launch"]["native"]["thread_id"], "late-thread");
    assert_eq!(status["data"]["observation"]["state"], "finished");
}

#[test]
fn real_status_scripts_reject_recycled_pids() {
    for executor in ["codex", "vibe"] {
        let (f, skills) = setup();
        let id = f.launch(executor);
        let mut unrelated = Command::new("/bin/cat").stdin(std::process::Stdio::piped()).spawn().unwrap();
        let mut record = f.record(&id);
        record["launch"]["native"]["pid"] = json!(unrelated.id());
        fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
        fs::write(f.dir(&id).join("run.pid"), unrelated.id().to_string()).unwrap();
        fs::write(f.dir(&id).join("events.jsonl"), "").unwrap();
        fs::write(f.dir(&id).join("stderr.log"), "").unwrap();
        let (code, status) = call(&f, &skills, &["status", &id]);
        assert!(unrelated.wait().unwrap().success());
        assert_eq!(code, 0);
        assert_ne!(status["data"]["observation"]["state"], "running", "{status}");
    }
}

#[test]
fn brief_lint_allows_review_questions_about_new_types() {
    let (f, skills) = setup();
    f.script(
        "implementer-core",
        "check_brief",
        &format!("exec bash '{}' \"$@\"", skills.join("implementer-core/scripts/check_brief.sh").display()),
    );
    let brief = fs::read_to_string(f.root.join("brief.md")).unwrap();
    f.write("brief.md", &format!("{brief}\nIs each new type necessary?\n"));
    let direct = f.dispatch("codex", &["--mode", "direct"], 1);
    assert!(direct["error"].as_str().unwrap().contains("Rejected alternative:"));
    f.dispatch("codex", &["--mode", "review"], 0);
}

#[test]
fn real_linter_accepts_generated_handoff_prompt_and_rejects_plain_dispatch() {
    let (f, skills) = setup();
    fs::write(
        stub_body(&f.bin.join("codex")),
        r#"#!/usr/bin/python3
import sys,pathlib
args=sys.argv
pathlib.Path(args[args.index('-o')+1]).write_text('stub report')
print('{"type":"thread.started","thread_id":"stub-thread"}')
print('{"type":"turn.completed"}')"#,
    )
    .unwrap();
    // A plain continuation document, including surface-check trigger words.
    let doc = "# Continuation\n\nPlain context: keep the rename, finish tests. New flag discussion deferred.\n";
    f.write("HANDOFF.md", doc);
    let brief = f.root.join("HANDOFF.md");
    let brief = brief.to_str().unwrap();
    let (code, handoff) = call(&f, &skills, &["handoff", "--executor", "codex", "--repo", ".", "--brief", brief]);
    assert_eq!(code, 0, "{handoff}");
    let id = handoff["data"]["run_id"].as_str().unwrap();
    let input = fs::read_to_string(f.dir(id).join("input.md")).unwrap();
    assert!(input.contains(doc), "{input}");
    for block in ["<task>", "<action_safety>", "<default_follow_through_policy>", "<structured_output_contract>"] {
        assert!(input.contains(block), "{block}");
    }
    // Review mode passes the real linter too and keeps the generated input read-only.
    let (code, review) =
        call(&f, &skills, &["handoff", "--executor", "codex", "--mode", "review", "--repo", ".", "--brief", brief]);
    assert_eq!(code, 0, "{review}");
    let review_id = review["data"]["run_id"].as_str().unwrap();
    assert!(fs::read_to_string(f.dir(review_id).join("input.md")).unwrap().contains("read-only"));
    // Ordinary dispatch still rejects the same plain document with the real linter.
    let (code, rejected) = call(&f, &skills, &["dispatch", "--executor", "codex", "--repo", ".", "--brief", brief]);
    assert_eq!(code, 1, "{rejected}");
    assert!(rejected["error"].as_str().unwrap().contains("missing required blocks"), "{rejected}");
    assert_eq!(f.call(&["ls"], 0)["data"]["runs"].as_array().unwrap().len(), 2);
}

#[test]
fn checked_dispatch_waits_for_a_live_stub_worker_and_verifies_through_vendored_scripts() {
    let (f, skills) = setup();
    f.write("state/hold", "");
    f.executable(
        &f.bin.join("codex"),
        r#"while [ "$1" != -o ]; do shift; done
report=$2
printf '%s\n' '{"type":"thread.started","thread_id":"stub-thread"}' '{"type":"turn.started"}'
touch "$HERDER_STATE_DIR/worker-entered"
while test -f "$HERDER_STATE_DIR/hold"; do sleep 0.02; done
printf '%s\n' 'stub report' > "$report"
printf '%s\n' '{"type":"turn.completed"}'"#,
    );
    let child = f
        .cmd()
        .env("HERDER_SKILLS_DIR", &skills)
        .args(["dispatch", "--executor", "codex", "--name", "checked", "--repo", ".", "--brief"])
        .arg(f.root.join("brief.md"))
        .args([
            "--path",
            "generated/",
            "--check",
            "test -s \"$HERDER_STATE_DIR\"/runs/*/final.md && touch \"$HERDER_STATE_DIR/checked\"",
            "--check-timeout",
            "10",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_file(&f.state.join("worker-entered"));
    // The worker emits events before the launch gate records its accepted identity.
    let run_dir = fs::read_dir(f.state.join("runs")).unwrap().next().unwrap().unwrap();
    let id = run_dir.file_name();
    let id = id.to_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.record(id)["launch"]["state"] == "starting" && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let (wait_code, wait) = call(&f, &skills, &["wait", "checked", "--timeout", "1"]);
    let checked_while_running = f.state.join("checked").exists();
    // A real Git status stream must exceed the pipe buffer to cover the previous SIGPIPE.
    fs::create_dir(f.repo.join("generated")).unwrap();
    for index in 0..1600 {
        fs::write(f.repo.join(format!("generated/changed-worker-output-{index:04}.txt")), "").unwrap();
    }
    fs::remove_file(f.state.join("hold")).unwrap();

    let out = child.wait_with_output().unwrap();
    assert_eq!(wait_code, 1, "{wait}");
    assert_eq!(wait["data"]["runs"][0]["id"], id, "{wait}");
    assert_eq!(wait["data"]["runs"][0]["state"], "running", "{wait}");
    assert_eq!(wait["data"]["runs"][0]["progress"]["kind"], "turn_started", "{wait}");
    assert!(!checked_while_running);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id}\tchecked\n"));
    assert!(String::from_utf8(out.stderr).unwrap().contains("PASS  checked  attempt 1"));
    assert!(f.state.join("checked").exists());
    let record = f.record(id);
    assert_eq!(record["verifications"][0]["verdict"], "pass");
    let collector = record["verifications"][0]["checks"][1]["output"].as_str().unwrap();
    assert_eq!(collector.lines().filter(|line| line.starts_with("?? generated/")).count(), 40);
}

#[test]
fn exact_wait_finds_a_live_worker_launched_through_a_state_alias() {
    let (f, _) = setup();
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let under_tmp = tmp.path().join("custom state");
    fs::create_dir(&under_tmp).unwrap();
    let alias = f.root.join("state-alias");
    std::os::unix::fs::symlink(&f.state, &alias).unwrap();
    let decoy_store = f.root.join("different state");
    fs::create_dir(&decoy_store).unwrap();
    f.script("codex-implementer", "status_classic", "touch \"$HERDER_SKILLS_DIR/decoy-called\"; exit 1");
    f.executable(
        &f.bin.join("rex"),
        "test -z \"${OSC7577_TOKEN+x}\"; printf '%s\\n' \"$@\" > \"$HERDER_STATE_DIR/rex-args\"",
    );
    f.executable(
        &f.bin.join("codex"),
        r#"test -z "${OSC7577_TOKEN+x}"
root=$(cd "$HERDER_STATE_DIR" && pwd -P)
printf '%s\n' "$@" > "$root/worker-argv"
while [ "$1" != -o ]; do shift; done
report=$2
printf '%s\n' '{"type":"thread.started","thread_id":"stub-thread"}'
while test -f "$root/hold"; do sleep 0.02; done
printf '%s\n' 'completed report' > "$report"
printf '%s\n' '{"type":"turn.completed"}'
touch "$root/worker-exiting""#,
    );
    for state in [&under_tmp, &alias] {
        let canonical = state.canonicalize().unwrap();
        fs::write(canonical.join("hold"), "").unwrap();
        let out = f
            .cmd()
            .env_remove("HERDER_SKILLS_DIR")
            .env("HERDER_STATE_DIR", state)
            .env("OSC7577_TOKEN", "fixture-token-only-for-test")
            .args(["dispatch", "--executor", "codex", "--repo", ".", "--pane", "--json", "--brief"])
            .arg(f.root.join("brief.md"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let dispatch: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = dispatch["data"]["run_id"].as_str().unwrap();
        let dir = canonical.join("runs").join(id);
        let worker = fs::read_to_string(canonical.join("worker-argv")).unwrap();
        assert!(worker.lines().any(|arg| Path::new(arg) == state.join("runs").join(id).join("final.md")));
        let pane = fs::read_to_string(canonical.join("rex-args")).unwrap();
        assert!(pane.ends_with(&format!(
            "\n--\n{}\nwait\n--run-dir\n{}\n",
            env!("CARGO_BIN_EXE_herder"),
            dir.display()
        )));

        let wait = || {
            let out = f
                .cmd()
                .env("HERDER_STATE_DIR", &decoy_store)
                .env("OSC7577_TOKEN", "fixture-token-only-for-test")
                .args(["wait", "--json", "--timeout", "0", "--run-dir"])
                .arg(&dir)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(1), "{out:?}");
            serde_json::from_slice::<Value>(&out.stdout).unwrap()
        };
        assert_eq!(wait()["data"]["runs"][0]["state"], "running");
        if state == &alias {
            fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&decoy_store, &alias).unwrap();
            let observed = wait();
            assert_eq!(observed["data"]["runs"][0]["state"], "unknown");
            assert_eq!(observed["data"]["runs"][0]["progress"]["cause"], "identity_mismatch");
            fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&f.state, &alias).unwrap();
        }
        assert!(!f.skills.join("decoy-called").exists());
        fs::remove_file(canonical.join("hold")).unwrap();
        wait_file(&canonical.join("worker-exiting"));
        if state == &alias {
            let finished = || f.call(&["wait", "--run-dir", dir.to_str().unwrap(), "--timeout", "2"], 0);
            assert_eq!(finished()["data"]["runs"][0]["state"], "finished");
            fs::remove_file(canonical.join("worker-exiting")).unwrap();
            fs::write(canonical.join("hold"), "").unwrap();
            let resumed = f
                .cmd()
                .env_remove("HERDER_SKILLS_DIR")
                .env("HERDER_STATE_DIR", &canonical)
                .args(["resume", id, "--json", "--brief"])
                .arg(f.root.join("brief.md"))
                .output()
                .unwrap();
            assert!(resumed.status.success(), "{resumed:?}");
            let mut record = f.record(id);
            assert_eq!(record["attempt"], 2);
            assert_eq!(record["resume_worker_dir"], json!(dir));
            assert_eq!(wait()["data"]["runs"][0]["state"], "running");

            // An attempt started before the new field still has its own launch receipt.
            record.as_object_mut().unwrap().remove("resume_worker_dir");
            fs::write(dir.join("run.json"), record.to_string()).unwrap();
            assert_eq!(wait()["data"]["runs"][0]["state"], "running");
            fs::remove_file(dir.join("launch2.log")).unwrap();
            assert_eq!(wait()["data"]["runs"][0]["state"], "unknown");

            fs::remove_file(canonical.join("hold")).unwrap();
            wait_file(&canonical.join("worker-exiting"));
            assert_eq!(finished()["data"]["runs"][0]["state"], "finished");
            let ordinary = f
                .cmd()
                .env_remove("HERDER_SKILLS_DIR")
                .env("HERDER_STATE_DIR", &alias)
                .args(["wait", id])
                .output()
                .unwrap();
            assert!(ordinary.status.success(), "{ordinary:?}");
            let original_report_path = alias.join("runs").join(id).join("final2.md");
            assert!(String::from_utf8(ordinary.stdout).unwrap().contains(original_report_path.to_str().unwrap()));
        }
    }
}

#[test]
fn checked_codex_requires_the_latest_complete_turn_event_even_with_a_report() {
    for (events, checked_code, legacy_state) in [
        ("", 1, "finished"),
        (r#"printf '%s\n' '{"type":"turn.completed"}' '{"type":"turn.started"}'"#, 1, "finished"),
        (r#"printf '%s\n' '{"type":"turn.completed"}' '{"type":"turn.failed"}'"#, 1, "failed"),
        (
            r#"printf '%s\n' '{"type":"turn.started"}' '{"type":"turn.completed"}'; printf '%s' '{"type":"turn.failed"'"#,
            0,
            "finished",
        ),
    ] {
        let (f, skills) = setup();
        f.executable(
            &f.bin.join("codex"),
            &format!(
                r#"while [ "$1" != -o ]; do shift; done
echo 'stub report' > "$2"
printf '%s\n' '{{"type":"thread.started","thread_id":"stub-thread"}}'
{events}
sleep 0.05
exit {worker_code}"#,
                worker_code = if checked_code == 0 { 0 } else { 9 },
            ),
        );
        let (code, result) = call(
            &f,
            &skills,
            &[
                "dispatch",
                "--executor",
                "codex",
                "--repo",
                ".",
                "--brief",
                f.root.join("brief.md").to_str().unwrap(),
                "--check",
                "touch \"$HERDER_STATE_DIR/checked\"",
                "--check-timeout",
                "3",
            ],
        );
        assert_eq!(code, checked_code, "{result}");
        let id = result["data"]["run_id"].as_str().unwrap();
        let (_, status) = call(&f, &skills, &["status", id]);
        assert_eq!(status["data"]["observation"]["state"], legacy_state, "{status}");
        let verified = result["data"]["verification"]["verdict"] == "pass";
        assert_eq!(checked_code == 0, verified);
        assert_eq!(f.state.join("checked").exists(), verified);
        if checked_code != 0 && legacy_state == "finished" {
            assert!(result["data"]["verification"]["message"].as_str().unwrap().contains("turn.completed"));
        }
        assert_eq!(f.record(id)["verifications"].as_array().unwrap().len(), usize::from(verified));
    }
}

#[test]
fn embedded_codex_resume_worker_finds_its_own_pid_recorded_at_startup() {
    let (f, _) = setup();
    let home = f.root.join("home");
    fs::create_dir(&home).unwrap();
    f.executable(
        &f.bin.join("codex"),
        r#"while [ "$#" -gt 0 ]; do
    if [ "$1" = -o ]; then shift; out=$1; echo 'stub report' > "$out"; break; fi
    shift
done
dir=$(dirname "$out")
echo "$$ $(cat "$dir/run.pid")" > "$dir/pid-seen-$(basename "$out")"
echo '{"type":"thread.started","thread_id":"stub-thread"}'
echo '{"type":"turn.completed"}'"#,
    );
    // An old launcher wrote run.pid from the shell after forking; disable that write so the
    // test fails unless the worker side records the pid before the worker starts.
    f.write("no-shell-printf.sh", "printf() { :; }\n");
    let call = |args: &[&str], bash_env: bool| {
        let mut cmd = f.cmd();
        cmd.env_remove("HERDER_SKILLS_DIR").env("HOME", &home);
        if bash_env {
            cmd.env("BASH_ENV", f.root.join("no-shell-printf.sh"));
        }
        let out = cmd.args(args).arg("--json").output().unwrap();
        (out.status.code().unwrap(), serde_json::from_slice::<Value>(&out.stdout).unwrap())
    };
    let brief = f.root.join("brief.md");
    let (code, dispatch) = call(
        &["dispatch", "--executor", "codex", "--mode", "review", "--repo", ".", "--brief", brief.to_str().unwrap()],
        false,
    );
    assert_eq!(code, 0, "{dispatch}");
    let id = dispatch["data"]["run_id"].as_str().unwrap();
    let (code, resumed) = call(&["resume", id, "--brief", brief.to_str().unwrap()], true);
    assert_eq!(code, 0, "{resumed}");
    let seen = fs::read_to_string(f.dir(id).join("pid-seen-final2.md")).unwrap();
    let mut pids = seen.split_whitespace();
    let own = pids.next();
    assert_eq!(pids.next(), own, "the resumed worker started before its pid was recorded: {seen:?}");
    assert_eq!(f.record(id)["attempt"], 2, "{resumed}");
}

const AGY_MODEL: &str = "gemini-4-neon-high";
const AGY_SUCCESS: &str = include_str!("../fixtures/agy/review-success.jsonl");

#[test]
fn stop_reaps_a_worker_command_in_its_own_process_group() {
    let (f, _) = setup();
    fs::remove_file(f.bin.join("kill")).unwrap();
    fs::write(
        stub_body(&f.bin.join("codex")),
        r#"#!/usr/bin/python3
import os
import signal
import subprocess
import sys
from pathlib import Path

signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
child = subprocess.Popen(["/bin/sleep", "60"], start_new_session=True)
Path(os.environ["HERDER_STATE_DIR"], "child-pid").write_text(str(child.pid))
print('{"type":"thread.started","thread_id":"stub-thread"}', flush=True)
signal.pause()
"#,
    )
    .unwrap();
    let brief = f.root.join("brief.md");
    let mut cmd = f.cmd();
    cmd.env_remove("HERDER_SKILLS_DIR");
    let launch = Fixture::call_with(
        &mut cmd,
        &["dispatch", "--executor", "codex", "--brief", brief.to_str().unwrap(), "--repo", "."],
        0,
    );
    let id = launch["data"]["run_id"].as_str().unwrap();
    let child: libc::pid_t = fs::read_to_string(f.state.join("child-pid")).unwrap().parse().unwrap();
    let stopped = f.cmd().env_remove("HERDER_SKILLS_DIR").args(["stop", id, "--json"]).output();
    // SAFETY: this positive PID belongs to the sleep the fixture just started.
    let exists = || unsafe { libc::kill(child, 0) == 0 };
    let deadline = Instant::now() + Duration::from_secs(1);
    while exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let leaked = exists();
    if leaked {
        // SAFETY: clean up the fixture's sleep before asserting the regression.
        unsafe { libc::kill(child, libc::SIGKILL) };
    }
    let stopped = stopped.unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
    let reply: Value = serde_json::from_slice(&stopped.stdout).unwrap();
    assert_eq!(reply["data"]["observation"]["state"], "stopped", "{reply}");
    assert!(!leaked, "worker's sleep {child} survived stop");
}

#[test]
fn agy_refuses_effort_extras_and_missing_model_before_recording() {
    let f = Fixture::new();
    for (args, message) in [
        (&["--model", AGY_MODEL, "--effort", "high"][..], "choose an effort in the --model slug"),
        (&["--mode", "review", "--model", AGY_MODEL, "--", "--mode", "accept-edits"], "extra arguments after --"),
        (&[], "agy requires --model"),
        (&["--model", ""], "agy requires --model"),
    ] {
        let result = f.dispatch("agy", args, 2);
        assert!(result["data"]["message"].as_str().unwrap().contains(message), "{result}");
        assert!(!f.state.join("runs").exists());
    }
}

#[test]
fn agy_resume_refuses_without_recording_an_attempt() {
    let f = Fixture::new();
    let launch = f.dispatch("agy", &["--model", AGY_MODEL], 0);
    let id = launch["data"]["run_id"].as_str().unwrap();
    f.status("exited", "COMPLETED");
    let record = f.record(id);
    let refused =
        f.call(&["resume", id, "--brief", f.root.join("brief.md").to_str().unwrap(), "--request-id", "agy-retry"], 1);
    assert!(refused["error"].as_str().unwrap().contains("agy does not support resume"));
    assert_eq!(f.record(id), record);
    assert!(!f.dir(id).join("input2.md").exists());
}

#[test]
fn agy_stop_requires_the_exact_log_marker() {
    let f = Fixture::new();
    let launch = f.dispatch("agy", &["--model", AGY_MODEL], 0);
    let id = launch["data"]["run_id"].as_str().unwrap();
    let log = f.dir(id).join("agy.log");
    fs::write(f.skills.join("ps"), format!("agy --log-file {}.other -p brief\n", log.display())).unwrap();
    let refused = f.call(&["stop", id], 1);
    assert!(refused["error"].as_str().unwrap().contains("refusing TERM"));
    assert!(!f.state.join("kill-args").exists());

    fs::write(f.skills.join("ps"), format!("agy --log-file {} -p brief\n", log.display())).unwrap();
    let stopped = f.call(&["stop", id], 0);
    assert_eq!(stopped["data"]["observation"]["state"], "stopped");
    assert_eq!(fs::read_to_string(f.state.join("kill-args")).unwrap(), "-TERM\n43210\n");
}

#[test]
fn agy_status_is_unknown_when_ps_fails() {
    let (f, _) = setup();
    let pid = std::process::id();
    f.script(
        "agy-review",
        "launch_agy",
        &format!(
            r#"echo {pid} > "$3/run.pid"
echo native-1 > "$3/conversation-id"
echo "RUN_DIR=$3 PID={pid} CONVERSATION_ID=native-1""#
        ),
    );
    let launch = f.dispatch("agy", &["--model", AGY_MODEL], 0);
    let id = launch["data"]["run_id"].as_str().unwrap();
    let dir = f.dir(id);
    fs::write(dir.join("events.jsonl"), AGY_SUCCESS).unwrap();
    f.executable(&f.bin.join("ps"), "exit 1");

    let status = || Fixture::call_with(f.cmd().env_remove("HERDER_SKILLS_DIR"), &["status", id], 0);
    let observed = status();
    let observation = &observed["data"]["observation"];
    assert_eq!(observation["state"], "unknown", "{observed}");
    assert_eq!(observation["cause"], "unrecognized_status");
    assert!(!dir.join("final.md").exists());

    f.executable(&f.bin.join("ps"), "echo unrelated-process");
    assert_eq!(status()["data"]["observation"]["state"], "finished");
    assert!(dir.join("final.md").exists());
}

#[test]
fn agy_doctor_and_launcher_require_api_key_mode() {
    let (f, _) = setup();
    let home = f.root.join("agy-home");
    let settings = home.join(AGY_SETTINGS);
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    for config in [
        None,
        Some("{"),
        Some(r#"{"modelProvider":"google"}"#),
        Some(r#"{"modelProvider":"gemini"} {"modelProvider":"gemini"}"#),
        Some(r#"{"modelProvider":"gemini","value":NaN}"#),
        Some(r#"{"modelProvider":"gemini"}"#),
    ] {
        if let Some(config) = config {
            fs::write(&settings, config).unwrap();
        }
        let healthy = config == Some(r#"{"modelProvider":"gemini"}"#);
        let mut cmd = f.cmd();
        cmd.env("HOME", &home).env_remove("GEMINI_API_KEY");
        let result = Fixture::call_with(&mut cmd, &["doctor", "--executor", "agy"], i32::from(!healthy));
        let checks = result["data"]["executors"][0]["checks"].as_array().unwrap();
        let check = checks.iter().find(|check| check["item"] == json!(settings)).unwrap();
        assert_eq!(check["ok"], healthy, "{result}");
        if !healthy {
            assert!(check["fix"].as_str().unwrap().contains(r#""modelProvider": "gemini""#));
            let mut cmd = f.cmd();
            cmd.env_remove("HERDER_SKILLS_DIR").env("HOME", &home);
            let launch = Fixture::call_with(
                &mut cmd,
                &[
                    "dispatch",
                    "--executor",
                    "agy",
                    "--model",
                    AGY_MODEL,
                    "--repo",
                    ".",
                    "--brief",
                    f.root.join("brief.md").to_str().unwrap(),
                ],
                1,
            );
            assert!(launch.to_string().contains("modelProvider"), "{launch}");
            assert!(!f.dir(launch["data"]["run_id"].as_str().unwrap()).join("run.pid").exists());
        }
    }
}

#[test]
fn agy_status_interprets_captured_streams() {
    let f = Fixture::new();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/ai-skills/agy-review/scripts/status_agy.sh");
    for (name, stream, verdict) in [
        ("review-success", AGY_SUCCESS, "COMPLETED"),
        ("denied", include_str!("../fixtures/agy/denied.jsonl"), "FAILED (denied actions: Bash)"),
        ("interrupted", include_str!("../fixtures/agy/interrupted.jsonl"), "STOPPED (interrupted)"),
        ("died", AGY_SUCCESS.lines().next().unwrap(), "DIED (no result event)"),
        (
            "error",
            r#"{"event":"result","result":{"status":"ERROR","error":"request failed"}}"#,
            "FAILED (request failed)",
        ),
        (
            "empty",
            r#"{"event":"result","result":{"status":"SUCCESS","denied_actions":[],"response":""}}"#,
            "FAILED (result status SUCCESS or empty response)",
        ),
    ] {
        let dir = f.root.join(name);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), stream).unwrap();
        if name == "denied" {
            fs::write(dir.join("stderr.log"), include_str!("../fixtures/agy/denied.stderr")).unwrap();
        }
        let out = Command::new("bash").arg(&script).arg(&dir).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(String::from_utf8(out.stdout).unwrap().contains(&format!("verdict: {verdict}\n")));
        let report = dir.join("final.md");
        if name == "review-success" {
            let result: Value = serde_json::from_str(stream.lines().last().unwrap()).unwrap();
            assert_eq!(fs::read_to_string(report).unwrap(), result["result"]["response"].as_str().unwrap());
            let init: Value = serde_json::from_str(stream.lines().next().unwrap()).unwrap();
            assert_eq!(fs::read_to_string(dir.join("conversation-id")).unwrap().trim(), init["conversation_id"]);
        } else {
            assert!(!report.exists());
        }
    }
}

#[test]
fn embedded_agy_dispatch_uses_the_review_and_direct_contracts() {
    for mode in ["review", "direct"] {
        let (f, _) = setup();
        f.write("agy-events.jsonl", AGY_SUCCESS);
        f.executable(
            &f.bin.join("agy"),
            r#"if IFS= read -r ignored; then exit 90; fi
pwd > "$HERDER_STATE_DIR/agy-cwd"
printf '%s\0' "$@" > "$HERDER_STATE_DIR/agy-args"
if [ "$7" = --mode ]; then touch unexpected.txt; fi
cat "$HERDER_STATE_DIR/../agy-events.jsonl""#,
        );
        let mut cmd = f.cmd();
        cmd.env_remove("HERDER_SKILLS_DIR").env_remove("GEMINI_API_KEY");
        let brief = f.root.join("brief.md");
        let mut args = vec![
            "dispatch",
            "--executor",
            "agy",
            "--mode",
            mode,
            "--model",
            AGY_MODEL,
            "--repo",
            ".",
            "--brief",
            brief.to_str().unwrap(),
        ];
        if mode == "review" {
            args.extend(["--check", "true"]);
        }
        let launch = Fixture::call_with(&mut cmd, &args, i32::from(mode == "review"));
        let id = launch["data"]["run_id"].as_str().unwrap();
        let dir = f.dir(id);
        let log = dir.join("agy.log");
        let input = fs::read_to_string(&brief).unwrap();
        let mut expected_args =
            vec!["--log-file", log.to_str().unwrap(), "--model", AGY_MODEL, "--output-format", "stream-json"];
        if mode == "review" {
            expected_args.extend(["--mode", "plan"]);
            assert!(launch["data"]["verification"]["scope"]["output"].as_str().unwrap().contains("unexpected.txt"));
        }
        expected_args.extend(["--dangerously-skip-permissions", "-p", &input, ""]);
        assert_eq!(fs::read(f.state.join("agy-args")).unwrap(), expected_args.join("\0").as_bytes());
        assert_eq!(fs::read_to_string(f.state.join("agy-cwd")).unwrap().trim(), f.repo.to_str().unwrap());
        let native = &f.record(id)["launch"]["native"];
        assert_eq!(native["executor"], "agy");
        let init: Value = serde_json::from_str(AGY_SUCCESS.lines().next().unwrap()).unwrap();
        assert_eq!(native["conversation_id"], init["conversation_id"]);
        let call = |args: &[&str], code| Fixture::call_with(f.cmd().env_remove("HERDER_SKILLS_DIR"), args, code);
        call(&["wait", "--run-dir", dir.to_str().unwrap()], 0);
        assert_eq!(call(&["status", id], 0)["data"]["observation"]["state"], "finished");
        assert_eq!(call(&["ls"], 0)["data"]["summary"]["agy"]["finished"], 1);
        let result: Value = serde_json::from_str(AGY_SUCCESS.lines().last().unwrap()).unwrap();
        assert_eq!(call(&["report", id], 0)["data"]["report"]["content"], result["result"]["response"]);
        if mode == "direct" {
            call(&["verify", id, "--path", "src/"], 0);
        }
    }
}
