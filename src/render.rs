use anyhow::Result;
use serde_json::{Value, json};

pub fn emit(json_output: bool, result: Result<(Value, i32)>, failure_code: i32) -> ! {
    let (data, error, code) = match result {
        Ok((data, code)) => (data, None, code),
        Err(e) => (Value::Null, Some(format!("{e:#}")), failure_code),
    };
    if json_output {
        println!("{}", json!({"ok":code == 0,"data":data,"error":error}));
    } else if let Some(error) = error {
        eprintln!("{error}");
    } else if let Some(id) = data.get("run_id").and_then(Value::as_str) {
        println!("{id}");
        for key in ["launch_error", "pane_error"] {
            if let Some(e) = data[key].as_str() {
                eprintln!("{e}");
            }
        }
    } else if let Some(report) = data.get("report").filter(|v| !v.is_null()) {
        println!("{}", report["path"].as_str().unwrap_or_default());
        if let Some(content) = report["content"].as_str() {
            print!("{content}");
        } else {
            eprintln!("no report yet; state={}", data["observation"]["state"]);
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&data).expect("JSON value is serializable"));
    }
    std::process::exit(code)
}
