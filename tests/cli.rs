use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;
#[path = "cases/contracts.rs"]
mod contracts;
#[path = "cases/corrections.rs"]
mod corrections;
#[path = "cases/durability.rs"]
mod durability;
#[path = "cases/lifecycle.rs"]
mod lifecycle;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    repo: PathBuf,
    skills: PathBuf,
    state: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let f = Self {
            repo: root.join("repo with spaces"),
            skills: root.join("skills"),
            state: root.join("state"),
            bin: root.join("bin"),
            root,
            _temp: temp,
        };
        for dir in [&f.repo, &f.skills, &f.state, &f.bin] {
            fs::create_dir_all(dir).unwrap();
        }
        f.git(&["init", "-q"]);
        f.git(&["commit", "--allow-empty", "-qm", "baseline"]);
        f.write("brief.md", "implement this task\n");
        for exe in ["vibe", "codex", "jq", "watch"] {
            f.executable(&f.bin.join(exe), "exit 0");
        }
        f.executable(&f.bin.join("rex"), "printf '%s\\n' \"$@\" > \"$CREW_STATE_DIR/rex-args\"; echo '{}'");
        f.executable(&f.bin.join("ps"), "cat \"$CREW_SKILLS_DIR/ps\"");
        f.executable(
            &f.bin.join("kill"),
            r#"printf '%s\n' "$@" > "$CREW_STATE_DIR/kill-args"
echo 'state=exited pid=43210' > "$CREW_SKILLS_DIR/status"
echo 'verdict: DIED' >> "$CREW_SKILLS_DIR/status""#,
        );
        f.script(
            "implementer-core",
            "check_brief",
            r#"test "$2" = direct
if test -f "$CREW_SKILLS_DIR/lint-fail"; then echo 'invalid brief' >&2; exit 1; fi"#,
        );
        f.script(
            "implementer-core",
            "collect_evidence",
            r#"printf '%s\n' "$@" > "$CREW_STATE_DIR/evidence-args"
if test -f "$CREW_SKILLS_DIR/scope-fail"; then echo 'outside authorized paths'; exit 2; fi"#,
        );
        f.executable(
            &f.bin.join("claude"),
            r#"printf '%s\n' "${CLAUDE_CONFIG_DIR:-none}" > "$CREW_STATE_DIR/profile"
case "$1" in
--bg)
  echo accepted >> "$CREW_STATE_DIR/launch-count"
  printf '%s\n' "$@" > "$CREW_STATE_DIR/claude-args"
  for arg in "$@"; do
    if test "${next:-}" = yes; then dir=$arg; break; fi
    if test "$arg" = --add-dir; then next=yes; fi
  done
  test -f "$dir/run.json"
  grep -q '"state": "starting"' "$dir/run.json"
  if test -f "$CREW_SKILLS_DIR/trust"; then echo 'Workspace not trusted. Accept trust.'; exit 1; fi
  if test -f "$CREW_SKILLS_DIR/idle"; then echo '(idle — send a prompt to start)'; exit 0; fi
  echo 'backgrounded · abc123 · worker';;
agents)
  test ! -f "$CREW_SKILLS_DIR/query-fail"
  if test -f "$CREW_SKILLS_DIR/idle-once"; then rm "$CREW_SKILLS_DIR/idle-once"; if test -f "$CREW_SKILLS_DIR/fail-after-identity"; then touch "$CREW_SKILLS_DIR/query-fail"; fi; sed 's/"busy"/"idle"/' "$CREW_SKILLS_DIR/agents.json"; exit 0; fi
  cat "$CREW_SKILLS_DIR/agents.json";;
stop)
  printf '%s\n' "$@" > "$CREW_STATE_DIR/stop-args"
  cp "$CREW_SKILLS_DIR/stopped.json" "$CREW_SKILLS_DIR/agents.json";;
