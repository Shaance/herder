use crate::{Action, IssueAction, domain::Run};
use anyhow::Result;
use serde_json::{Value, json};

pub fn emit(json_output: bool, result: Result<(Value, i32)>, failure_code: i32, human: fn(&Value)) -> ! {
    let (data, error, code) = match result {
        Ok((data, code)) => (data, None, code),
        Err(e) => (Value::Null, Some(format!("{e:#}")), failure_code),
    };
    if json_output {
        println!("{}", json!({"ok":code == 0,"data":data,"error":error}));
    } else if let Some(error) = error {
        eprintln!("{error}");
    } else {
        human(&data);
    }
    std::process::exit(code)
}

pub fn human(action: &Action) -> fn(&Value) {
    match action {
        Action::Doctor { .. } => |data| {
            for executor in data["executors"].as_array().expect("doctor executors") {
                for check in executor["checks"].as_array().expect("executor checks") {
                    let item = format!("{}: {}", text(&executor["executor"]), text(&check["item"]));
                    print_check(check["ok"] == true, &item, &check["fix"]);
                }
            }
            println!("claude profile: {}", text(&data["claude_profile"]));
            print_check(data["rex"]["ok"] == true, "rex", &data["rex"]["fix"]);
            print_check(data["pane_loop"]["watch_available"] == true, "watch", &data["pane_loop"]["fix"]);
        },
        Action::Ls { .. } => |data| {
            for entry in data["runs"].as_array().expect("run list") {
                if entry["error"].is_string() {
                    println!("{}\tbroken\t{}", text(&entry["run_id"]), text(&entry["error"]));
                    continue;
                }
                let run: Run = serde_json::from_value(entry["run"].clone()).expect("serialized run");
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    run.id,
                    run.name.escape_debug(),
                    text(&json!(run.launch.target())["executor"]),
                    text(&entry["observation"]["state"]),
                    run.attempt
                );
            }
        },
        Action::Status { .. } => |data| {
            let run: Run = serde_json::from_value(data["run"].clone()).expect("serialized run");
            let dir = run.brief.parent().expect("brief is stored in the run directory");
            println!(
                "state: {}\nevidence: {}\nexecutor: {}\nattempt: {}\nrun dir: {}\nreport path: {}\nlatest verdict: {}",
                text(&data["observation"]["state"]),
                text(&data["observation"]["evidence"]),
                text(&json!(run.launch.target())["executor"]),
                run.attempt,
                dir.display(),
                dir.join(run.report_name()).display(),
                run.verifications
                    .iter()
                    .rev()
                    .find(|v| v.attempt == run.attempt)
                    .map(|v| text(&json!(v.verdict)))
                    .unwrap_or_else(|| "none".into())
            );
        },
        Action::Issue { command: IssueAction::Ls { .. } } => |data| {
            for issue in data["issues"].as_array().expect("issue list") {
                let log = &issue["log"];
                println!(
                    "{}\t{}\t{}\t{}",
                    text(&log["id"]),
                    text(&log["area"]),
                    text(&log["title"]),
                    if issue["resolution"].is_null() { "open" } else { "resolved" }
                );
            }
        },
        Action::Dispatch(_) => |data| {
            if let Some(id) = data["run_id"].as_str() {
                println!("{id}");
                for key in ["launch_error", "pane_error"] {
                    if let Some(e) = data[key].as_str() {
                        eprintln!("{e}");
                    }
                }
            } else {
                pretty(data);
            }
        },
        Action::Report { .. } => |data| {
            let report = &data["report"];
            println!("{}", report["path"].as_str().expect("report path"));
            if let Some(content) = report["content"].as_str() {
                print!("{content}");
            } else {
                eprintln!("no report yet; state={}", data["observation"]["state"]);
            }
        },
        _ => pretty,
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.escape_debug().to_string(),
        Value::Null => "none".into(),
        _ => value.to_string(),
    }
}
fn print_check(ok: bool, item: &str, fix: &Value) {
    print!("{} {item}", if ok { "ok" } else { "missing" });
    if !ok {
        print!("; fix: {}", text(fix));
    }
    println!();
}
pub fn pretty(data: &Value) {
    println!("{}", serde_json::to_string_pretty(data).expect("JSON value is serializable"));
}
