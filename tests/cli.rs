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
#[path = "cases/handoff.rs"]
mod handoff;
#[path = "cases/lifecycle.rs"]
mod lifecycle;
#[path = "cases/workflow.rs"]
mod workflow;

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
            r#"case "$2" in direct|review) ;; *) exit 2 ;; esac
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
  if test "$2" = --resume; then
    printf '%s\n' "${CLAUDE_CONFIG_DIR:-none}" > "$CREW_STATE_DIR/resume-profile"
    test "$#" = 4
    test "$3" = native-session
    test "$(pwd)" = "$(cat "$CREW_STATE_DIR/resume-repo")"
    printf '%s\n' "$@" > "$CREW_STATE_DIR/resume-args"
    if test -f "$CREW_SKILLS_DIR/resume-output"; then cat "$CREW_SKILLS_DIR/resume-output"; exit 0; fi
    if test -f "$CREW_SKILLS_DIR/delayed-wake"; then touch "$CREW_SKILLS_DIR/waking"; echo 'backgrounded · abc123 · worker'; exit 0; fi
    if test -f "$CREW_SKILLS_DIR/resume-copy"; then
      if ! grep -q '^id-only$' "$CREW_SKILLS_DIR/resume-copy"; then
        echo 'note: background session abc123 keeps its own saved options, so the flags you passed started a copy as def456. Without flags, the same command continues abc123 itself.'
      fi
      if ! grep -q '^note-only$' "$CREW_SKILLS_DIR/resume-copy"; then echo 'backgrounded · def456 · worker'; fi
      exit 0
    fi
    grep -q '"status":null' "$CREW_SKILLS_DIR/agents.json"
    report=$(printf '%s' "$4" | sed -n 's/^Write your final report to \(.*\)\.$/\1/p')
    printf 'attempt 2\n' > "$report"
    sed 's/"stopped"/"done"/' "$CREW_SKILLS_DIR/agents.json" > "$CREW_SKILLS_DIR/resumed.json"
    mv "$CREW_SKILLS_DIR/resumed.json" "$CREW_SKILLS_DIR/agents.json"
    echo 'prompt: started a copy as deadbeef'
    echo 'backgrounded · abc123 · worker'
    exit 0
  fi
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
  if test -f "$CREW_SKILLS_DIR/waking"; then
    rm "$CREW_SKILLS_DIR/waking"
    touch "$CREW_SKILLS_DIR/awake"
    sed 's/"stopped"/"done"/' "$CREW_SKILLS_DIR/agents.json"
    exit 0
  fi
  if test -f "$CREW_SKILLS_DIR/awake"; then sed 's/"status":null/"status":"busy"/' "$CREW_SKILLS_DIR/agents.json"; exit 0; fi
  test ! -f "$CREW_SKILLS_DIR/query-fail"
  if test -f "$CREW_SKILLS_DIR/idle-once"; then rm "$CREW_SKILLS_DIR/idle-once"; if test -f "$CREW_SKILLS_DIR/fail-after-identity"; then touch "$CREW_SKILLS_DIR/query-fail"; fi; sed 's/"busy"/"idle"/' "$CREW_SKILLS_DIR/agents.json"; exit 0; fi
  cat "$CREW_SKILLS_DIR/agents.json";;