*) exit 90;;
esac"#,
        );
        for (owner, suffix, key, file) in [
            ("vibe-implementer", "vibe", "SESSION_ID", "session-id"),
            ("codex-implementer", "classic", "THREAD_ID", "thread-id"),
        ] {
            f.script(
                owner,
                &format!("launch_{suffix}"),
                &format!(
                    r#"test -f "$3/run.json"
grep -q '"state": "starting"' "$3/run.json"
printf '%s\n' "$@" > "$CREW_STATE_DIR/launch-args"
cp "$2" "$3/brief.md"
echo 43210 > "$3/run.pid"
if test -f "$CREW_SKILLS_DIR/gate"; then echo 'gate elapsed alive' >&2; exit 3; fi
echo native-1 > "$3/{file}"
echo "RUN_DIR=$3 PID=43210 {key}=native-1""#
                ),
            );
            f.script(
                owner,
                &format!("status_{suffix}"),
                r#"test ! -f "$CREW_SKILLS_DIR/query-fail"
printf '%s\n' "$@" > "$CREW_STATE_DIR/status-args"
if test -f "$1/export${2:-}.txt"; then cp "$1/export${2:-}.txt" "$1/final${2:-}.md"; fi
cat "$CREW_SKILLS_DIR/status""#,
            );
            f.script(
                owner,
                &format!("resume_{suffix}"),
                &format!(
                    r#"test "$(cat "$1/{file}")" = native-1
grep -Eq '"state": "(starting|pending_resume)"' "$1/run.json"
printf '%s\n' "$@" > "$CREW_STATE_DIR/resume-args"
echo 43211 > "$1/run.pid"
echo "attempt $4" > "$1/final$4.md"
echo 'state=alive pid=43211' > "$CREW_SKILLS_DIR/status"
echo "RUN_DIR=$1 PID=43211 {key}=native-1""#
                ),
            );
        }
        f.status("alive", "RUNNING");
        f.agents("busy", "working", "native-session");
        let stopped = json!([{"id":"abc123","kind":"background","sessionId":"native-session","cwd":f.repo,"status":"idle","state":"stopped"}]);
        fs::write(f.skills.join("stopped.json"), stopped.to_string()).unwrap();
        f
    }
    fn executable(&self, path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn script(&self, owner: &str, name: &str, body: &str) {
        let dir = self.skills.join(owner).join("scripts");
        fs::create_dir_all(&dir).unwrap();
        self.executable(&dir.join(format!("{name}.sh")), body);
    }
    fn write(&self, name: &str, body: &str) {
        fs::write(self.root.join(name), body).unwrap();
    }
    fn agents(&self, status: &str, state: &str, session: &str) {
        fs::write(
            self.skills.join("agents.json"),
            json!([{"pid":1,"kind":"interactive","sessionId":"other","cwd":self.repo,"status":"idle"},{"id":"abc123","kind":"background","sessionId":session,"cwd":self.repo,"status":status,"state":state}]).to_string(),
        )
        .unwrap();
    }
    fn status(&self, state: &str, verdict: &str) {
        fs::write(self.skills.join("status"), format!("state={state} pid=43210\nverdict: {verdict}\n")).unwrap();
    }
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_crew"));
        cmd.current_dir(&self.repo)
            .env("CREW_STATE_DIR", &self.state)
            .env("CREW_SKILLS_DIR", &self.skills)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("REX_BLOCK", "test-block");
        cmd
    }
    fn call(&self, args: &[&str], expected: i32) -> Value {
        let out = self.cmd().args(args).arg("--json").output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{:?}: {} {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let data: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(data["ok"], expected == 0);
        data
    }
    fn dispatch(&self, executor: &str, extra: &[&str], expected: i32) -> Value {
        let brief = self.root.join("brief.md");
        let mut args = vec!["dispatch", "--executor", executor, "--brief", brief.to_str().unwrap(), "--repo", "."];
        args.extend(extra);
        self.call(&args, expected)
    }
    fn launch(&self, executor: &str) -> String {
        self.dispatch(executor, &[], 0)["data"]["run_id"].as_str().unwrap().into()
    }
    fn dir(&self, id: &str) -> PathBuf {
        self.state.join("runs").join(id)
    }
    fn record(&self, id: &str) -> Value {
        serde_json::from_slice(&fs::read(self.dir(id).join("run.json")).unwrap()).unwrap()
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.repo)
            .args(["-c", "user.name=Crew test", "-c", "user.email=crew@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
}

#[test]
fn claude_dispatch_records_identity_profile_and_prompt_first() {
    let f = Fixture::new();
    // A fresh background session reports idle once before it starts working.
    fs::write(f.skills.join("idle-once"), "").unwrap();
    let result = f.dispatch(
        "claude",
        &["--profile", "/tmp/explicit-profile", "--model", "model-x", "--goal", "goal-1", "--pane"],
        0,
    );
    let id = result["data"]["run_id"].as_str().unwrap();
    assert_eq!(id.len(), 20);
    let record = f.record(id);
    assert_eq!(record["launch"]["native"]["session_id"], "native-session");
    assert_eq!(record["launch"]["native"]["profile"], "/tmp/explicit-profile");
    assert_eq!(record["goal"], "goal-1");
    let args = fs::read_to_string(f.state.join("claude-args")).unwrap();
    assert!(args.starts_with("--bg\nimplement this task"));
    assert!(args.contains(&format!("Write your final report to {}/final.md", f.dir(id).display())));
    assert!(args.contains("--model\nmodel-x"));
    let pane = fs::read_to_string(f.state.join("rex-args")).unwrap();
    assert!(pane.contains("CLAUDE_CONFIG_DIR=/tmp/explicit-profile\nclaude\nattach\nabc123"));
    let status = f.call(&["status", id], 0);
    assert_eq!(status["data"]["observation"]["state"], "running");
    f.agents("idle", "waiting_for_permission", "native-session");
    assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "blocked");
    assert_eq!(f.call(&["ls", "--goal", "other"], 0)["data"]["runs"], json!([]));
}

#[test]
fn claude_launch_failures_and_lint_never_claim_success() {
    for mode in ["trust", "idle", "lint-fail"] {
        let f = Fixture::new();
        fs::write(f.skills.join(mode), "").unwrap();
        let result = f.dispatch("claude", &["--profile", "/tmp/profile"], 1);
        if mode == "lint-fail" {
            assert!(!f.state.join("runs").exists());
            assert!(!f.state.join("claude-args").exists());
        } else {
            let id = result["data"]["run_id"].as_str().unwrap();
            let record = f.record(id);
            assert_eq!(record["launch"]["state"], "failed");
            if mode == "trust" {
                assert!(
                    record["launch"]["error"]["remediation"]
                        .as_str()
                        .unwrap()
                        .contains("under profile /tmp/profile once and accept trust")
                );
            } else {
                assert!(record["launch"]["error"]["message"].as_str().unwrap().contains("idle"));
            }
        }
    }
}

#[test]
fn script_dispatch_status_and_report_use_executor_artifacts() {
    for executor in ["vibe", "codex"] {
        let f = Fixture::new();
        let result = f.dispatch(executor, &["--model", "model-x", "--pane"], 0);
        let id = result["data"]["run_id"].as_str().unwrap();
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "running");
        let missing = f.call(&["report", id], 1);
        assert_eq!(missing["data"]["report"]["message"], "no report yet");
        assert_eq!(missing["data"]["observation"]["state"], "running");
        f.status("exited", "COMPLETED");
        fs::write(f.dir(id).join("export.txt"), "worker report").unwrap();
        let report = f.call(&["report", id], 0);
        assert_eq!(report["data"]["report"]["content"], "worker report");
        assert_eq!(report["data"]["observation"]["state"], "finished");
        let args = fs::read_to_string(f.state.join("launch-args")).unwrap();
        assert!(args.contains("input.md"));
        assert!(args.contains("--model\nmodel-x"));
        let pane = fs::read_to_string(f.state.join("rex-args")).unwrap();
        assert!(pane.contains("watch\n-n\n2\n-x\n"));
    }
}

