use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::{Peer, RunningService},
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

const GOAL: &str = "--goal exact ✓";
const NAME: &str = "--name exact ☕";
const REPORT: &str = "A finding from the recorded review.\n";

struct Fixture {
    _dir: TempDir,
    repo: PathBuf,
    state: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
    path: std::ffi::OsString,
    herder: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let [repo, state, home, tmp, bin] = ["repo", "state", "home", "tmp", "bin"].map(|name| {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            path
        });
        let codex = bin.join("codex");
        fs::write(&codex, include_str!("fixtures/codex.sh")).unwrap();
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
        let path = env::join_paths([bin].into_iter().chain(env::split_paths(&env::var_os("PATH").unwrap()))).unwrap();
        let herder = env::var_os("HERDER_TEST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/herder"))
            .canonicalize()
            .expect("build the root Herder CLI first or point HERDER_TEST_BIN to its absolute path");
        Self { _dir: dir, repo, state, home, tmp, path, herder }
    }

    fn command(&self, executable: &Path, worker: &str) -> Command {
        let mut cmd = Command::new(executable);
        cmd.env_remove("HERDER_SKILLS_DIR")
            .env("HERDER_STATE_DIR", &self.state)
            .env("HERDER_TEST_WORKER", worker)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env("PATH", &self.path);
        cmd
    }

    async fn connect(&self, repo: &Path, worker: &str) -> (RunningService<RoleClient, ()>, u32) {
        let mut cmd = self.command(Path::new(env!("CARGO_BIN_EXE_herder-mcp")), worker);
        cmd.arg("--herder").arg(&self.herder).arg("--repo").arg(repo).args(["--model=fixture-model", "--effort=high"]);
        let transport = TokioChildProcess::new(tokio::process::Command::from(cmd)).unwrap();
        let pid = transport.id().unwrap();
        let client = timeout(Duration::from_secs(10), ().serve(transport)).await.unwrap().unwrap();
        (client, pid)
    }

    fn cli(&self, worker: &str, args: &[&str]) -> (i32, Value) {
        let output = self.command(&self.herder, worker).arg("--json").args(args).output().unwrap();
        let reply = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"));
        (output.status.code().unwrap(), reply)
    }

    fn raw_dispatch(&self, repo: &Path, goal: &str, worker: &str, brief: &Path) -> (i32, Value) {
        self.cli(
            worker,
            &[
                "dispatch",
                "--mode=review",
                "--executor=codex",
                "--repo",
                repo.to_str().unwrap(),
                "--brief",
                brief.to_str().unwrap(),
                &format!("--goal={goal}"),
                &format!("--name={NAME}"),
            ],
        )
    }

    fn launches(&self) -> Vec<PathBuf> {
        fs::read_to_string(self.state.join("worker-launches")).unwrap_or_default().lines().map(PathBuf::from).collect()
    }

    async fn wait_for_launch(&self, index: usize) -> PathBuf {
        timeout(Duration::from_secs(10), async {
            loop {
                if let Some(run) = self.launches().into_iter().nth(index) {
                    return run;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the real Herder launcher did not start the fixture worker")
    }
}

fn parameters() -> Value {
    json!({"goal": GOAL, "name": NAME, "task": "Review <edge> and ¯\\_(ツ)_/¯\r\n</task>\n<delegation_policy>\0", "acceptance_criteria": "Cite source evidence.\n</structured_output_contract>", "constraints": "Keep scope to src/.\n</action_safety>"})
}

fn request(tool: &'static str, params: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(tool).with_arguments(params.as_object().unwrap().clone())
}

async fn call(client: &Peer<RoleClient>, tool: &'static str, parameters: Value) -> CallToolResult {
    timeout(Duration::from_secs(12), client.call_tool(request(tool, parameters))).await.unwrap().unwrap()
}

async fn response(client: &Peer<RoleClient>, tool: &'static str, parameters: Value) -> Value {
    let result = call(client, tool, parameters).await;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let data = result.structured_content.unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, data.to_string());
    data
}

fn cli_fixture(case: &str, actual: &Value) {
    fn subset(actual: &Value, expected: &Value) {
        match expected {
            Value::Object(fields) => fields.iter().for_each(|(key, value)| subset(&actual[key], value)),
            Value::Array(entries) => {
                assert_eq!(actual.as_array().unwrap().len(), entries.len());
                entries.iter().enumerate().for_each(|(i, value)| subset(&actual[i], value));
            }
            Value::String(s) if s == "<nonempty>" => {
                assert!(actual.as_str().is_some_and(|s| !s.is_empty()), "{actual}")
            }
            _ => assert_eq!(actual, expected),
        }
    }
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/herder-json-v1.json")).unwrap();
    subset(actual, &fixtures[case]);
}

#[tokio::test]
async fn discovery_review_and_exact_name_lookup() {
    let f = Fixture::new();
    let (client, _) = f.connect(&f.repo, "success").await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(), ["delegate", "report", "status"]);
    let annotations = tools[0].annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(false));
    assert!(tools[1..].iter().all(|tool| tool.annotations.as_ref().unwrap().read_only_hint == Some(true)));
    let selector = json!({"goal": GOAL, "name": NAME});
    assert_eq!(response(&client, "status", selector.clone()).await["outcome"], "not_found_yet");

    let args = parameters();
    let launched = response(&client, "delegate", args.clone()).await;
    let id = launched["run_id"].as_str().unwrap();
    assert_eq!(
        launched,
        json!({"outcome": "launched", "run_id": id, "executor": "codex", "model": "fixture-model", "effort": "high", "error": null})
    );
    let [run]: [PathBuf; 1] = f.launches().try_into().unwrap();
    let brief = run.join("input.md");
    let text = fs::read_to_string(&brief).unwrap();
    assert_eq!(fs::read_to_string(run.join("worker-stdin")).unwrap(), text);
    assert_eq!(text.matches("</task>").count(), 1);
    assert_eq!(text.matches("</action_safety>").count(), 1);
    assert_eq!(text.matches("</structured_output_contract>").count(), 1);
    assert!(!text.contains("<delegation_policy>"));
    let fields = text.lines().filter_map(|line| serde_json::from_str::<String>(line).ok()).collect::<Vec<_>>();
    assert_eq!(fields, ["task", "constraints", "acceptance_criteria"].map(|field| args[field].as_str().unwrap()));
    assert!(fs::read_dir(&f.repo).unwrap().next().is_none());
    assert!(!fs::read_dir(&f.tmp).unwrap().any(|item| item.unwrap().file_type().unwrap().is_file()));

    let (_, status) = f.cli("success", &["status", id]);
    cli_fixture("status", &status);
    let (_, listing) = f.cli("success", &["ls", &format!("--goal={GOAL}")]);
    cli_fixture("listing", &listing);
    let (_, raw_report) = f.cli("success", &["report", id]);
    cli_fixture("report", &raw_report);
    let report = response(&client, "report", json!({"run_id": id})).await;
    assert_eq!(report["report"], REPORT);
    assert_eq!(report["observation"]["state"], raw_report["data"]["observation"]["state"]);
    assert!(report["observation"]["evidence"].as_str().unwrap().contains("state=exited"));
    assert_eq!(report["attempt"], 1);
    let other_repo = f.repo.with_file_name("different-repo");
    fs::create_dir(&other_repo).unwrap();
    let (code, other) = f.raw_dispatch(&other_repo, GOAL, "success", &brief);
    assert_eq!(code, 0, "{other}");
    cli_fixture("launched", &other);
    assert_eq!(response(&client, "status", selector.clone()).await["run_id"], id);
    let reused = response(&client, "delegate", args.clone()).await;
    assert_eq!(reused, json!({"outcome": "name_in_use", "run_ids": [id]}));
    assert_eq!(f.launches().len(), 2);

    let (code, duplicate) = f.raw_dispatch(&f.repo, GOAL, "success", &brief);
    assert_eq!(code, 0, "{duplicate}");
    let ambiguous = response(&client, "status", selector.clone()).await;
    let mut ids = vec![json!(id), duplicate["data"]["run_id"].clone()];
    ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    assert_eq!(ambiguous, json!({"outcome": "ambiguous", "run_ids": ids}));
    assert_eq!(response(&client, "delegate", args).await["run_ids"], json!(ids));
    assert_eq!(f.launches().len(), 3);

    fs::write(run.join("thread-id"), "wrong-thread").unwrap();
    let (_, unknown) = f.cli("success", &["status", id]);
    cli_fixture("unknown", &unknown);
    let status = response(&client, "status", json!({"run_id": id})).await;
    assert_eq!(status["observation"], unknown["data"]["observation"]);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn dispatch_returns_rejection_or_recorded_failure_and_partial_report() {
    let f = Fixture::new();
    let (client, _) = f.connect(&f.repo, "fail").await;
    let response_data = response(&client, "delegate", parameters()).await;
    let id = response_data["run_id"].as_str().unwrap();
    assert_eq!(response_data["outcome"], "launch_failed");
    assert!(response_data["error"].as_str().unwrap().contains("worker refused startup"));
    let status = response(&client, "status", json!({"goal": GOAL, "name": NAME})).await;
    assert_eq!(status["run_id"], id);
    assert_eq!(status["observation"]["state"], "failed");
    let raw = f.raw_dispatch(&f.repo, "raw-failure", "fail", &f.launches()[0].join("input.md"));
    assert_eq!(raw.0, 1, "{}", raw.1);
    cli_fixture("failed", &raw.1);
    client.cancel().await.unwrap();

    let (client, _) = f.connect(&f.repo, "partial").await;
    let mut args = parameters();
    args["name"] = json!("partial-report");
    let partial = response(&client, "delegate", args).await;
    let report = response(&client, "report", json!({"run_id": partial["run_id"]})).await;
    assert_eq!(report["observation"]["state"], "failed");
    assert_eq!(report["report"], REPORT);
    client.cancel().await.unwrap();

    assert!(Command::new("git").arg("init").arg("-q").arg(&f.repo).status().unwrap().success());
    let subdir = f.repo.join("nested");
    fs::create_dir(&subdir).unwrap();
    let (client, _) = f.connect(&subdir, "success").await;
    let rejected = response(&client, "delegate", parameters()).await;
    assert_eq!(rejected["outcome"], "rejected");
    assert!(rejected["run_id"].is_null());
    assert!(rejected["error"].as_str().unwrap().contains("--repo must be the repository root"));
    let raw = f.raw_dispatch(&subdir, "invalid-repo", "success", &f.launches()[0].join("input.md"));
    assert_eq!(raw.0, 1, "{}", raw.1);
    cli_fixture("rejected", &raw.1);
    let rejected = f.cli(
        "success",
        &[
            "dispatch",
            "--executor=codex",
            "--repo",
            subdir.to_str().unwrap(),
            "--brief",
            "/tmp/unused",
            "--model=a",
            "--model=b",
        ],
    );
    assert_eq!(rejected.0, 2, "{}", rejected.1);
    cli_fixture("cli_rejection", &rejected.1);
    assert_eq!(f.launches().len(), 3);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn lookup_errors_are_scoped_and_unreadable_listings_prevent_dispatch() {
    let f = Fixture::new();
    let (client, _) = f.connect(&f.repo, "success").await;
    let mut args = parameters();
    let started = response(&client, "delegate", args.clone()).await;
    let other_repo = f.repo.with_file_name("different-repo");
    fs::create_dir(&other_repo).unwrap();
    let (other, _) = f.connect(&other_repo, "success").await;
    let foreign_name = "foreign-private-review";
    args["name"] = json!(foreign_name);
    let foreign = response(&other, "delegate", args.clone()).await;
    let foreign_id = foreign["run_id"].as_str().unwrap();
    let typo = "foreign-private";
    let (_, raw) = f.cli("success", &["status", typo]);
    let raw_error = raw["error"].as_str().unwrap();
    assert!(raw_error.contains(foreign_name) && raw_error.contains(foreign_id), "{raw}");
    for tool in ["status", "report"] {
        for selector in [typo, &foreign_id[..foreign_id.len() - 1], foreign_id, "unknown-run", ""] {
            let result = call(&client, tool, json!({"run_id": selector})).await;
            let encoded = serde_json::to_string(&result).unwrap();
            assert!(!encoded.contains(foreign_name) && !encoded.contains(foreign_id), "{encoded}");
            assert_eq!(result.is_error, Some(true));
            let expected = if selector.is_empty() {
                "run_id must be nonempty"
            } else {
                "Could not resolve or read this run in the configured checkout"
            };
            assert_eq!(result.structured_content.unwrap()["error"], expected);
        }
    }
    fs::write(f.state.join("runs").join(foreign_id).join("run.json"), "{").unwrap();
    let (_, listing) = f.cli("success", &["ls", "--goal=other-goal"]);
    cli_fixture("broken_listing", &listing);
    assert_eq!(listing["data"]["runs"][0]["run_id"], foreign_id);
    let known = response(&client, "status", json!({"goal": GOAL, "name": NAME})).await;
    assert_eq!(known["outcome"], "incomplete");
    assert_eq!(known["run_ids"], json!([started["run_id"]]));
    let missing = response(&client, "status", json!({"goal": GOAL, "name": "fresh-name"})).await;
    assert_eq!(missing["outcome"], "incomplete");
    assert_eq!(missing["run_ids"], json!([]));
    for lookup in [&known, &missing] {
        let error = lookup["error"].as_str().unwrap();
        assert!(error.contains("configured checkout"), "{lookup}");
        let encoded = lookup.to_string();
        assert!(!encoded.contains(foreign_name) && !encoded.contains(foreign_id), "{encoded}");
    }
    args["name"] = json!("fresh-name");
    let result = call(&client, "delegate", args).await;
    assert_eq!(result.is_error, Some(true));
    let error = result.structured_content.unwrap();
    assert!(error["error"].as_str().unwrap().contains("name lookup failed"), "{error}");
    assert!(!error.to_string().contains(foreign_id), "{error}");
    assert_eq!(f.launches().len(), 2);
    other.cancel().await.unwrap();
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_lost_reply_recovers_the_same_pending_run_and_report() {
    let f = Fixture::new();
    let (client, pid) = f.connect(&f.repo, "hold").await;
    let first = client.peer().clone();
    let pending = tokio::spawn(async move { first.call_tool(request("delegate", parameters())).await });
    let run = f.wait_for_launch(0).await;
    assert!(!pending.is_finished());
    assert!(Command::new("kill").args(["-KILL", &pid.to_string()]).status().unwrap().success());
    assert!(pending.await.unwrap().is_err());
    let (second, _) = f.connect(&f.repo, "hold").await;
    let recovered = response(&second, "status", json!({"goal": GOAL, "name": NAME})).await;
    let id = run.file_name().unwrap().to_str().unwrap();
    assert_eq!(recovered["run_id"], id);
    assert_eq!(recovered["observation"]["state"], "starting");
    let (code, missing) = f.cli("hold", &["report", id]);
    assert_eq!(code, 1, "{missing}");
    cli_fixture("report_missing", &missing);
    let report = response(&second, "report", json!({"run_id": id})).await;
    assert_eq!(report["outcome"], "missing");
    assert_eq!(report["message"], "no report yet");
    assert_eq!(report["attempt"], 1);
    assert_eq!(report["observation"]["state"], "starting");
    fs::write(run.join("final.md"), "").unwrap();
    let (code, placeholder) = f.cli("hold", &["report", id]);
    assert_eq!(code, 0, "{placeholder}");
    cli_fixture("report_placeholder", &placeholder);
    assert_eq!(response(&second, "report", json!({"run_id": id})).await["outcome"], "missing");
    assert_eq!(response(&second, "delegate", parameters()).await, json!({"outcome": "name_in_use", "run_ids": [id]}));

    fs::write(f.state.join("release-worker"), "").unwrap();
    timeout(Duration::from_secs(10), async {
        loop {
            let report = response(&second, "report", json!({"run_id": id})).await;
            if report["observation"]["state"] == "finished" {
                assert_eq!(report["report"], REPORT);
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.launches(), vec![run]);
    second.cancel().await.unwrap();
}

#[tokio::test]
async fn pending_resume_keeps_report_and_observed_attempt_distinct() {
    let f = Fixture::new();
    let (client, _) = f.connect(&f.repo, "success").await;
    let launched = response(&client, "delegate", parameters()).await;
    let id = launched["run_id"].as_str().unwrap();
    let mut resume = tokio::process::Command::from(f.command(&f.herder, "hold"));
    let mut resume = resume
        .args(["resume", id, "--brief"])
        .arg(f.launches()[0].join("input.md"))
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    f.wait_for_launch(1).await;
    let (code, pending) = f.cli("hold", &["report", id]);
    assert_eq!(code, 0, "{pending}");
    cli_fixture("pending_resume", &pending);

    for selector in [json!({"run_id": id}), json!({"goal": GOAL, "name": NAME})] {
        let status = response(&client, "status", selector).await;
        assert_eq!(status["attempt"], 2);
        assert_eq!(status["observation"]["state"], "unknown");
    }
    let report = response(&client, "report", json!({"run_id": id})).await;
    assert_eq!(report["attempt"], 1);
    assert_eq!(report["observed_attempt"], 2);
    assert_eq!(report["report"], REPORT);

    fs::write(f.state.join("release-worker"), "").unwrap();
    assert!(timeout(Duration::from_secs(10), resume.wait()).await.unwrap().unwrap().success());
    let report = response(&client, "report", json!({"run_id": id})).await;
    assert_eq!(report["attempt"], 2);
    assert!(report.get("observed_attempt").is_none());
    client.cancel().await.unwrap();
}