stop)
  printf '%s\n' "${CLAUDE_CONFIG_DIR:-none}" > "$CREW_STATE_DIR/stop-profile"
  printf '%s\n' "$@" > "$CREW_STATE_DIR/stop-args"
  echo "$2" >> "$CREW_STATE_DIR/stops"
  if test -f "$CREW_SKILLS_DIR/stop-live"; then exit 0; fi
  if test "$2" != abc123 && test -f "$CREW_SKILLS_DIR/stop-fail"; then exit 1; fi
  if test "$2" = def456; then exit 0; fi
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
if test -f "$CREW_SKILLS_DIR/drop-brief"; then rm "$CREW_SKILLS_DIR/../brief.md"; fi
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
        let stopped = json!([{"id":"abc123","kind":"background","sessionId":"native-session","cwd":f.repo,"status":null,"state":"stopped"}]);
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
        let out = self.cmd().arg("--json").args(args).output().unwrap();
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
fn human_launch_failures_and_rejections_keep_their_output_streams() {
    let f = Fixture::new();
    fs::write(f.skills.join("trust"), "").unwrap();
    let brief = f.root.join("brief.md");
    let name = "worker\nname";
    let out = f
        .cmd()
        .args(["dispatch", "--executor", "claude", "--brief"])
        .arg(&brief)
        .args(["--repo", ".", "--profile", "/tmp/profile", "--name", name])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let (id, displayed_name) = stdout.trim_end().split_once('\t').unwrap();
    assert_eq!(displayed_name, name.escape_debug().to_string().escape_debug().to_string());
    let record = f.record(id);
    let error = &record["launch"]["error"];
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!("run {id}: {}; {}\n", error["message"].as_str().unwrap(), error["remediation"].as_str().unwrap())
    );

    let out = f
        .cmd()
        .env_remove("REX_BLOCK")
        .args(["dispatch", "--executor", "codex", "--brief"])
        .arg(&brief)
        .args(["--repo", ".", "--pane"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stdout).unwrap().contains('\t'));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "worker recorded; pane failed: environment variable not found\n"
    );

    let out = f
        .cmd()
        .args(["dispatch", "--executor", "vibe", "--brief"])
        .arg(&brief)
        .args(["--repo", ".", "--profile", "/tmp/profile"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "{\n  \"message\": \"--profile applies only to Claude\"\n}\n");
    assert!(out.stderr.is_empty());
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
        assert!(pane.contains(&format!(
            "--shell=none\n--\nenv\nCREW_STATE_DIR={}\n{}\nwait\n{id}\n",
            f.state.display(),
            env!("CARGO_BIN_EXE_crew")
        )));
        assert!(!pane.contains("watch"));
    }
}

#[test]
fn resume_selects_current_attempt_report_and_retains_native_session() {
    for executor in ["vibe", "codex"] {
        let f = Fixture::new();
        let result = f.dispatch(executor, &["--model", "model-x", "--", "-c", "key=value with spaces"], 0);
        let id = result["data"]["run_id"].as_str().unwrap().to_owned();
        assert_eq!(f.record(&id)["extra"], json!(["-c", "key=value with spaces"]));
        f.status("exited", "COMPLETED");
        fs::write(f.dir(&id).join("final.md"), "old report").unwrap();
        let brief = f.root.join("brief.md");
        let resumed = f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 0);
        assert_eq!(resumed["data"]["run"]["attempt"], 2);
        assert_eq!(resumed["data"]["run"]["launch"]["native"]["pid"], 43211);
        let report = f.call(&["report", &id], 0);
        assert_eq!(report["data"]["report"]["content"], "attempt 2\n");
        assert!(report["data"]["report"]["path"].as_str().unwrap().ends_with("final2.md"));
        assert_eq!(
            fs::read_to_string(f.state.join("resume-args")).unwrap(),
            format!(
                "{}\n{}\n{}\n2\n--\n--model\nmodel-x\n-c\nkey=value with spaces\n",
                f.dir(&id).display(),
                f.repo.display(),
                f.dir(&id).join("input2.md").display()
            )
        );
        fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
        let state = f.call(&["status", &id], 0);
        assert_eq!(state["data"]["observation"]["state"], "finished");
    }
}

#[test]
fn claude_resume_stops_idle_and_wakes_same_identity() {
    let f = Fixture::new();
    let result =
        f.dispatch("claude", &["--profile", "/tmp/resume-profile", "--model", "saved-model", "--", "--verbose"], 0);
    let id = result["data"]["run_id"].as_str().unwrap();
    let native = f.record(id)["launch"]["native"].clone();
    f.agents("idle", "done", "native-session");
    fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
    fs::write(f.dir(id).join("final.md"), "earlier evidence").unwrap();
    let brief = f.root.join("brief.md");
    f.call(&["resume", id, "--brief", brief.to_str().unwrap(), "--request-id", "delta"], 0);
    assert_eq!(f.record(id)["attempt"], 2);
    assert_eq!(f.record(id)["launch"]["native"], native);
    for operation in ["stop-profile", "resume-profile"] {
        assert_eq!(fs::read_to_string(f.state.join(operation)).unwrap(), "/tmp/resume-profile\n");
    }
    assert_eq!(fs::read_to_string(f.state.join("stops")).unwrap(), "abc123\n");
    assert_eq!(
        fs::read_to_string(f.state.join("resume-args")).unwrap(),
        format!(
            "--bg\n--resume\nnative-session\nimplement this task\n\n\nWrite your final report to {}/final2.md.\n\n",
            f.dir(id).display()
        )
    );
    assert_eq!(f.call(&["report", id], 0)["data"]["report"]["content"], "attempt 2\n");
    assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "finished");
    f.call(&["wait", id, "--timeout", "1"], 0);
    f.call(&["verify", id, "--path", "src/"], 0);
    f.call(&["resume", id, "--brief", brief.to_str().unwrap(), "--request-id", "delta"], 0);
    assert_eq!(f.record(id)["attempt"], 2);
    assert_eq!(fs::read_to_string(f.dir(id).join("final.md")).unwrap(), "earlier evidence");
}

