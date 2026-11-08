use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use aether_runtime::Runtime;
use aether_store::SqliteStore;
use tokio::task::JoinSet;

use crate::instance_lock::InstanceLock;
use crate::noesis::NoesisService;

const RUNTIME_DEADLINE: Duration = Duration::from_secs(30);
const DRAIN_DEADLINE: Duration = Duration::from_secs(3);
const STORE_DEADLINE: Duration = Duration::from_secs(2);
const CLEANUP_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct Locks {
    _store: Arc<InstanceLock>,
    _socket: Arc<InstanceLock>,
    _socket_alias: Option<Arc<tempfile::TempDir>>,
}

impl Locks {
    pub(crate) fn new(
        store: Arc<InstanceLock>,
        socket: Arc<InstanceLock>,
        socket_alias: Option<tempfile::TempDir>,
    ) -> Self {
        Self {
            _store: store,
            _socket: socket,
            _socket_alias: socket_alias.map(Arc::new),
        }
    }
}

async fn phase(
    name: &'static str,
    deadline: Duration,
    work: impl Future<Output = bool> + Send + 'static,
) -> bool {
    assert!(!name.is_empty());
    assert!(!deadline.is_zero());
    let task = tokio::spawn(work);
    match tokio::time::timeout(deadline, task).await {
        Ok(Ok(clean)) => clean,
        Ok(Err(error)) => {
            tracing::error!(phase = name, %error, "shutdown phase failed");
            false
        }
        Err(_) => {
            tracing::warn!(
                phase = name,
                ?deadline,
                "shutdown phase exceeded its deadline"
            );
            false
        }
    }
}

pub(crate) async fn runtime(runtime: Arc<Runtime>, locks: Locks) -> bool {
    phase(
        "runtime and agent processes",
        RUNTIME_DEADLINE,
        async move {
            let _locks = locks;
            let outcome = runtime.shutdown().await;
            if !outcome.surviving_agent_processes.is_empty() {
                tracing::warn!(pids = ?outcome.surviving_agent_processes, "agent processes survived shutdown");
                return false;
            }
            true
        },
    )
    .await
}

pub(crate) async fn connections(mut connections: JoinSet<()>, locks: Locks) -> bool {
    phase("connections", DRAIN_DEADLINE, async move {
        let _locks = locks;
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "connection task failed");
            }
        }
        true
    })
    .await
}

pub(crate) async fn noesis(noesis: Arc<NoesisService>, locks: Locks) -> bool {
    phase("noesis", DRAIN_DEADLINE, async move {
        let _locks = locks;
        noesis.shutdown().await;
        true
    })
    .await
}

pub(crate) async fn store(
    store: Arc<SqliteStore>,
    checkpoint: bool,
    locks: Locks,
) -> anyhow::Result<bool> {
    if !checkpoint {
        tracing::warn!("skipping final WAL sync because an earlier shutdown phase did not finish");
    }
    store_with(store, checkpoint, STORE_DEADLINE, locks, |store| {
        store.sync()
    })
    .await
}

async fn store_with(
    store: Arc<SqliteStore>,
    checkpoint: bool,
    deadline: Duration,
    guard: impl Send + 'static,
    sync: impl FnOnce(&SqliteStore) -> aether_store::Result<bool> + Send + 'static,
) -> anyhow::Result<bool> {
    assert!(!deadline.is_zero());
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("final-store-sync".into())
        .spawn(move || {
            let _guard = guard;
            let result = if checkpoint {
                sync(&store).map(Some)
            } else {
                Ok(None)
            };
            drop(store);
            match send.send(result) {
                Err(Err(error)) => {
                    tracing::error!(%error, "final store sync failed after its deadline");
                }
                Err(Ok(Some(false))) => {
                    tracing::warn!("final WAL sync left frames for a reader; they stay in the WAL");
                }
                _ => {}
            }
        })?;
    match tokio::time::timeout(deadline, receive).await {
        Ok(Ok(Ok(Some(true)))) => Ok(true),
        Ok(Ok(Ok(Some(false)))) => {
            tracing::warn!("final WAL sync left frames for a reader; they stay in the WAL");
            Ok(true)
        }
        Ok(Ok(Ok(None))) => Ok(false),
        Ok(Ok(Err(error))) => Err(anyhow::Error::new(error).context("final store sync")),
        Ok(Err(error)) => Err(anyhow::Error::new(error).context("final store sync thread")),
        Err(_) => {
            tracing::warn!(
                phase = "final store sync",
                ?deadline,
                "shutdown phase exceeded its deadline; committed data remains in the WAL"
            );
            Ok(false)
        }
    }
}