#[test]
fn resume_selects_current_attempt_report_and_retains_native_session() {
    for executor in ["vibe", "codex"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        f.status("exited", "COMPLETED");
        fs::write(f.dir(&id).join("final.md"), "old report").unwrap();
        let brief = f.root.join("brief.md");
        let resumed = f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 0);
        assert_eq!(resumed["data"]["run"]["attempt"], 2);
        assert_eq!(resumed["data"]["run"]["launch"]["native"]["pid"], 43211);
        let report = f.call(&["report", &id], 0);
        assert_eq!(report["data"]["report"]["content"], "attempt 2\n");
        assert!(report["data"]["report"]["path"].as_str().unwrap().ends_with("final2.md"));
        assert!(fs::read_to_string(f.state.join("resume-args")).unwrap().ends_with("\n2\n"));
        fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
        let state = f.call(&["status", &id], 0);
        assert_eq!(state["data"]["observation"]["state"], "finished");
    }
    let f = Fixture::new();
    let id = f.launch("claude");
    let error = f.call(&["resume", &id, "--brief", "delta.md"], 1);
    assert!(error["error"].as_str().unwrap().contains("claude attach abc123"));
    assert_eq!(f.record(&id)["attempt"], 1);
}

#[test]
fn stop_signals_only_confirmed_recorded_identity() {
    for executor in ["claude", "vibe", "codex"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        let marker = if executor == "vibe" { "out" } else { "final.md" };
        fs::write(f.skills.join("ps"), format!("{executor} --output {}\n", f.dir(&id).join(marker).display())).unwrap();
        let stopped = f.call(&["stop", &id], 0);
        assert_eq!(stopped["data"]["observation"]["state"], "stopped");
        let (file, expected) =
            if executor == "claude" { ("stop-args", "stop\nabc123\n") } else { ("kill-args", "-TERM\n43210\n") };
        assert_eq!(fs::read_to_string(f.state.join(file)).unwrap(), expected);
    }
}

