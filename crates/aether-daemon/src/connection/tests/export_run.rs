//! `ExportRun` masks with the daemon's own redactor, known secrets included.
use crate::authz::AuthContext;
use crate::connection::dispatch::dispatch_version;
use aether_core::event::Actor;
use aether_protocol::{Command, Reply};
use base64::Engine as _;

const KNOWN: &str = "fleet-token-0123456789abcdef";

#[tokio::test]
async fn an_export_through_the_daemon_masks_its_known_secrets() {
    let store: std::sync::Arc<dyn aether_store::Store> =
        std::sync::Arc::new(aether_store::SqliteStore::open_in_memory().expect("store"));
    let (daemon, _config_dir) = super::daemon_with_config(store.clone(), |config| {
        config.redactor = aether_tools::Redactor::with_known_secrets([KNOWN]).expect("compiles");
    })
    .await;
    let (agent, _) = daemon
        .runtime
        .create_agent(
            aether_runtime::CreateAgentSpec {
                role: "root".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    daemon
        .runtime
        .publish_artifact(
            aether_runtime::artifacts::ArtifactPublication {
                kind: aether_core::artifact::ArtifactKind::File,
                name: "deploy.log".into(),
                summary: format!("token {KNOWN}"),
                content_type: Some("text/plain".into()),
                bytes: format!("token {KNOWN}\n").into_bytes(),
                agent: agent.id,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    daemon
        .runtime
        .cancel_agent(agent.id, None, Actor::user())
        .expect("a finished run exports");

    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("run.aether");
    let command = Command::ExportRun {
        run: agent.run,
        path: path.clone(),
    };
    let reply = dispatch_version(&daemon, command, Actor::user(), &AuthContext::Local).await;
    let Ok(Reply::RunExported(summary)) = reply else {
        panic!("an export reply: {reply:?}");
    };
    assert_eq!(summary.artifact_payloads, 1);
    let text = std::fs::read_to_string(&path).expect("read export");
    assert!(
        !text.contains(KNOWN),
        "an exported record carries the secret"
    );
    let payload = text
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("a JSON line"))
        .find(|line| line["record"] == "artifact_bytes")
        .and_then(|line| line["base64"].as_str().map(str::to_owned))
        .expect("a payload record");
    let payload = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .expect("base64");
    assert_eq!(payload, b"token [REDACTED:known_secret]\n");
    daemon.runtime.shutdown().await;
}
