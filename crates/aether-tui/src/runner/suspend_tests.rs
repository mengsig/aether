#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use aether_client::Transport;
use aether_core::event::{Actor, Event as StoredEvent, EventDraft, EventKind};
use aether_core::id::EventId;
use aether_core::model::{ContentBlock, MessageRole};
use aether_protocol::{PROTOCOL_VERSION, Reply, ServerFrame, Welcome};

#[test]
#[ignore = "child entry point: requires the suspend/fg PTY controller"]
fn suspend_loop_child() {
    let output =
        std::fs::File::create(std::env::var_os("AETHER_SUSPEND_RECEIPT").expect("fixture receipt"))
            .expect("receipt file");
    let mut observer = process_tests::Observer::new(output);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("CLI runtime");
    runtime.block_on(async {
        let fixture = crate::testing::app_with_transcript(&[]);
        let agent = fixture.selected.expect("fixture agent");
        let tree = fixture.tree;
        let history = StoredEvent::from_draft(
            EventDraft::new(
                Actor::Agent { id: agent },
                EventKind::AgentMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::text("conversation before suspension")],
                    invocation: None,
                },
            )
            .agent(agent),
            EventId::new(),
            1,
            aether_core::time::now(),
        );
        let (transport, mut peer) = Transport::memory(32);
        let daemon = tokio::spawn(async move {
            while let Some(frame) = peer.incoming.recv().await {
                let reply = match frame.command {
                    Command::Hello(_) => Reply::Welcome(Welcome {
                        protocol_version: PROTOCOL_VERSION,
                        daemon_version: "suspend-fixture".into(),
                        instance_id: "suspend-fixture".into(),
                        schema_version: 1,
                        pid: std::process::id(),
                        features: vec![],
                        daemon_protocol_version: None,
                        min_protocol_version: None,
                        idle_timeout_secs: None,
                        device: None,
                        submission_horizon: None,
                    }),
                    Command::Subscribe { .. } => Reply::Subscribed { id: 1, last_seq: 1 },
                    Command::GetTree { .. } => Reply::Tree(tree.clone()),
                    Command::GetEvents { .. } => Reply::Events(vec![history.clone()]),
                    _ => Reply::Ok,
                };
                peer.outgoing
                    .send(ServerFrame::Reply {
                        id: frame.id,
                        result: Ok(reply),
                    })
                    .await
                    .expect("fixture reply");
            }
        });
        let client = Client::open(transport).await.expect("fixture client");
        let result = run_inner(
            client,
            TuiOptions {
                focus: Some(agent),
                home: Some(std::env::current_dir().expect("fixture home")),
                ..Default::default()
            },
            Some(&mut observer),
        )
        .await;
        daemon.abort();
        if let Err(error) = daemon.await {
            assert!(error.is_cancelled(), "fixture daemon failed: {error}");
        }
        result.expect("running input loop must resume");
    });
}

#[test]
fn running_input_loop_survives_suspend_and_fg() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test_tui_suspend.py");
    let status = std::process::Command::new("python3")
        .arg(script)
        .arg(std::env::current_exe().expect("test executable"))
        .status()
        .expect("PTY controller");
    assert!(status.success(), "suspend/fg controller: {status}");
}