#[test]
fn claude_resume_rejects_copy_and_cleans_it_up() {
    for mode in ["both", "id-only", "note-only"] {
        let f = Fixture::new();
        let id = f.launch("claude");
        let native = f.record(&id)["launch"]["native"].clone();
        f.agents("idle", "done", "native-session");
        fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
        fs::write(f.skills.join("resume-copy"), mode).unwrap();
        fs::write(f.dir(&id).join("final.md"), "earlier evidence").unwrap();
        let brief = f.root.join("brief.md");
        let error = f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
        assert!(error["error"].as_str().unwrap().contains("created a copy"));
        let record = f.record(&id);
        assert_eq!(record["attempt"], 1);
        assert_eq!(record["launch"]["state"], "pending_resume");
        assert_eq!(record["launch"]["previous"], native);
        assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "stopped");
        assert_eq!(f.record(&id), record);
        assert!(record["launch"]["error"]["message"].as_str().unwrap().contains("copy"));
        assert_eq!(fs::read_to_string(f.state.join("stops")).unwrap(), "abc123\ndef456\n");
        assert_eq!(fs::read_to_string(f.dir(&id).join("final.md")).unwrap(), "earlier evidence");
    }
}

#[test]
fn claude_resume_refuses_running_session() {
    let f = Fixture::new();
    let id = f.launch("claude");
    let before = f.record(&id);
    let brief = f.root.join("brief.md");
    let error = f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
    assert!(error["error"].as_str().unwrap().contains("cannot resume"));
    assert_eq!(f.record(&id), before);
    assert!(!f.state.join("stop-args").exists());
    assert!(!f.dir(&id).join("input2.md").exists());
}

#[test]
fn claude_resume_retries_pending_error_with_identity_checks() {
    let f = Fixture::new();
    let id = f.launch("claude");
    let native = f.record(&id)["launch"]["native"].clone();
    f.agents("idle", "done", "native-session");
    fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
    fs::write(f.skills.join("resume-copy"), "").unwrap();
    let brief = f.root.join("brief.md");
    f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
    fs::remove_file(f.skills.join("resume-copy")).unwrap();
    let pending = f.record(&id);
    f.agents("idle", "done", "different-session");
    let error = f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
    assert!(error["error"].as_str().unwrap().contains("identity mismatch"));
    assert_eq!(f.record(&id), pending);
    f.agents("busy", "working", "native-session");
    f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
    assert_eq!(f.record(&id), pending);
    f.agents("idle", "done", "native-session");
    f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 0);
    assert_eq!(f.record(&id)["attempt"], 2);
    assert_eq!(f.record(&id)["launch"]["native"], native);
    assert!(!f.dir(&id).join("input3.md").exists());
}

#[test]
fn claude_pending_interrupted_resume_retries_same_attempt() {
    let f = Fixture::new();
    let id = f.launch("claude");
    let mut record = f.record(&id);
    let native = record["launch"]["native"].clone();
    record["launch"] = json!({"state":"pending_resume", "previous":native, "attempt":2, "error":null});
    fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
    let brief = f.root.join("brief.md");
    fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
    f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 1);
    assert_eq!(f.record(&id), record);
    f.agents("idle", "done", "native-session");
    f.call(&["resume", &id, "--brief", brief.to_str().unwrap()], 0);
    assert_eq!(f.record(&id)["attempt"], 2);
    assert_eq!(f.record(&id)["launch"]["native"], native);
    assert!(!f.dir(&id).join("input3.md").exists());
}

#[test]
fn claude_pending_stop_uses_recorded_identity() {
    let f = Fixture::new();
    let id = f.launch("claude");
    let mut record = f.record(&id);
    record["launch"] =
        json!({"state":"pending_resume", "previous":record["launch"]["native"], "attempt":2, "error":null});
    fs::write(f.dir(&id).join("run.json"), record.to_string()).unwrap();
    assert_eq!(f.call(&["stop", &id], 0)["data"]["observation"]["state"], "stopped");
    assert_eq!(fs::read_to_string(f.state.join("stop-args")).unwrap(), "stop\nabc123\n");
    assert_eq!(f.record(&id), record);
}