#[test]
fn mismatched_identity_or_reused_pid_is_never_stopped_or_resumed() {
    for mode in ["pid", "native", "ps", "claude"] {
        let f = Fixture::new();
        let id = f.launch(if mode == "claude" { "claude" } else { "vibe" });
        match mode {
            "pid" => fs::write(f.dir(&id).join("run.pid"), "43212").unwrap(),
            "native" => fs::write(f.dir(&id).join("session-id"), "different-session").unwrap(),
            "ps" => fs::write(f.skills.join("ps"), "unrelated process").unwrap(),
            _ => f.agents("busy", "working", "different-session"),
        }
        f.call(&["stop", &id], 1);
        assert!(!f.state.join("kill-args").exists());
        assert!(!f.state.join("stop-args").exists());
        if mode != "ps" {
            f.call(&["resume", &id, "--brief", "delta.md"], 1);
            assert!(!f.state.join("resume-args").exists());
        }
    }
}

#[test]
fn unavailable_executor_is_unknown_on_every_read() {
    for executor in ["claude", "vibe", "codex"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        fs::write(f.skills.join("query-fail"), "").unwrap();
        let status = f.call(&["status", &id], 0);
        assert_eq!(status["data"]["observation"]["state"], "unknown");
        assert!(status["data"]["observation"]["evidence"].as_str().unwrap().contains("recorded=Running"));
        let list = f.call(&["ls"], 0);
        assert_eq!(list["data"]["runs"][0]["observation"]["state"], "unknown");
        fs::remove_file(f.skills.join("query-fail")).unwrap();
        assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "running");
    }
}