pub(crate) struct Cleanup {
    pub socket: std::path::PathBuf,
    pub socket_published: Option<std::fs::Metadata>,
    pub pid_path: std::path::PathBuf,
    pub pid_published: Option<std::fs::File>,
    pub bridge: Option<crate::bridge::Bridge>,
    pub store_path: std::path::PathBuf,
    pub clear_crash_marker: bool,
}

impl Cleanup {
    fn run(self) -> anyhow::Result<()> {
        let socket = self.socket_published.map_or(Ok(()), |identity| {
            crate::remove_daemon_file(&self.socket, &identity)
        });
        let pid = self.pid_published.map_or(Ok(()), |file| {
            file.metadata()
                .map_err(anyhow::Error::new)
                .and_then(|identity| crate::remove_daemon_file(&self.pid_path, &identity))
        });
        let bridge = self.bridge.map_or(Ok(()), crate::bridge::Bridge::remove);
        if self.clear_crash_marker {
            crate::crash::end_run(&self.store_path);
        }
        socket.and(pid).and(bridge)
    }
}

pub(crate) async fn cleanup(cleanup: Cleanup, locks: Locks) -> anyhow::Result<()> {
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("daemon-cleanup".into())
        .spawn(move || {
            let _locks = locks;
            if let Err(Err(error)) = send.send(cleanup.run()) {
                tracing::error!(%error, "daemon cleanup failed after its deadline");
            }
        })?;
    match tokio::time::timeout(CLEANUP_DEADLINE, receive).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(anyhow::Error::new(error).context("daemon cleanup thread")),
        Err(_) => {
            tracing::warn!(
                phase = "daemon cleanup",
                "shutdown phase exceeded its deadline"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::event::{Actor, EventDraft, EventKind};
    use aether_store::{Commit, EventFilter, Store};
    use std::io::Write;

    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn stalled_final_sync_times_out_and_committed_rows_survive() {
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || Log(writer.clone()))
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        store
            .commit(Commit::new().event(EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: "before-stop".into(),
                    payload: serde_json::Value::Null,
                },
            )))
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let stopping = tokio::spawn(store_with(
            store,
            true,
            Duration::from_millis(100),
            (),
            move |store| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let result = store.sync();
                finished_tx.send(()).unwrap();
                result
            },
        ));
        tokio::time::timeout(Duration::from_secs(3), started_rx)
            .await
            .unwrap()
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(3), stopping)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!outcome, "the stalled checkpoint must exceed its deadline");
        let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains("final store sync") && log.contains("exceeded its deadline"),
            "{log}"
        );
        let reopened = SqliteStore::open(&path).unwrap();
        let events = reopened.events(0, 100, &EventFilter::default()).unwrap();
        assert!(events.iter().any(
            |event| matches!(&event.kind, EventKind::Custom { name, .. } if name == "before-stop")
        ));
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), finished_rx)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn timed_out_phase_returns_without_cancelling_its_work() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let stopping = tokio::spawn(phase(
            "test phase",
            Duration::from_millis(100),
            async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                finished_tx.send(()).unwrap();
                true
            },
        ));
        tokio::time::timeout(Duration::from_secs(3), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_secs(3), stopping)
                .await
                .unwrap()
                .unwrap()
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), finished_rx)
            .await
            .unwrap()
            .unwrap();
    }
}