#[test]
fn claude_resume_help_describes_saved_options() {
    let f = Fixture::new();
    let out = f.cmd().args(["resume", "--help"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().contains(
        "Claude stops a settled live session and resumes the same identity with saved options under the recorded profile."));
}

#[test]
fn claude_resume_waits_for_fresh_evidence() {
    for stale_report in [false, true] {
        let f = Fixture::new();
        let id = f.launch("claude");
        f.agents("idle", "done", "native-session");
        fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
        fs::write(f.skills.join("delayed-wake"), "").unwrap();
        if stale_report {
            fs::write(f.dir(&id).join("final2.md"), "stale").unwrap();
        }
        let started = std::time::Instant::now();
        f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 0);
        assert!(started.elapsed() >= std::time::Duration::from_secs(1), "gate accepted stale evidence");
        assert_eq!(f.record(&id)["attempt"], 2);
    }
}

#[test]
fn claude_resume_refuses_wake_if_stop_leaves_session_live() {
    let f = Fixture::new();
    let id = f.launch("claude");
    f.agents("idle", "done", "native-session");
    fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
    fs::write(f.skills.join("stop-live"), "").unwrap();
    let error = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 1);
    assert!(error["error"].as_str().unwrap().contains("still live after stop"));
    assert!(!f.state.join("resume-args").exists());
    assert_eq!(f.record(&id)["attempt"], 1);
}

#[test]
fn claude_resume_parses_notes_and_attempts_all_copy_cleanup() {
    for output in
        ["note: started a copy as def456!\nbackgrounded · fed123 · worker", "note: started a copy as ?unknown"]
    {
        let f = Fixture::new();
        let id = f.launch("claude");
        f.agents("idle", "done", "native-session");
        fs::write(f.state.join("resume-repo"), f.repo.to_str().unwrap()).unwrap();
        fs::write(f.skills.join("resume-output"), output).unwrap();
        fs::write(f.skills.join("stop-fail"), "").unwrap();
        let error = f.call(&["resume", &id, "--brief", f.root.join("brief.md").to_str().unwrap()], 1);
        assert!(error["error"].as_str().unwrap().contains(output));
        if output.contains("?unknown") {
            assert!(!error["error"].as_str().unwrap().contains("cleanup failures:"));
            assert!(
                error["error"]
                    .as_str()
                    .unwrap()
                    .contains("no copy id could be parsed; the copy must be stopped manually")
            );
        }
        let stops = fs::read_to_string(f.state.join("stops")).unwrap();
        assert_eq!(stops, if output.contains("def456") { "abc123\ndef456\nfed123\n" } else { "abc123\n" });
    }
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
            fs::write(f.repo.join("delta.md"), "delta").unwrap();
            let error = f.call(&["resume", &id, "--brief", "delta.md"], 1);
            if mode == "claude" {
                assert!(error["error"].as_str().unwrap().contains("identity mismatch"));
            }
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
fn wait_outlasts_a_transient_unknown_and_reports_a_lasting_one() {
    let f = Fixture::new();
    let id = f.launch("codex");
    fs::write(f.skills.join("query-fail"), "").unwrap();
    let waiter =
        f.cmd().args(["--json", "wait", &id, "--timeout", "30"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    f.status("exited", "COMPLETED");
    fs::remove_file(f.skills.join("query-fail")).unwrap();
    let out = waiter.wait_with_output().unwrap();
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{result}");
    assert_eq!(result["data"]["runs"][0]["state"], "finished");

    fs::write(f.skills.join("query-fail"), "").unwrap();
    let lasting = f.call(&["wait", &id, "--timeout", "1"], 1);
    assert!(lasting["data"]["message"].as_str().unwrap().contains("unknown state at timeout"), "{lasting}");
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
        f.status("exited", "COMPLETED");
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
    f.status("exited", "COMPLETED");
    f.call(&["verify", &id], 2);
    f.call(&["verify", &id, "--path", "tests/"], 0);
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
        assert!(data["data"].get("pane_loop").is_none());
        assert!(data["data"]["rex"].get("detail").is_none());
    }
    let out = f.cmd().args(["doctor", "--executor", "claude"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("ok claude: claude\n"));
    assert!(text.contains("claude profile: none\nok rex\n"));
    assert!(!text.contains("fix:"));
    f.executable(&f.bin.join("rex"), "echo private-identity; exit 1");
    fs::remove_file(f.bin.join("watch")).unwrap();
    fs::remove_file(f.bin.join("vibe")).unwrap();
    let doctor = f.call(&["doctor", "--executor", "vibe"], 1);
    assert!(doctor.to_string().contains("install vibe and put its executable on PATH"));
    assert!(doctor["data"]["rex"]["fix"].is_string());
    assert!(doctor["data"].get("pane_loop").is_none());
    assert!(!doctor.to_string().contains("private-identity"));
    let out = f.cmd().args(["doctor", "--executor", "vibe"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("missing vibe: vibe; fix: install vibe and put its executable on PATH"));
    assert!(text.contains("missing rex; fix:"));
    assert!(!text.contains("watch"));
}

#[test]
fn doctor_preserves_paths_in_json_and_escapes_them_in_human_output() {
    use std::os::unix::ffi::OsStringExt;

    let f = Fixture::new();
    let skills = f.root.join("skills\nwith\\controls");
    std::os::unix::fs::symlink(&f.skills, &skills).unwrap();
    let profile = f.root.join("profile\nwith\\controls");
    let mut cmd = f.cmd();
    let out = cmd
        .env("CREW_SKILLS_DIR", &skills)
        .arg("doctor")
        .args(["--executor", "claude", "--profile"])
        .arg(&profile)
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    let data: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["data"]["claude_profile"], json!(profile));
    assert_eq!(
        data["data"]["executors"][0]["checks"][1]["item"],
        json!(skills.join("implementer-core/scripts/check_brief.sh"))
    );
    let out = f
        .cmd()
        .env("CREW_SKILLS_DIR", &skills)
        .arg("doctor")
        .args(["--executor", "claude", "--profile"])
        .arg(&profile)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(&format!(
        "ok claude: {}\n",
        skills.join("implementer-core/scripts/check_brief.sh").to_str().unwrap().escape_debug()
    )));
    assert!(text.contains(&format!("claude profile: {}\n", profile.to_str().unwrap().escape_debug())));

    let invalid_profile = f.root.join(std::ffi::OsString::from_vec(vec![b'p', 0xff]));
    let out = f
        .cmd()
        .arg("doctor")
        .args(["--executor", "vibe", "--profile"])
        .arg(&invalid_profile)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(101));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("path contains invalid UTF-8 characters"));
}