#[test]
fn gate_elapsed_preserves_intent_and_recovers_late_identity() {
    for executor in ["vibe", "codex"] {
        let f = Fixture::new();
        fs::write(f.skills.join("gate"), "").unwrap();
        let id = f.launch(executor);
        assert_eq!(f.record(&id)["launch"]["state"], "starting");
        assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "unknown");
        let file = if executor == "vibe" { "session-id" } else { "thread-id" };
        fs::write(f.dir(&id).join(file), "native-1").unwrap();
        assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "running");
        assert_eq!(f.record(&id)["launch"]["state"], "launched");
    }
}

#[test]
fn verification_records_scope_and_each_check_without_changing_lifecycle() {
    for mode in ["pass", "scope-fail", "check-fail"] {
        let f = Fixture::new();
        let dispatch = f.dispatch("vibe", &["--path", "src/"], 0);
        let id = dispatch["data"]["run_id"].as_str().unwrap();
        if mode == "scope-fail" {
            fs::write(f.skills.join(mode), "").unwrap();
        }
        let check = if mode == "check-fail" { "exit 7" } else { "pwd" };
        let verification =
            f.call(&["verify", id, "--check", check, "--check", "printf last"], if mode == "pass" { 0 } else { 1 });
        let record = &verification["data"]["run"];
        assert_eq!(record["launch"]["intent"]["state"], "running");
        let verdict = &record["verifications"][0];
        assert_eq!(verdict["verdict"], if mode == "pass" { "pass" } else { "fail" });
        assert_eq!(verdict["checks"][0]["exit_code"], 0);
        assert_eq!(verdict["checks"][1]["exit_code"], if mode == "scope-fail" { 2 } else { 0 });
        assert_eq!(verdict["checks"][2]["exit_code"], if mode == "check-fail" { 7 } else { 0 });
        assert_eq!(verdict["checks"][3]["output"], "last");
        if mode != "check-fail" {
            assert_eq!(verdict["checks"][2]["output"], format!("{}\n", f.repo.display()));
        }
        assert!(
            fs::read_to_string(f.state.join("evidence-args")).unwrap().ends_with(&format!("{}\n", f.repo.display()))
        );
        assert_eq!(f.record(id)["verifications"], record["verifications"]);
    }
    let f = Fixture::new();
    let id = f.launch("codex");
    f.call(&["verify", &id], 2);
    f.call(&["verify", &id, "--path", "tests/"], 0);
}

#[test]
fn issue_events_fold_and_preserve_append_only_history() {
    let f = Fixture::new();
    let logged = f.call(
        &[
            "issue",
            "log",
            "--title",
            "t",
            "--observed",
            "o",
            "--expected",
            "e",
            "--area",
            "executor:vibe",
            "--evidence",
            "a",
            "--evidence",
            "b",
        ],
        0,
    );
    let id = logged["data"]["issue"]["id"].as_str().unwrap();
    let before = fs::read(f.state.join("issues.jsonl")).unwrap();
    assert_eq!(f.call(&["issue", "ls", "--open"], 0)["data"]["issues"].as_array().unwrap().len(), 1);
    f.call(&["issue", "resolve", id, "--note", "fixed"], 0);
    let all = f.call(&["issue", "ls"], 0);
    assert_eq!(all["data"]["issues"][0]["resolution"]["note"], "fixed");
    assert_eq!(all["data"]["issues"][0]["log"]["evidence"], json!(["a", "b"]));
    assert!(fs::read(f.state.join("issues.jsonl")).unwrap().starts_with(&before));
    assert_eq!(f.call(&["issue", "ls", "--open"], 0)["data"]["issues"], json!([]));
    f.call(&["issue", "resolve", "missing", "--note", "x"], 1);
}

