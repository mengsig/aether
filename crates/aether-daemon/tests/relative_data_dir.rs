#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use aether_protocol::PROTOCOL_VERSION;
use serde_json::json;
use support::RawConn;
use tokio_util::sync::CancellationToken;

/// `target` as a path relative to this process's working directory.
fn relative_to_cwd(target: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let target = target.canonicalize().unwrap();
    let up = cwd
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .map(|_| Component::ParentDir);
    let down = target
        .components()
        .filter(|part| matches!(part, Component::Normal(_)));
    let relative: PathBuf = up.chain(down).collect();
    assert!(relative.is_relative(), "{}", relative.display());
    assert_eq!(relative.canonicalize().unwrap(), target);
    relative
}

/// `AETHER_DATA_DIR` may be relative. An engine runs in its agent's workspace,
/// so a stored image has to be kept under the resolved, absolute store path.
#[tokio::test]
async fn a_relative_data_directory_stores_images_under_the_resolved_store() {
    const PNG_BASE64: &str = "iVBORw0KGgpwaXhlbHM=";
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let workspace = root.path().join("elsewhere");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let socket = root.path().join("aether.sock");
    let config = aether_daemon::DaemonConfig {
        socket: socket.clone(),
        store_path: relative_to_cwd(&data).join("aether.db"),
        config_dir: Some(root.path().join("config")),
        workspace_root: Some(workspace.clone()),
        max_concurrent_turns: None,
        yolo: false,
        tcp: None,
        allow_remote: false,
    };
    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(aether_daemon::run(config, shutdown.clone()));
    let mut conn = connected(&socket, &serving).await;

    let created = conn
        .call(
            json!({"cmd": "create_agent", "role": "reader", "start": false,
            "model": {"provider": "claude-code", "model": "haiku"},
            "scope": {"workspace_root": workspace}}),
        )
        .await;
    let agent = created["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("an agent id: {created}"))
        .to_owned();
    let sent = conn
        .call(
            json!({"cmd": "send_message", "to": agent, "content": "look",
            "attachments": [{"media_type": "image/png", "data": PNG_BASE64}]}),
        )
        .await;
    assert_eq!(sent["result"]["Ok"]["reply"], "message_sent", "{sent}");

    let stored = data.canonicalize().unwrap().join("aether.attachments");
    let files: Vec<PathBuf> = std::fs::read_dir(&stored)
        .unwrap_or_else(|error| panic!("{}: {error}", stored.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].is_absolute(), "{files:?}");
    assert_eq!(
        std::fs::read(&files[0]).unwrap(),
        b"\x89PNG\r\n\x1a\npixels"
    );

    shutdown.cancel();
    let ended = tokio::time::timeout(Duration::from_secs(30), serving).await;
    ended.unwrap().unwrap().unwrap();
}

async fn connected(socket: &Path, daemon: &tokio::task::JoinHandle<anyhow::Result<()>>) -> RawConn {
    for _ in 0..1200 {
        assert!(!daemon.is_finished(), "the daemon stopped before serving");
        if let Ok(mut conn) = RawConn::connect(socket).await
            && conn.hello(PROTOCOL_VERSION).await.is_ok()
        {
            return conn;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon never answered on {}", socket.display());
}