#[test]
fn every_help_has_example_and_usage_errors_are_json() {
    let f = Fixture::new();
    for command in [
        vec![],
        vec!["doctor"],
        vec!["dispatch"],
        vec!["handoff"],
        vec!["ls"],
        vec!["status"],
        vec!["wait"],
        vec!["report"],
        vec!["stop"],
        vec!["resume"],
        vec!["verify"],
    ] {
        let out = f.cmd().args(command).arg("--help").output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("Example:"));
    }
    assert!(f.call(&["dispatch", "--executor", "bad"], 2)["error"].as_str().unwrap().contains("error:"));
    f.call(&["status", "../escape"], 1);
}

#[test]
fn repo_subdirectory_is_refused_with_its_root() {
    let f = Fixture::new();
    fs::create_dir_all(f.repo.join("sub")).unwrap();
    let brief = f.root.join("brief.md");
    let args = ["dispatch", "--executor", "codex", "--brief", brief.to_str().unwrap(), "--repo", "sub"];
    let error = f.call(&args, 1);
    let message = error["error"].as_str().unwrap();
    assert!(message.contains("repository root") && message.contains(f.repo.to_str().unwrap()), "{message}");
    assert!(!f.state.join("runs").exists());
}

#[test]
fn non_claude_profile_is_a_usage_error() {
    let f = Fixture::new();
    let result = f.dispatch("vibe", &["--profile", "/tmp/profile"], 2);
    assert_eq!(result["data"]["message"], "--profile applies only to Claude");
    assert!(!f.state.join("runs").exists());
}

