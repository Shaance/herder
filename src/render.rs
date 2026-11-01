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
            if data["executors"].as_array().expect("doctor executors").iter().any(|e| e["executor"] == "claude") {
                println!("claude profile: {}", text(&data["claude_profile"]));
            }
            print_check(data["rex"]["ok"] == true, "rex", &data["rex"]["fix"]);
        },
        Action::Ls { .. } => |data| {
            for entry in data["runs"].as_array().expect("run list") {
                if entry["error"].is_string() {
                    println!("{}\tbroken\t{}", text(&entry["run_id"]), text(&entry["error"]));
                    continue;
                }
                let run: Run = serde_json::from_value(entry["run"].clone()).expect("serialized run");
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    run.id,
                    if run.name == run.id { "-" } else { &run.name }.escape_debug(),
                    run.goal.as_deref().unwrap_or("-").escape_debug(),
                    text(&json!(run.launch.target())["executor"]),
                    text(&entry["observation"]["state"]),
                    run.attempt,
                    run.model.as_deref().unwrap_or("-").escape_debug()
                );
            }
            for (executor, states) in data["summary"].as_object().expect("ls summary") {
                let states = states.as_object().expect("state counts");
                let total: u64 = states.values().map(|count| count.as_u64().unwrap()).sum();
                let counts =
                    states.iter().map(|(state, count)| format!("{state} {count}")).collect::<Vec<_>>().join(", ");
                println!("{executor}: {total} runs ({counts})");
            }
        },
        Action::Wait { .. } => |data| {
            for run in data["runs"].as_array().expect("wait runs") {
                println!(
                    "{}\t{}\t{}\t{}",
                    text(&run["id"]),
                    text(&run["name"]).escape_debug(),
                    text(&run["state"]),
                    text(&run["report_path"])
                );
            }
            if let Some(message) = data["message"].as_str() {
                eprintln!("{message}");
            }
        },
        Action::Status { .. } => |data| {
            let run: Run = serde_json::from_value(data["run"].clone()).expect("serialized run");
            let dir = run.brief.parent().expect("brief is stored in the run directory");
            println!("state: {}\nevidence:", text(&data["observation"]["state"]));
            for line in data["observation"]["evidence"].as_str().expect("observation evidence").lines() {
                println!("  {line}");
            }
            println!(
                "executor: {}\nattempt: {}\nrun dir: {}\nreport path: {}\nlatest verdict: {}",
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
                println!("{id}\t{}", text(&data["run"]["name"]).escape_debug());
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
        Action::Resume { .. } => |data| {
            println!(
                "{} attempt {}: {}",
                text(&data["run"]["id"]),
                data["run"]["attempt"],
                text(&data["observation"]["state"])
            );
        },
        Action::Verify { .. } => |data| {
            if let Some(message) = data["message"].as_str() {
                println!("{message}");
                return;
            }
            let verification =
                data["run"]["verifications"].as_array().expect("verifications").last().expect("recorded verification");
            println!("{} attempt {}", text(&verification["verdict"]), verification["attempt"]);
            for check in verification["checks"].as_array().expect("verification checks") {
                if check["exit_code"] != 0 {
                    println!(
                        "{}: {}",
                        text(&check["command"]),
                        check["output"].as_str().expect("check output").lines().next().unwrap_or("")
                    );
                }
            }
        },
        Action::Stop { .. } => |data| {
            println!("{}: {}", text(&data["run"]["id"]), text(&data["observation"]["state"]));
            if data["observation"]["state"] != "stopped" {
                println!("{}", data["observation"]["evidence"].as_str().expect("observation evidence"));
            }
        },
        Action::Issue { command: IssueAction::Log { .. } } => |data| println!("{}", text(&data["issue"]["id"])),
        Action::Issue { command: IssueAction::Resolve { .. } } => {
            |data| println!("{} resolved", text(&data["issue"]["id"]))
        }
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