#[test]
fn doctor_resolves_profile_precedence_and_reports_exact_gaps() {
    let f = Fixture::new();
    for (extra, env, expected) in [
        (vec![], None, Value::Null),
        (vec![], Some("/tmp/env"), json!("/tmp/env")),
        (vec!["--profile", "/tmp/flag"], Some("/tmp/env"), json!("/tmp/flag")),
    ] {
        let mut cmd = f.cmd();
        cmd.args(["doctor", "--json"]);
        cmd.args(extra);
        if let Some(profile) = env {
            cmd.env("CLAUDE_CONFIG_DIR", profile);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success());
        let data: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(data["data"]["claude_profile"], expected);
        assert_eq!(data["data"]["rex"]["fix"], Value::Null);
        assert_eq!(data["data"]["pane_loop"]["fix"], Value::Null);
        assert!(data["data"]["rex"].get("detail").is_none());
    }
    let out = f.cmd().args(["doctor", "--executor", "claude"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("ok claude: claude\n"));
    assert!(text.contains("claude profile: none\nok rex\nok watch\n"));
    assert!(!text.contains("fix:"));
    f.executable(&f.bin.join("rex"), "echo private-identity; exit 1");
    fs::remove_file(f.bin.join("watch")).unwrap();
    fs::remove_file(f.bin.join("vibe")).unwrap();
    let doctor = f.call(&["doctor", "--executor", "vibe"], 1);
    assert!(doctor.to_string().contains("install vibe and put its executable on PATH"));
    assert!(doctor["data"]["rex"]["fix"].is_string());
    assert!(doctor["data"]["pane_loop"]["fix"].is_string());
    assert!(!doctor.to_string().contains("private-identity"));
    let out = f.cmd().args(["doctor", "--executor", "vibe"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("missing vibe: vibe; fix: install vibe and put its executable on PATH"));
    assert!(text.contains("missing rex; fix:"));
    assert!(text.contains("missing watch; fix:"));
}

#[test]
fn every_help_has_example_and_usage_errors_are_json() {
    let f = Fixture::new();
    for command in [
        vec![],
        vec!["doctor"],
        vec!["dispatch"],
        vec!["ls"],
        vec!["status"],
        vec!["report"],
        vec!["stop"],
        vec!["resume"],
        vec!["verify"],
        vec!["issue"],
        vec!["issue", "log"],
        vec!["issue", "ls"],
        vec!["issue", "resolve"],
    ] {
        let out = f.cmd().args(command).arg("--help").output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Example:"));
    }
    assert!(f.call(&["dispatch", "--executor", "bad"], 2)["error"].as_str().unwrap().contains("error:"));
    f.call(&["status", "../escape"], 1);
}

#[test]
fn non_claude_profile_is_a_usage_error() {
    let f = Fixture::new();
    let result = f.dispatch("vibe", &["--profile", "/tmp/profile"], 2);
    assert_eq!(result["data"]["message"], "--profile applies only to Claude");
    assert!(!f.state.join("runs").exists());
}

#[test]
fn failed_launch_observation_keeps_error_without_querying() {
    for mode in ["trust", "idle"] {
        let f = Fixture::new();
        fs::write(f.skills.join(mode), "").unwrap();
        let result = f.dispatch("claude", &["--profile", "/tmp/profile"], 1);
        let id = result["data"]["run_id"].as_str().unwrap();
        let record = f.record(id);
        f.executable(&f.bin.join("claude"), "touch \"$CREW_STATE_DIR/queried\"; exit 99");
        for args in [vec!["status", id], vec!["ls"], vec!["report", id]] {
            let result = f.call(&args, if args[0] == "report" { 1 } else { 0 });
            let data = if args[0] == "ls" { &result["data"]["runs"][0] } else { &result["data"] };
            assert_eq!(data["observation"]["state"], "failed");
            let evidence = data["observation"]["evidence"].as_str().unwrap();
            for field in ["message", "remediation"] {
                assert!(evidence.contains(record["launch"]["error"][field].as_str().unwrap()));
            }
        }
        assert!(!f.state.join("queried").exists());
        assert_eq!(f.record(id), record);
    }
}

#[test]
fn human_lists_and_status_show_run_facts() {
    for executor in ["claude", "vibe", "codex"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        for (args, expected) in [
            (vec!["ls"], vec![format!("{id}\t-\t-\t{executor}\trunning\t1")]),
            (
                vec!["status", &id],
                vec![
                    "state: running".into(),
                    "evidence:\n  ".into(),
                    format!("executor: {executor}"),
                    "attempt: 1".into(),
                    format!("run dir: {}", f.dir(&id).display()),
                    format!("report path: {}/final.md", f.dir(&id).display()),
                    "latest verdict: none".into(),
                ],
            ),
        ] {
            let out = f.cmd().args(args).output().unwrap();
            assert!(out.status.success());
            let text = String::from_utf8(out.stdout).unwrap();
            assert!(!text.starts_with('{'));
            for expected in expected {
                assert!(text.contains(&expected), "{text}");
            }
        }
        if executor != "claude" {
            f.call(&["verify", &id, "--path", "src/"], 0);
            f.status("exited", "COMPLETED");
            let out = f.cmd().args(["resume", &id, "--brief"]).arg(f.root.join("brief.md")).output().unwrap();
            assert!(out.status.success());
            assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id} attempt 2: running\n"));
            let out = f.cmd().args(["status", &id]).output().unwrap();
            assert!(out.status.success());
            let text = String::from_utf8(out.stdout).unwrap();
            for expected in ["attempt: 2", "final2.md", "latest verdict: none"] {
                assert!(text.contains(expected), "{text}");
            }
        }
    }
}

#[test]
fn human_actions_and_multiline_evidence() {
    let f = Fixture::new();
    let id = f.dispatch("codex", &["--name", "worker\nname", "--goal", "polish"], 0)["data"]["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = f.cmd().arg("ls").output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id}\tworker\\nname\tpolish\tcodex\trunning\t1\n"));
    let check = "echo first; echo second; exit 1";
    for (args, code, expected) in [
        (vec!["verify", &id], 2, "verify requires --path when none were recorded at dispatch\n".to_owned()),
        (vec!["verify", &id, "--path", "src/"], 0, "pass attempt 1\n".into()),
        (vec!["verify", &id, "--path", "src/", "--check", check], 1, format!("fail attempt 1\n{check}: first\n")),
        (
            vec!["issue", "log", "--title", "test", "--observed", "actual", "--expected", "expected"],
            0,
            "issue-0001\n".into(),
        ),
        (vec!["issue", "resolve", "issue-0001", "--note", "fixed"], 0, "issue-0001 resolved\n".into()),
    ] {
        let out = f.cmd().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(code));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
    }
    fs::write(f.skills.join("ps"), format!("codex -o {}\n", f.dir(&id).join("final.md").display())).unwrap();
    let kill = fs::read(f.bin.join("kill")).unwrap();
    f.executable(&f.bin.join("kill"), "exit 0");
    let out = f.cmd().args(["stop", &id]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with(&format!("{id}: running\nstop sent; exit not confirmed:")), "{text}");
    fs::write(f.bin.join("kill"), kill).unwrap();
    let out = f.cmd().args(["stop", &id]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id}: stopped\n"));
    let out = f.cmd().args(["status", &id]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("state: stopped\nevidence:\n  stopped by crew stop; state=exited pid=43210\n  verdict: DIED\n"),
        "{text}"
    );
    assert!(!text.contains("\\n"), "{text}");
}

