use crate::{
    domain::{Executor, Run, State},
    handlers::{Launched, ListEntry, Output, RunDetail},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

pub fn emit(json_output: bool, result: Result<(Output, i32)>, failure_code: i32) -> ! {
    let (output, error, code) = match result {
        Ok((output, code)) => (Some(output), None, code),
        Err(e) => (None, Some(format!("{e:#}")), failure_code),
    };
    if json_output {
        let data = output.as_ref().map(data).unwrap_or(Value::Null);
        println!("{}", json!({"ok":code == 0,"data":data,"error":error}));
    } else if let Some(error) = error {
        eprintln!("{error}");
    } else if let Some(output) = output {
        human(&output);
    }
    std::process::exit(code)
}

fn data(output: &Output) -> Value {
    match output {
        Output::Doctor(doctor) => json!(doctor),
        Output::Ls { runs, summary } => json!({"runs":runs,"summary":summary}),
        Output::Wait { runs, message } => json!({"runs":runs,"message":message}),
        Output::Launch(launched) => launch_data(launched),
        Output::Launches { runs, error } => {
            json!({"runs":runs.iter().map(launch_data).collect::<Vec<_>>(),"error":error})
        }
        Output::Run { run, observation, detail } => {
            let report = match detail {
                RunDetail::Report { path, content: Some(content) } => json!({"path":path,"content":content}),
                RunDetail::Report { path, content: None } => json!({"path":path,"message":"no report yet"}),
                _ => Value::Null,
            };
            let mut data = json!({"run":run,"observation":observation,"report":report});
            if let RunDetail::Status { tail: Some(tail) } = detail {
                data["tail"] = json!(tail);
            }
            data
        }
        Output::Message(message) | Output::Rejection(message) => json!({"message":message}),
    }
}

fn launch_data(Launched { run, launch_error, pane_error }: &Launched) -> Value {
    json!({"run_id":run.id,"run":run,"launch_error":launch_error,"pane_error":pane_error})
}

fn print_launch(Launched { run, launch_error, pane_error }: &Launched) {
    println!("{}\t{}", run.id, run.name.escape_debug().to_string().escape_debug());
    for error in [launch_error, pane_error].into_iter().flatten() {
        eprintln!("{error}");
    }
}

fn human(output: &Output) {
    match output {
        Output::Doctor(doctor) => {
            for executor in &doctor.executors {
                for check in &executor.checks {
                    let item = format!("{}: {}", executor.executor.as_str(), path_text(&check.item).escape_debug());
                    print_check(check.ok, &item, check.fix.as_deref());
                }
            }
            if doctor.executors.iter().any(|e| e.executor == Executor::Claude) {
                let profile = doctor.claude_profile.as_deref().map(path_text).unwrap_or("none");
                println!("claude profile: {}", profile.escape_debug());
            }
            print_check(doctor.rex.ok, "rex", doctor.rex.fix);
        }
        Output::Ls { runs, summary } => {
            for entry in runs {
                match entry {
                    ListEntry::Broken { run_id, error } => {
                        println!("{}\tbroken\t{}", run_id.escape_debug(), error.escape_debug());
                    }
                    ListEntry::Run { run, observation } => println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        run.id,
                        if run.name == run.id { "-" } else { &run.name }.escape_debug(),
                        run.goal.as_deref().unwrap_or("-").escape_debug(),
                        run.launch.target().executor().as_str(),
                        observation.state().as_str(),
                        run.attempt,
                        run.model.as_deref().unwrap_or("-").escape_debug()
                    ),
                }
            }
            for (executor, states) in summary {
                let total: usize = states.values().sum();
                let counts =
                    states.iter().map(|(state, count)| format!("{state} {count}")).collect::<Vec<_>>().join(", ");
                println!("{executor}: {total} runs ({counts})");
            }
        }
        Output::Wait { runs, message } => {
            for run in runs {
                println!(
                    "{}\t{}\t{}\t{}",
                    run.id.escape_debug(),
                    run.name.escape_debug().to_string().escape_debug(),
                    run.state.as_str(),
                    path_text(&run.report_path).escape_debug()
                );
            }
            if let Some(message) = message {
                eprintln!("{message}");
            }
        }
        Output::Launch(launched) => print_launch(launched),
        Output::Launches { runs, error } => {
            runs.iter().for_each(print_launch);
            if let Some(error) = error {
                eprintln!("{error}");
            }
        }
        Output::Message(message) => println!("{message}"),
        Output::Rejection(message) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"message":message})).expect("JSON value is serializable")
            );
        }
        Output::Run { run, observation, detail } => match detail {
            RunDetail::Status { tail } => {
                let dir = run.brief.parent().expect("brief is stored in the run directory");
                println!("state: {}", observation.state().as_str());
                if let Some(cause) = observation.cause() {
                    println!("cause: {}", cause.as_str());
                }
                println!("evidence:");
                for line in observation.evidence.lines() {
                    println!("  {line}");
                }
                println!(
                    "executor: {}\nattempt: {}\nrun dir: {}\nreport path: {}\nlatest verdict: {}",
                    run.launch.target().executor().as_str(),
                    run.attempt,
                    dir.display(),
                    dir.join(run.report_name()).display(),
                    run.verifications
                        .iter()
                        .rev()
                        .find(|v| v.attempt == run.attempt)
                        .map(|v| v.verdict.as_str())
                        .unwrap_or("none")
                );
                if let Some(tail) = tail {
                    if let Some(message) = &tail.message {
                        println!("{message}");
                    } else if let Some(path) = &tail.path {
                        println!("tail: {}", path.display());
                        for line in &tail.lines {
                            println!("  {line}");
                        }
                    }
                }
            }
            RunDetail::Report { path, content } => {
                println!("{}", path_text(path));
                if let Some(content) = content {
                    print!("{content}");
                } else {
                    eprintln!("no report yet; state=\"{}\"", observation.state().as_str());
                }
            }
            RunDetail::Resume => {
                println!("{} attempt {}: {}", run.id.escape_debug(), run.attempt, observation.state().as_str());
            }
            RunDetail::Verify => {
                let verification = run.verifications.last().expect("verify records a verification");
                println!("{} attempt {}", verification.verdict.as_str(), verification.attempt);
                for check in &verification.checks {
                    if check.exit_code != Some(0) {
                        println!("{}: {}", check.command.escape_debug(), check.output.lines().next().unwrap_or(""));
                    }
                }
            }
            RunDetail::Stop => {
                println!("{}: {}", run.id.escape_debug(), observation.state().as_str());
                if observation.state() != State::Stopped {
                    println!("{}", observation.evidence);
                }
            }
        },
    }
}

pub fn wait_progress(run: &Run, state: State) {
    println!("{}\t{}\t{}", run.id, run.name.escape_debug(), state.as_str());
}

pub fn path_text(path: &Path) -> &str {
    // Match Serde's Path serialization error before a typed output is returned.
    path.to_str()
        .ok_or_else(|| <serde_json::Error as serde::ser::Error>::custom("path contains invalid UTF-8 characters"))
        .unwrap()
}

fn print_check(ok: bool, item: &str, fix: Option<&str>) {
    print!("{} {item}", if ok { "ok" } else { "missing" });
    if !ok {
        print!("; fix: {}", fix.unwrap_or("none").escape_debug());
    }
    println!();
}
