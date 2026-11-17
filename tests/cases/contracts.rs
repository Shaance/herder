use super::*;
use std::{thread, time::Duration};

fn setup() -> (Fixture, PathBuf) {
    let f = Fixture::new();
    fs::remove_file(f.bin.join("ps")).unwrap();
    let skills = std::env::var_os("CREW_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/ai-skills"));
    let skills = std::path::absolute(skills).unwrap();
    assert!(
        skills.join("codex-implementer/scripts/launch_classic.sh").is_file(),
        "contract tests require the vendored scripts or CREW_SKILLS_DIR pointing to a skills directory"
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
    let out = f.cmd().env("CREW_SKILLS_DIR", skills).args(args).arg("--json").output().unwrap();
    (out.status.code().unwrap(), serde_json::from_slice(&out.stdout).unwrap())
}

#[test]
fn embedded_scripts_run_dispatch_lint_launch_status_and_verify() {
    let (f, _) = setup();
    let home = f.root.join("empty-home");
    fs::create_dir(&home).unwrap();
    f.executable(
        &f.bin.join("codex"),
        r#"while [ "$#" -gt 0 ]; do
    if [ "$1" = -o ]; then shift; echo 'stub report' > "$1"; break; fi
    shift
done
echo '{"type":"thread.started","thread_id":"stub-thread"}'
echo '{"type":"turn.completed"}'"#,
    );
    let call = |args: &[&str]| {
        let out = f.cmd().env_remove("CREW_SKILLS_DIR").env("HOME", &home).args(args).arg("--json").output().unwrap();
        (out.status.code().unwrap(), serde_json::from_slice::<Value>(&out.stdout).unwrap())
    };
    f.write("invalid.md", "A plain brief.\n");
    let invalid = f.root.join("invalid.md");
    let (code, rejected) =
        call(&["dispatch", "--executor", "codex", "--repo", ".", "--brief", invalid.to_str().unwrap()]);
    assert_eq!(code, 1, "{rejected}");
    assert!(rejected["error"].as_str().unwrap().contains("missing required blocks"), "{rejected}");

    let brief = f.root.join("brief.md");
    let (code, dispatch) =
        call(&["dispatch", "--executor", "codex", "--repo", ".", "--brief", brief.to_str().unwrap(), "--path", "."]);
    assert_eq!(code, 0, "{dispatch}");
    assert_eq!(dispatch["data"]["launch_error"], Value::Null, "{dispatch}");
    let id = dispatch["data"]["run_id"].as_str().unwrap();
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
}

#[test]
fn real_launch_and_status_scripts_accept_stub_executors() {
    for executor in ["codex", "vibe"] {
        let (f, skills) = setup();
        let body = if executor == "codex" {
            r#"import sys,pathlib
args=sys.argv
pathlib.Path(args[args.index('-o')+1]).write_text('stub report')
print('{"type":"thread.started","thread_id":"stub-thread"}')
print('{"type":"turn.completed"}')"#
        } else {
            r#"import sys,pathlib,json
args=sys.argv; out=pathlib.Path(args[args.index('--output-dir')+1]); out.mkdir()
(out/'export.json').write_text(json.dumps({'outcome':'finished','config':{'active_model':'stub','models':{'stub':{'name':'stub','thinking':'high'}}}}))
print('{"sessionId":"stub-session"}')
print('{"type":"message","role":"assistant","content":[{"type":"text","text":"stub report"}]}')"#
        };
        fs::write(f.bin.join(executor), format!("#!/usr/bin/python3\n{body}\n")).unwrap();
        let (code, dispatch) = call(
            &f,
            &skills,
            &["dispatch", "--executor", executor, "--repo", ".", "--brief", f.root.join("brief.md").to_str().unwrap()],
        );
        assert_eq!(code, 0, "{dispatch}");
        let id = dispatch["data"]["run_id"].as_str().unwrap();
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
        f.bin.join("codex"),
        r#"#!/usr/bin/python3
import sys,time,pathlib
pathlib.Path(sys.argv[sys.argv.index('-o')+1]).write_text('partial report')
sys.stdout.write('{"type":"thread.started","thread_id":"stub-thread"}\n{"type":')
sys.stdout.flush()
time.sleep(3)
print('"turn.failed","error":{"message":"failed after report"}}')
"#,
    )
    .unwrap();
    let (code, dispatch) = call(
        &f,
        &skills,
        &["dispatch", "--executor", "codex", "--repo", ".", "--brief", f.root.join("brief.md").to_str().unwrap()],
    );
    // The stub always exits itself, including when the launch contract is broken.
    thread::sleep(Duration::from_secs(3));
    assert_eq!(code, 0, "accepted partial JSONL must survive: {dispatch}");
    let id = dispatch["data"]["run_id"].as_str().unwrap();
    let (code, report) = call(&f, &skills, &["report", id]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["data"]["observation"]["state"], "failed");
    assert_eq!(report["data"]["report"]["content"], "partial report");
}

#[test]
fn real_status_scripts_report_stream_age_and_running_command() {
    for executor in ["codex", "vibe"] {
        let (f, skills) = setup();
        let body = if executor == "codex" {
            r#"import sys,time
print('{"type":"thread.started","thread_id":"stub-thread"}')
print('{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"stub-long-command"}}', flush=True)
time.sleep(6)"#
        } else {
            r#"import time
print('{"sessionId":"stub-session"}', flush=True)
time.sleep(6)"#
        };
        fs::write(f.bin.join(executor), format!("#!/usr/bin/python3\n{body}\n")).unwrap();
        let (code, dispatch) = call(
            &f,
            &skills,
            &["dispatch", "--executor", executor, "--repo", ".", "--brief", f.root.join("brief.md").to_str().unwrap()],
        );
        assert_eq!(code, 0, "{dispatch}");
        let id = dispatch["data"]["run_id"].as_str().unwrap();
        let (code, status) = call(&f, &skills, &["status", id]);
        assert_eq!(code, 0, "{status}");
        assert_eq!(status["data"]["observation"]["state"], "running", "{status}");
        let evidence = status["data"]["observation"]["evidence"].as_str().unwrap();
        let age = evidence.split_whitespace().find_map(|t| t.strip_prefix("stream_age_s=")).unwrap_or("");
        assert!(age.parse::<u64>().is_ok(), "{evidence}");
        if executor == "codex" {
            assert!(evidence.contains("running_command: stub-long-command"), "{evidence}");
        }
        thread::sleep(Duration::from_secs(6));
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
        let mut unrelated = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let mut record = f.record(&id);
        record["launch"]["native"]["pid"] = json!(unrelated.id());
        fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
        fs::write(f.dir(&id).join("run.pid"), unrelated.id().to_string()).unwrap();
        fs::write(f.dir(&id).join("events.jsonl"), "").unwrap();
        fs::write(f.dir(&id).join("stderr.log"), "").unwrap();
        let (code, status) = call(&f, &skills, &["status", &id]);
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert_eq!(code, 0);
        assert_ne!(status["data"]["observation"]["state"], "running", "{status}");
    }
}

#[test]
fn real_linter_accepts_generated_handoff_prompt_and_rejects_plain_dispatch() {
    let (f, skills) = setup();
    fs::write(
        f.bin.join("codex"),
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