#[test]
fn issue_free_text_accepts_leading_hyphens() {
    let f = Fixture::new();
    let value = "--disallowedTools ate the prompt";
    let logged = f.call(
        &[
            "issue",
            "log",
            "--title",
            value,
            "--observed",
            value,
            "--expected",
            value,
            "--evidence",
            value,
            "--evidence",
            "-second",
        ],
        0,
    );
    for field in ["title", "observed", "expected"] {
        assert_eq!(logged["data"]["issue"][field], value);
    }
    assert_eq!(logged["data"]["issue"]["evidence"], json!([value, "-second"]));
    let id = logged["data"]["issue"]["id"].as_str().unwrap();
    for (resolved, expected) in [(false, "open"), (true, "resolved")] {
        if resolved {
            assert_eq!(f.call(&["issue", "resolve", id, "--note", value], 0)["data"]["issue"]["note"], value);
        }
        let out = f.cmd().args(["issue", "ls"]).output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{id}\tcrew\t{value}\t{expected}\n"));
    }
}

#[test]
fn accepted_claude_gate_failure_recovers_without_relaunch() {
    for mode in ["json", "query", "post-identity"] {
        let f = Fixture::new();
        match mode {
            "json" => fs::write(f.skills.join("agents.json"), "not json").unwrap(),
            "query" => fs::write(f.skills.join("query-fail"), "").unwrap(),
            _ => {
                fs::write(f.skills.join("idle-once"), "").unwrap();
                fs::write(f.skills.join("fail-after-identity"), "").unwrap();
            }
        }
        let result = f.dispatch("claude", &["--profile", "/tmp/profile"], 1);
        let id = result["data"]["run_id"].as_str().unwrap();
        let record = f.record(id);
        let launch = &record["launch"];
        let error = if mode == "post-identity" {
            assert_eq!(launch["state"], "launched");
            assert_eq!(launch["native"]["session_id"], "native-session");
            assert_eq!(launch["intent"]["state"], "unconfirmed");
            &launch["intent"]["error"]
        } else {
            assert_eq!(launch["state"], "accepted");
            assert_eq!(launch["id"], "abc123");
            assert_eq!(launch["profile"], "/tmp/profile");
            assert!(launch.get("native").is_none());
            &launch["error"]
        };
        assert!(result["data"]["launch_error"].as_str().unwrap().contains("do not relaunch"));
        assert!(!error["message"].as_str().unwrap().is_empty());
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "unknown");
        assert_eq!(f.record(id), record);
        f.call(&["stop", id], 1);
        assert!(!f.state.join("stop-args").exists());
        if mode != "json" {
            fs::remove_file(f.skills.join("query-fail")).unwrap();
        }
        f.agents("busy", "working", "native-session");
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "running");
        let recovered = f.record(id);
        assert_eq!(recovered["launch"]["native"]["session_id"], "native-session");
        assert_eq!(recovered["launch"]["native"]["profile"], "/tmp/profile");
        assert_eq!(recovered["launch"]["intent"]["error"], *error);
        f.agents("idle", "done", "native-session");
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "finished");
        assert_eq!(fs::read_to_string(f.state.join("launch-count")).unwrap(), "accepted\n");
    }
}