#[test]
fn review_dispatch_launches_one_named_run_per_model() {
    let f = Fixture::new();
    let args = ["--mode", "review", "--model", "model-a", "--model", "model-b", "--name", "rev", "--goal", "g"];
    let runs = f.dispatch("vibe", &args, 0)["data"]["runs"].as_array().unwrap().clone();
    let recorded: Vec<_> = runs
        .iter()
        .map(|launch| {
            let record = f.record(launch["run_id"].as_str().unwrap());
            (record["name"].clone(), record["model"].clone(), record["goal"].clone())
        })
        .collect();
    assert_eq!(
        recorded,
        [(json!("rev-model-a"), json!("model-a"), json!("g")), (json!("rev-model-b"), json!("model-b"), json!("g"))]
    );
    assert_eq!(f.call(&["ls", "--goal", "g"], 0)["data"]["runs"].as_array().unwrap().len(), 2);
}

#[test]
fn a_failed_later_launch_still_reports_the_runs_already_started() {
    let f = Fixture::new();
    // The first launch deletes the dispatched brief, so copying it for the second run fails.
    fs::write(f.skills.join("drop-brief"), "").unwrap();
    let args = ["--mode", "review", "--model", "model-a", "--model", "model-b", "--name", "rev"];
    let data = f.dispatch("vibe", &args, 1)["data"].clone();
    let runs = data["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{data}");
    assert_eq!(f.record(runs[0]["run_id"].as_str().unwrap())["name"], "rev-model-a");
    assert!(data["error"].as_str().unwrap().starts_with("model-b: "), "{data}");
}

#[test]
fn several_models_are_refused_for_writers_handoffs_and_repeats() {
    let f = Fixture::new();
    let brief = f.root.join("brief.md");
    for (command, mode, second, refusal) in [
        ("dispatch", "direct", "model-b", "a writer runs one model"),
        ("handoff", "review", "model-b", "a handoff continues one conversation"),
        ("dispatch", "review", "model-a", "must name a different model"),
    ] {
        let args = [command, "--executor", "vibe", "--brief", brief.to_str().unwrap(), "--repo", ".", "--mode", mode];
        let result = f.call(&[&args[..], &["--model", "model-a", "--model", second]].concat(), 2);
        assert!(result["data"]["message"].as_str().unwrap().contains(refusal), "{result}");
    }
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
            (vec!["ls"], vec![format!("{id}\t-\t-\t{executor}\trunning\t1\t-")]),
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
            f.status("exited", "COMPLETED");
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
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{id}\tworker\\nname\tpolish\tcodex\trunning\t1\t-\ncodex: 1 runs (running 1)\n")
    );
    f.status("exited", "COMPLETED");
    let check = "echo first; echo second; exit 1";
    for (args, code, expected) in [
        (vec!["verify", &id], 2, "verify requires --path when none were recorded at dispatch\n".to_owned()),
        (vec!["verify", &id, "--path", "src/"], 0, "pass attempt 1\n".into()),
        (vec!["verify", &id, "--path", "src/", "--check", check], 1, format!("fail attempt 1\n{check}: first\n")),
    ] {
        let out = f.cmd().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(code));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
    }
    f.status("alive", "RUNNING");
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

