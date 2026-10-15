use std::{sync::Arc, time::Duration};

use crate::Runtime;

pub(crate) fn spawn(runtime: Arc<Runtime>) {
    let interval = Duration::from_secs(u64::from(
        runtime.models.config().storage.gc_interval_seconds.get(),
    ));
    tokio::spawn(run(runtime, interval));
}

pub(super) async fn run(runtime: Arc<Runtime>, interval: Duration) {
    let mut startup = true;
    loop {
        let filesystem_only = startup;
        startup = false;
        let sweeping = runtime.clone();
        // Await this worker before starting another interval. Slow sweeps
        // never overlap or accumulate a backlog of blocking jobs.
        let result = tokio::task::spawn_blocking(move || {
            if sweeping.shutdown.is_cancelled() {
                return;
            }
            match sweeping.storage_gc(false) {
                Ok(report) => tracing::info!(%report, "storage maintenance finished"),
                Err(error) => tracing::error!(%error, "storage maintenance failed"),
            }
            // Artifact startup cleanup already ran; preserve the history recovery window
            // before the first periodic sweep.
            if filesystem_only {
                return;
            }
            match sweeping.sweep_artifacts() {
                Ok(0) => {}
                Ok(evicted) => tracing::info!(evicted, "retention dropped artifact bytes"),
                Err(error) => tracing::error!(%error, "artifact retention sweep failed"),
            }
            if sweeping.shutdown.is_cancelled() {
                return;
            }
            match sweeping.sweep_run_history_inner(aether_core::time::now(), true) {
                Ok(report) => tracing::info!(?report, "completed-run retention sweep finished"),
                Err(error) => tracing::error!(%error, "completed-run retention sweep failed"),
            }
        })
        .await;
        if let Err(error) = result {
            tracing::error!(%error, "retention worker failed");
        }
        tokio::select! {
            biased;
            _ = runtime.shutdown.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}