#[test]
fn accepted_claude_identity_must_validate_before_observation_or_stop() {
    for mode in ["cwd", "empty-session"] {
        let f = Fixture::new();
        f.agents("busy", "working", if mode == "empty-session" { "" } else { "native-session" });
        if mode == "cwd" {
            let path = f.skills.join("agents.json");
            let mut agents: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            agents[1]["cwd"] = json!(f.root);
            fs::write(path, agents.to_string()).unwrap();
        }
        let result = f.dispatch("claude", &[], 1);
        let id = result["data"]["run_id"].as_str().unwrap();
        let record = f.record(id);
        assert_eq!(record["launch"]["state"], "accepted");
        assert!(record["launch"]["error"]["message"].as_str().unwrap().contains("identity mismatch"));
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "unknown");
        f.call(&["stop", id], 1);
        assert!(!f.state.join("stop-args").exists());
        assert_eq!(f.record(id), record);
        f.agents("busy", "working", "native-session");
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "running");
        f.agents("busy", "working", "different-session");
        assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "unknown");
        f.call(&["stop", id], 1);
        assert!(!f.state.join("stop-args").exists());
        f.agents("busy", "working", "native-session");
        assert_eq!(f.call(&["stop", id], 0)["data"]["observation"]["state"], "stopped");
        assert_eq!(fs::read_to_string(f.state.join("launch-count")).unwrap(), "accepted\n");
    }
}
