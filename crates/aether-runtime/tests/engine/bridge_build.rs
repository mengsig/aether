//! Replacing the installed binary while the daemon runs leaves the MCP
//! bridge every native turn launches on the daemon's own build.
use super::*;

/// Starts the bridge command it is handed, as an engine starts its MCP
/// server, and records what that build printed.
struct BridgeProbe {
    inner: CheckpointEngine,
    printed: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Engine for BridgeProbe {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "bridge-probe/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let bridge = turn.mcp.as_ref().expect("a native turn carries the bridge");
        let output = tokio::process::Command::new(&bridge.command)
            .output()
            .await
            .expect("the bridge starts");
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        self.printed.lock().unwrap().push(printed);
        self.inner.run(turn, cancel).await
    }
}

/// An executable `aether` that prints `build`. A child writes it, so no fd
/// of this process is on it when a sibling test's fork would inherit one.
fn install(path: &std::path::Path, build: &str) {
    let status = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            r#"printf '#!/bin/sh\necho %s\n' "$1" > "$2" && chmod 755 "$2""#,
        ])
        .args(["sh", build])
        .arg(path)
        .status()
        .expect("spawn sh");
    assert!(status.success(), "writing {} failed", path.display());
}

#[tokio::test]
async fn a_binary_replaced_under_a_running_daemon_is_not_the_bridge() {
    let temp = tempfile::tempdir().unwrap();
    let installed_dir = temp.path().join("install");
    std::fs::create_dir(&installed_dir).unwrap();
    let installed = installed_dir.join("aether");
    install(&installed, "build-1");
    let bin = temp.path().join("data/bin");
    std::fs::create_dir(temp.path().join("data")).unwrap();
    let published = aether_core::bridge_bin::publish(&bin, &installed, "1.0.0").unwrap();
    let printed = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(BridgeProbe {
        inner: CheckpointEngine {
            turns: Arc::default(),
            valid: true,
        },
        printed: printed.clone(),
    });
    let command = published.path().to_path_buf();
    let h = harness_with_engine_config(engine, |config| {
        config.bridge.as_mut().unwrap().command = command.clone();
    })
    .await;
    let id = chat_agent(&h);
    engine_turn(&h, id, "before the update").await;

    // What `aether update` does: stage the new build beside the old, rename it over.
    let staged = installed_dir.join(".aether.new");
    install(&staged, "build-2");
    std::fs::rename(&staged, &installed).unwrap();
    let now = std::process::Command::new(&installed).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&now.stdout).trim(), "build-2");
    engine_turn(&h, id, "after the update").await;

    assert_eq!(*printed.lock().unwrap(), vec!["build-1", "build-1"]);
    assert!(
        aether_core::bridge_bin::sweep(&bin, false)
            .unwrap()
            .iter()
            .all(|file| file.action == aether_core::bridge_bin::SweepAction::Kept),
        "a sweep keeps the running daemon's copy"
    );
    assert!(command.exists());
    published.remove().unwrap();
}
