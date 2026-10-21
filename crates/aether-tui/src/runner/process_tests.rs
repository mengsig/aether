#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

pub(super) struct Observer {
    output: File,
    event: Option<Value>,
}

impl Observer {
    pub(super) fn new(output: File) -> Self {
        Self {
            output,
            event: None,
        }
    }

    pub(super) fn before(&mut self, event: &LoopEvent) {
        self.event = match event {
            #[cfg(unix)]
            LoopEvent::Suspend => Some(json!({"kind": "resumed"})),
            LoopEvent::App(
                AppEvent::Key(_)
                | AppEvent::Paste(_)
                | AppEvent::ImagePathPasted { .. }
                | AppEvent::Resize(_, _)
                | AppEvent::Focus(_),
            ) => Some(json!({"kind": "input"})),
            LoopEvent::App(AppEvent::Reply {
                then: Then::Backfill { .. },
                ..
            }) => Some(json!({"kind": "backfill"})),
            LoopEvent::App(AppEvent::ClipboardRead(_) | AppEvent::ImageRead(_)) => {
                Some(json!({"kind": "pasted"}))
            }
            LoopEvent::Reconnected(_) => Some(json!({"kind": "reconnected"})),
            LoopEvent::App(AppEvent::Subscribed { id, last_seq }) => {
                Some(json!({"kind": "subscribed", "id": id, "high_water": last_seq}))
            }
            LoopEvent::App(AppEvent::Disconnected { because }) => {
                Some(json!({"kind": "disconnected", "because": because}))
            }
            LoopEvent::App(AppEvent::Frame(SubscriptionFrame::Event(event))) => {
                Some(json!({"kind": "event", "event": event}))
            }
            LoopEvent::App(AppEvent::Reply {
                result: Err(ClientError::Disconnected),
                ..
            }) => Some(json!({"kind": "failed_reply"})),
            _ => None,
        };
    }

    pub(super) fn after(&mut self, app: &App) -> anyhow::Result<()> {
        let Some(mut record) = self.event.take() else {
            return Ok(());
        };
        record["cursor"] = json!(app.last_seq);
        record["connected"] = json!(app.connected);
        if matches!(
            record["kind"].as_str(),
            Some("input" | "resumed" | "backfill" | "pasted")
        ) {
            record["draft"] = json!(app.input.text);
            record["images"] = json!(app.input.images.len());
            record["flash"] = json!(app.live_flash());
            record["selected"] = json!(app.selected);
            record["size"] = json!(app.size);
            record["transcript"] = json!(
                app.selected
                    .and_then(|id| app.transcripts.get(&id))
                    .map(|transcript| format!("{:?}", transcript.cells))
            );
        }
        serde_json::to_writer(&mut self.output, &record)?;
        self.output.write_all(b"\n")?;
        self.output.flush()?;
        Ok(())
    }
}

#[test]
#[ignore = "child entry point: requires a PTY and disposable daemon from process budget controller"]
fn process_budget_child() {
    let socket = PathBuf::from(std::env::var_os("AETHER_BUDGET_SOCKET").expect("fixture socket"));
    let receipt = std::env::var_os("AETHER_BUDGET_RECEIPT").expect("fixture receipt");
    let mut observer = Observer::new(File::create(receipt).expect("receipt file"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("same runtime as the CLI");
    runtime.block_on(async {
        let client = Client::connect(&socket).await.expect("fixture connection");
        run_inner(
            client,
            TuiOptions {
                socket_path: Some(socket),
                ..Default::default()
            },
            Some(&mut observer),
        )
        .await
        .expect("real runner");
    });
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release process budget; scripts/ci.sh runs it"
)]
fn measured_process_budgets() {
    if !cfg!(target_os = "linux") {
        println!("SKIP process CPU/restart budgets: Linux /proc and PTY harness only");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runner_test = std::env::current_exe().expect("test executable");
    let binary = runner_test
        .parent()
        .expect("test dependencies directory")
        .parent()
        .expect("release directory")
        .join("aether");
    let result = std::process::Command::new("python3")
        .current_dir(&root)
        .arg("scripts/tui_process_budgets.py")
        .arg("--binary")
        .arg(binary)
        .arg("--runner-test")
        .arg(runner_test)
        .status()
        .expect("launch process budget controller");
    assert!(
        result.success(),
        "process budget controller failed: {result}"
    );
}
