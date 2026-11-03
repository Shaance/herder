use super::*;

fn doc(f: &Fixture, body: &str) -> PathBuf {
    f.write("HANDOFF.md", body);
    f.root.join("HANDOFF.md")
}
fn handoff(f: &Fixture, executor: &str, brief: &Path, extra: &[&str]) -> Value {
    let mut args = vec!["handoff", "--executor", executor, "--brief", brief.to_str().unwrap(), "--repo", "."];
    args.extend(extra);
    f.call(&args, 0)
}
const CONTEXT: &str =
    "# Continuation\n\nRemaining work: finish the handoff tests. Decision: keep dispatch strict. Marker ZQX-917.\n";

#[test]
fn handoff_launches_a_plain_continuation_and_reuses_dispatch_options() {
    let f = Fixture::new();
    let brief = doc(&f, CONTEXT);
    let result = handoff(
        &f,
        "vibe",
        &brief,
        &["--model", "model-x", "--goal", "g1", "--name", "cont", "--path", "src/", "--pane", "--", "-c", "k=v"],
    );
    let id = result["data"]["run_id"].as_str().unwrap();
    let record = f.record(id);
    assert_eq!(record["goal"], "g1");
    assert_eq!(record["name"], "cont");
    assert_eq!(record["model"], "model-x");
    assert_eq!(record["extra"], json!(["-c", "k=v"]));
    assert_eq!(record["authorized_paths"], json!(["src/"]));
    assert_eq!(record["brief"], json!(f.dir(id).join("input.md")));
    // The stored input carries the exact source context plus the real continuation contract.
    let input = fs::read_to_string(f.dir(id).join("input.md")).unwrap();
    assert!(input.contains(CONTEXT), "{input}");
    for block in [
        "<task>",
        "<workstream_contract>",
        "<implementation_authority>",
        "<action_safety>",
        "<default_follow_through_policy>",
        "<verification_loop>",
        "<economy_contract>",
        "<structured_output_contract>",
    ] {
        assert!(input.contains(block), "{block}");
    }
    // The receiver gets the stored input: the stub launch script copies the brief it was given.
    assert_eq!(fs::read(f.dir(id).join("brief.md")).unwrap(), input.as_bytes());
    let args = fs::read_to_string(f.state.join("launch-args")).unwrap();
    assert!(args.contains("input.md"));
    assert!(args.contains("--model\nmodel-x"));
    assert!(fs::read_to_string(f.state.join("rex-args")).unwrap().contains("wait"));
    // The document itself is never printed to stdout.
    let out = f
        .cmd()
        .args(["handoff", "--executor", "codex", "--brief", brief.to_str().unwrap(), "--repo", "."])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!text.contains("ZQX-917"), "{text}");
}

#[test]
fn handoff_snapshot_survives_later_edits_and_deletion_of_the_source() {
    let f = Fixture::new();
    let brief = doc(&f, "original context line ALPHA-331\n");
    let result = handoff(&f, "claude", &brief, &[]);
    let id = result["data"]["run_id"].as_str().unwrap();
    let stored = fs::read(f.dir(id).join("input.md")).unwrap();
    assert!(String::from_utf8_lossy(&stored).contains("original context line ALPHA-331"));
    // The Claude stub receives the same snapshot as its prompt.
    assert!(fs::read_to_string(f.state.join("claude-args")).unwrap().contains("original context line ALPHA-331"));
    fs::write(&brief, "rewritten after launch\n").unwrap();
    assert_eq!(fs::read(f.dir(id).join("input.md")).unwrap(), stored);
    fs::remove_file(&brief).unwrap();
    assert_eq!(fs::read(f.dir(id).join("input.md")).unwrap(), stored);
}

#[test]
fn handoff_rejects_invalid_documents_before_any_launch() {
    for (name, bytes) in [
        ("missing", None),
        ("empty", Some(&b""[..])),
        ("whitespace", Some(&b"  \n\t "[..])),
        ("non-utf8", Some(&[0xffu8, 0xfe, b'A'][..])),
    ] {
        let f = Fixture::new();
        let brief = match bytes {
            Some(bytes) => {
                let path = f.root.join("HANDOFF.md");
                fs::write(&path, bytes).unwrap();
                path
            }
            None => f.root.join("absent.md"),
        };
        let out = f
            .cmd()
            .arg("--json")
            .args(["handoff", "--executor", "vibe", "--brief", brief.to_str().unwrap(), "--repo", "."])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{name}");
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["ok"], false, "{name}");
        assert!(result["data"]["message"].as_str().unwrap().len() > 3, "{name}: {result}");
        assert!(!f.state.join("runs").exists(), "{name}");
        assert!(!f.state.join("launch-args").exists(), "{name}");
    }
}

#[test]
fn handoff_lint_failure_leaves_no_staging_or_published_run() {
    let f = Fixture::new();
    fs::write(f.skills.join("lint-fail"), "").unwrap();
    let brief = doc(&f, "plain context without contract blocks\n");
    let result = f.call(&["handoff", "--executor", "vibe", "--brief", brief.to_str().unwrap(), "--repo", "."], 1);
    assert!(result["error"].as_str().unwrap().contains("invalid brief"), "{result}");
    let empty = f.state.join("runs").read_dir().map(|mut d| d.next().is_none()).unwrap_or(true);
    assert!(empty, "orphan staging left behind");
    assert!(!f.state.join("launch-args").exists());
}

#[test]
fn handoff_review_mode_instructs_read_only_and_profile_rule_holds() {
    let f = Fixture::new();
    let brief = doc(&f, "review the handoff diff\n");
    let result = f.call(
        &["handoff", "--executor", "codex", "--mode", "review", "--brief", brief.to_str().unwrap(), "--repo", "."],
        0,
    );
    let id = result["data"]["run_id"].as_str().unwrap();
    let input = fs::read_to_string(f.dir(id).join("input.md")).unwrap();
    assert!(input.contains("read-only"), "{input}");
    assert!(input.contains("review the handoff diff"));
    assert_eq!(f.call(&["status", id], 0)["data"]["observation"]["state"], "running");
    let misuse = f.call(
        &["handoff", "--executor", "vibe", "--profile", "/tmp/p", "--brief", brief.to_str().unwrap(), "--repo", "."],
        2,
    );
    assert_eq!(misuse["data"]["message"], "--profile applies only to Claude");
    assert_eq!(f.call(&["ls"], 0)["data"]["runs"].as_array().unwrap().len(), 1);
}
