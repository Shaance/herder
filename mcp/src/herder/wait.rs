use super::{CLI_PROTOCOL_ERROR, CLI_START_ERROR, Companion, Envelope, HerderUnavailable, WorkerState};
use crate::WaitArgs;
use anyhow::{Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{num::NonZeroU32, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

#[derive(Deserialize)]
struct WaitReply {
    runs: [WaitRun; 1],
    timed_out: bool,
}

#[derive(Deserialize)]
struct WaitRun {
    id: String,
    attempt: NonZeroU32,
    state: WorkerState,
}

impl Companion {
    pub async fn wait_for_run(&self, args: WaitArgs) -> Result<Value> {
        let seconds = args.timeout_seconds.unwrap_or(15);
        ensure!((1..=30).contains(&seconds), "timeout_seconds must be between 1 and 30");
        let timeout_arg = seconds.to_string();
        let flags = ["--timeout".as_ref(), timeout_arg.as_ref()];
        let cli_args = self.exact_args("wait", &args.run_id, &flags)?;
        let mut child = Command::from(self.cli_command(&cli_args))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| CLI_START_ERROR)?;
        let mut stdout = child.stdout.take().expect("Herder wait was spawned with piped stdout");
        let mut bytes = Vec::new();
        // Allow the CLI's full worker wait budget plus time to start and emit its JSON reply.
        let exit = match timeout(Duration::from_secs(seconds + 2), async {
            tokio::try_join!(child.wait(), stdout.read_to_end(&mut bytes))
        })
        .await
        {
            Ok(Ok((status, _))) => status,
            Ok(Err(_)) => {
                let _ = child.kill().await;
                bail!(HerderUnavailable("Could not read the Herder wait process. Check the configured executable"));
            }
            Err(_) => {
                let _ = child.kill().await;
                bail!(HerderUnavailable(
                    "Herder did not return from wait within its process deadline. Check the configured executable"
                ));
            }
        };

        let reply = Envelope::parse(exit, &bytes)?.exact_observation()?;
        let ok = reply.ok;
        let WaitReply { runs: [run], timed_out } = reply.decode()?;
        ensure!(run.id == args.run_id, CLI_PROTOCOL_ERROR);
        let settled = matches!(
            run.state,
            WorkerState::Blocked | WorkerState::Finished | WorkerState::Failed | WorkerState::Stopped
        );
        ensure!(timed_out != settled && ok == matches!(run.state, WorkerState::Finished), CLI_PROTOCOL_ERROR);
        Ok(json!({
            "outcome": if timed_out { "timed_out" } else { "settled" }, "run_id": run.id,
            "attempt": run.attempt, "state": run.state, "timed_out": timed_out,
        }))
    }
}