#[test]
fn request_id_replays_return_the_recorded_run_without_relaunching() {
    for executor in ["vibe", "codex"] {
        let f = Fixture::new();
        let id = f.launch(executor);
        f.status("exited", "COMPLETED");
        let suffix = if executor == "vibe" { "vibe" } else { "classic" };
        let original =
            fs::read_to_string(f.skills.join(format!("{executor}-implementer/scripts/resume_{suffix}.sh"))).unwrap();
        f.script(
            &format!("{executor}-implementer"),
            &format!("resume_{suffix}_impl"),
            &original.replace("43211", "RESUME_PID"),
        );
        f.script(
            &format!("{executor}-implementer"),
            &format!("resume_{suffix}"),
            &format!(
                "echo call >> \"$CREW_STATE_DIR/resume-count\"\npid=$((43210 + $(wc -l < \"$CREW_STATE_DIR/resume-count\")))\nsed \"s/RESUME_PID/$pid/g\" \"$(dirname \"$0\")/resume_{suffix}_impl.sh\" | sh -s -- \"$@\""
            ),
        );
        f.write("delta.md", "sync payload one");
        let brief = f.root.join("delta.md");
        let brief = brief.to_str().unwrap();
        let args = ["resume", id.as_str(), "--brief", brief, "--request-id", "sync-1"];
        let count = |f: &Fixture| fs::read_to_string(f.state.join("resume-count")).unwrap();
        let first = f.call(&args, 0);
        assert_eq!(first["data"]["run"]["attempt"], 2);
        assert_eq!(first["data"]["run"]["request_attempts"], json!({"sync-1": 2}));
        assert_eq!(count(&f), "call\n");
        // Replay while the resumed attempt is running.
        let replay = f.call(&args, 0);
        assert_eq!(replay["data"]["run"]["attempt"], 2);
        assert_eq!(replay["data"]["observation"]["state"], "running");
        assert_eq!(count(&f), "call\n");
        // Replay after the attempt settles: still one executor invocation.
        fs::write(f.skills.join("status"), "state=exited pid=43211\nverdict: COMPLETED\n").unwrap();
        assert_eq!(f.call(&["status", &id], 0)["data"]["observation"]["state"], "finished");
        let settled = f.call(&args, 0);
        assert_eq!(settled["data"]["run"]["attempt"], 2);
        assert_eq!(settled["data"]["observation"]["state"], "finished");
        assert_eq!(count(&f), "call\n");
        // The same id with different brief bytes is rejected.
        f.write("delta.md", "sync payload two");
        let rejected = f.call(&args, 1);
        assert!(rejected["error"].as_str().unwrap().contains("sync-1"), "{rejected}");
        assert_eq!(count(&f), "call\n");
        // A different request id reserves a later attempt.
        let second = f.call(&["resume", &id, "--brief", brief, "--request-id", "sync-2"], 0);
        assert_eq!(second["data"]["run"]["attempt"], 3);
        assert_eq!(second["data"]["run"]["request_attempts"], json!({"sync-1": 2, "sync-2": 3}));
        assert_eq!(count(&f), "call\ncall\n");
    }
}

#[test]
fn request_id_reservation_is_saved_before_launch_and_protects_its_input() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    f.script("codex-implementer", "resume_classic", "echo call >> \"$CREW_STATE_DIR/resume-count\"\nexit 1");
    f.write("delta.md", "reserved payload");
    let brief = f.root.join("delta.md");
    let brief = brief.to_str().unwrap();
    let count = || fs::read_to_string(f.state.join("resume-count")).unwrap();
    f.call(&["resume", &id, "--brief", brief, "--request-id", "sync-9"], 1);
    let record = f.record(&id);
    assert_eq!(record["launch"]["state"], "pending_resume");
    assert_eq!(record["request_attempts"], json!({"sync-9": 2}));
    assert_eq!(fs::read_to_string(f.dir(&id).join("input2.md")).unwrap(), "reserved payload");
    assert_eq!(count(), "call\n");
    // A replayed key after the failed launch stays explicit and never relaunches.
    let replay = f.call(&["resume", &id, "--brief", brief, "--request-id", "sync-9"], 0);
    assert_eq!(replay["data"]["run"]["launch"]["state"], "pending_resume");
    assert_eq!(replay["data"]["run"]["request_attempts"], json!({"sync-9": 2}));
    assert_eq!(count(), "call\n");
    // A no-id resume cannot rewrite the reserved input with different content.
    f.write("delta.md", "different content");
    let denied = f.call(&["resume", &id, "--brief", brief], 1);
    assert!(denied["error"].as_str().unwrap().contains("reserved"), "{denied}");
    assert_eq!(fs::read_to_string(f.dir(&id).join("input2.md")).unwrap(), "reserved payload");
    assert_eq!(count(), "call\n");
    // A new id cannot steal the pending attempt reserved by another id.
    let steal = f.call(&["resume", &id, "--brief", brief, "--request-id", "sync-10"], 1);
    assert!(steal["error"].as_str().unwrap().contains("reserved"), "{steal}");
    assert!(steal["error"].as_str().unwrap().contains("retry without --request-id"));
    assert_eq!(f.record(&id)["request_attempts"], json!({"sync-9": 2}));
    assert_eq!(count(), "call\n");
}

#[test]
fn request_id_is_bounded_and_control_free() {
    let f = Fixture::new();
    let id = f.launch("codex");
    f.status("exited", "COMPLETED");
    let brief = f.root.join("brief.md");
    let before = f.record(&id);
    let long = "x".repeat(201);
    for bad in ["", long.as_str(), "a\tb"] {
        let result = f.call(&["resume", &id, "--brief", brief.to_str().unwrap(), "--request-id", bad], 1);
        assert!(result["error"].as_str().unwrap().contains("request id"), "{result}");
    }
    assert_eq!(f.record(&id), before);
}
