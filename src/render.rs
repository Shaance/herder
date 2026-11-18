use crate::{
    doctor::CheckStatus,
    domain::{Executor, FreshVerification, Run, State, Verdict},
    handlers::{Launched, ListEntry, Output, RunDetail, Tail},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{io::IsTerminal, path::Path};

pub fn emit(json_output: bool, result: Result<(Output, i32)>, failure_code: i32) -> ! {
    let (data, error, code) = match result {
        Ok((output, code)) if json_output => (data(&output), None, code),
        Err(error) if json_output => (Value::Null, Some(format!("{error:#}")), failure_code),
        Ok((output, code)) => {
            human(&output);
            std::process::exit(code)
        }
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(failure_code)
        }
    };
    println!("{}", json!({"ok":code == 0,"data":data,"error":error}));
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
            if let RunDetail::Status { tail: Some(tail), .. } = detail {
                data["tail"] = match tail {
                    Tail::Lines(path, lines) => json!({"path":path,"lines":lines,"message":null}),
                    Tail::Unavailable(path, message) => json!({"path":path,"lines":[],"message":message}),
                    Tail::NoStream(message) => json!({"path":null,"lines":[],"message":message}),
                };
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
    let terminal = std::io::stdout().is_terminal();
    let color = terminal && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    match output {
        Output::Doctor(doctor) => {
            for executor in &doctor.executors {
                for check in &executor.checks {
                    let item = format!("{}: {}", executor.executor.as_str(), path_text(&check.item).escape_debug());
                    print_check(&item, &check.status);
                }
            }
            if doctor.executors.iter().any(|e| e.executor == Executor::Claude) {
                let profile = doctor.claude_profile.as_deref().map(path_text).unwrap_or("none");
                println!("claude profile: {}", profile.escape_debug());
            }
            print_check("rex", &doctor.rex);
        }
        Output::Ls { runs, summary } => {
            if terminal {
                let rows = runs
                    .iter()
                    .map(|entry| match entry {
                        ListEntry::Broken { run_id, error } => [
                            run_id.escape_debug().to_string(),
                            "-".into(),
                            "-".into(),
                            "-".into(),
                            "broken".into(),
                            "-".into(),
                            error.escape_debug().to_string(),
                        ],
                        ListEntry::Run { run, observation } => [
                            run.id.escape_debug().to_string(),
                            if run.name == run.id { "-" } else { &run.name }.escape_debug().to_string(),
                            run.goal.as_deref().unwrap_or("-").escape_debug().to_string(),
                            run.launch.target().executor().as_str().into(),
                            observation.state().as_str().into(),
                            run.attempt.to_string(),
                            run.model.as_deref().unwrap_or("-").escape_debug().to_string(),
                        ],
                    })
                    .collect::<Vec<_>>();
                println!("{}", table(&rows, color));
            } else {
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
            RunDetail::Status { dir, tail } => {
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
                match tail {
                    Some(Tail::Unavailable(_, message) | Tail::NoStream(message)) => println!("{message}"),
                    Some(Tail::Lines(path, lines)) => {
                        println!("tail: {}", path.display());
                        for line in lines {
                            println!("  {line}");
                        }
                    }
                    None => {}
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
            RunDetail::Verify(verification) => {
                print!("{}", verification_text(&run.name, verification, color));
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

fn paint(text: &str, code: &str, enabled: bool) -> String {
    if enabled && !code.is_empty() { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_owned() }
}

fn table(rows: &[[String; 7]], color: bool) -> String {
    let header = ["ID", "NAME", "GOAL", "EXECUTOR", "STATE", "ATTEMPT", "MODEL"];
    let mut widths = header.map(str::len);
    for row in rows {
        for (width, value) in widths.iter_mut().zip(row) {
            *width = (*width).max(value.chars().count());
        }
    }
    let mut output = String::new();
    for (index, value) in header.iter().enumerate() {
        output.push_str(value);
        if index < header.len() - 1 {
            output.push_str(&" ".repeat(widths[index] - value.len() + 2));
        }
    }
    output.push('\n');
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            let code = if value == "-" {
                "2"
            } else if index == 4 {
                match value.as_str() {
                    "failed" | "broken" => "31",
                    "finished" => "32",
                    "starting" | "running" | "blocked" => "33",
                    "unknown" => "35",
                    _ => "",
                }
            } else {
                ""
            };
            output.push_str(&paint(value, code, color));
            if index < row.len() - 1 {
                output.push_str(&" ".repeat(widths[index] - value.chars().count() + 2));
            }
        }
        output.push('\n');
    }
    output
}

fn verification_text(name: &str, verification: &FreshVerification, color: bool) -> String {
    let (verdict, code) = match verification.verdict {
        Verdict::Pass => ("PASS", "32"),
        Verdict::Fail => ("FAIL", "31"),
    };
    let mut output =
        format!("{}  {}  attempt {}\n", paint(verdict, code, color), name.escape_debug(), verification.attempt);
    let scope = std::iter::once(("scope".to_owned(), &verification.scope));
    let commands =
        verification.commands.iter().map(|check| (format!("check  {}", check.command.escape_debug()), check));
    for (label, check) in scope.chain(commands) {
        let (result, code) = match check.exit_code {
            Some(0) => ("ok".to_owned(), "32"),
            Some(exit) => (format!("failed (exit {exit})"), "31"),
            None => ("failed (no exit code)".to_owned(), "31"),
        };
        output.push_str(&format!("  {label}  {}\n", paint(&result, code, color)));
        if check.exit_code != Some(0) {
            let mut remaining = 2000;
            let mut lines = check.output.lines();
            let mut truncated = false;
            for line in lines.by_ref().take(20) {
                let escaped = line.escape_debug().to_string();
                let prefix: String = escaped.chars().take(remaining).collect();
                remaining -= prefix.chars().count();
                output.push_str(&format!("    {prefix}\n"));
                if prefix.len() < escaped.len() {
                    truncated = true;
                    break;
                }
            }
            if truncated || lines.next().is_some() {
                output.push_str("    ... (output truncated; see run.json)\n");
            }
        }
    }
    output
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

fn print_check(item: &str, status: &CheckStatus) {
    match status {
        CheckStatus::Present => println!("ok {item}"),
        CheckStatus::Missing(fix) => println!("missing {item}; fix: {}", fix.escape_debug()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Check;
    use std::num::NonZeroU32;

    #[test]
    fn table_aligns_header_and_rows_before_coloring() {
        let rows = [
            ["1", "alpha", "-", "codex", "finished", "1", "-"].map(str::to_owned),
            ["broken", "-", "-", "-", "broken", "-", "bad record"].map(str::to_owned),
        ];
        assert_eq!(
            table(&rows, false),
            "ID      NAME   GOAL  EXECUTOR  STATE     ATTEMPT  MODEL\n\
             1       alpha  -     codex     finished  1        -\n\
             broken  -      -     -         broken    -        bad record\n"
        );
        let colored = table(&rows, true);
        assert!(colored.contains("codex     \x1b[32mfinished\x1b[0m  1        \x1b[2m-\x1b[0m\n"));
        assert!(colored.contains("\x1b[31mbroken\x1b[0m    \x1b[2m-\x1b[0m"));
        assert!(!table(&rows, false).contains('\x1b'));
    }

    #[test]
    fn table_colors_each_state_without_coloring_other_values() {
        let rows = ["starting", "running", "blocked", "failed", "unknown", "stopped"]
            .map(|state| ["id", "name", "goal", "codex", state, "1", "model"].map(str::to_owned));
        let output = table(&rows, true);
        for state in ["starting", "running", "blocked"] {
            assert!(output.contains(&format!("\x1b[33m{state}\x1b[0m")));
        }
        assert!(output.contains("\x1b[31mfailed\x1b[0m"));
        assert!(output.contains("\x1b[35munknown\x1b[0m"));
        assert!(output.contains("codex     stopped"));
        assert!(!output.contains("\x1b[33mname"));
    }

    #[test]
    fn verification_layout_shows_results_and_failed_output() {
        let verification = FreshVerification {
            verdict: Verdict::Fail,
            attempt: NonZeroU32::new(2).unwrap(),
            scope: Check { command: String::new(), exit_code: Some(1), output: "outside src/\nfile".into() },
            commands: vec![
                Check { command: "true".into(), exit_code: Some(0), output: "not displayed".into() },
                Check { command: "exit 7".into(), exit_code: Some(7), output: "first\nsecond\n".into() },
                Check { command: "signal".into(), exit_code: None, output: "no status".into() },
            ],
        };
        assert_eq!(
            verification_text("worker\nname", &verification, false),
            concat!(
                "FAIL  worker\\nname  attempt 2\n",
                "  scope  failed (exit 1)\n",
                "    outside src/\n",
                "    file\n",
                "  check  true  ok\n",
                "  check  exit 7  failed (exit 7)\n",
                "    first\n",
                "    second\n",
                "  check  signal  failed (no exit code)\n",
                "    no status\n"
            )
        );
        let colored = verification_text("worker", &verification, true);
        assert!(colored.contains("\x1b[31mFAIL\x1b[0m"));
        assert!(colored.contains("\x1b[31mfailed (exit 7)\x1b[0m"));
        assert!(colored.contains("\x1b[32mok\x1b[0m"));
        assert!(!colored.contains("not displayed"));
    }

    #[test]
    fn verification_trims_output_and_escapes_control_codes() {
        let verification = FreshVerification {
            verdict: Verdict::Fail,
            attempt: NonZeroU32::MIN,
            scope: Check { command: String::new(), exit_code: Some(0), output: String::new() },
            commands: vec![Check {
                command: "echo\x1b".into(),
                exit_code: Some(3),
                output: format!("\x1b{}", "x".repeat(2500)),
            }],
        };
        let output = verification_text("name", &verification, false);
        assert!(!output.contains('\x1b'));
        assert!(output.contains("echo\\u{1b}"));
        assert!(output.contains("... (output truncated; see run.json)"));
        assert!(output.len() < 2200);
    }
}
