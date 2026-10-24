use std::{sync::Arc, time::Duration};

use crate::{Runtime, RuntimeError};

pub(crate) fn spawn(runtime: Arc<Runtime>) {
    let interval = Duration::from_secs(u64::from(
        runtime.models.config().storage.gc_interval_seconds.get(),
    ));
    let supervising = runtime.clone();
    tokio::spawn(async move {
        crate::supervision::isolate(
            "retention",
            supervising.shutdown.clone(),
            false,
            |detail| supervising.report_problem("background.retention".into(), detail),
            run(runtime, interval),
        )
        .await;
    });
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
            sweep(&sweeping, filesystem_only);
        })
        .await;
        if let Err(error) = result {
            runtime.report_problem("retention.worker".into(), format!("Retention worker failed: {error}; it will retry at the next maintenance interval. Run `aether doctor` and `aether diagnose`."));
        } else {
            runtime.clear_problem("retention.worker");
        }
        tokio::select! {
            biased;
            _ = runtime.shutdown.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

pub(super) fn sweep(runtime: &Runtime, filesystem_only: bool) {
    if runtime.shutdown.is_cancelled() {
        return;
    }
    match runtime.storage_gc(false) {
        // The sweep holding the lock does this work; a quiet skip is correct.
        Err(RuntimeError::SweepBusy) => {
            tracing::debug!("storage maintenance skipped: another sweep is running");
        }
        Ok(report) => {
            runtime.clear_problem("retention.storage");
            tracing::info!(%report, "storage maintenance finished");
        }
        Err(error) => {
            runtime.report_problem(
                "retention.storage".into(),
                format!("Storage maintenance failed: {error}; the next maintenance interval will retry. Run `aether doctor` and `aether diagnose`."),
            );
        }
    }
    // Artifact startup cleanup already ran; preserve the history recovery window
    // before the first periodic sweep.
    if filesystem_only {
        return;
    }
    match runtime.sweep_artifacts() {
        Ok(evicted) => {
            runtime.clear_problem("retention.artifacts");
            if evicted > 0 {
                tracing::info!(evicted, "retention dropped artifact bytes");
            }
        }
        Err(error) => {
            runtime.report_problem(
                "retention.artifacts".into(),
                format!("Artifact retention failed: {error}; the next maintenance interval will retry. Run `aether doctor` and `aether diagnose`."),
            );
        }
    }
    if runtime.shutdown.is_cancelled() {
        return;
    }
    match runtime.sweep_run_history_inner(aether_core::time::now(), true) {
        Ok(report) => {
            runtime.clear_problem("retention.runs");
            tracing::info!(?report, "completed-run retention sweep finished");
        }
        Err(error) => {
            runtime.report_problem(
                "retention.runs".into(),
                format!("Completed-run retention failed: {error}; the next maintenance interval will retry. Run `aether doctor` and `aether diagnose`."),
            );
        }
    }
    if runtime.shutdown.is_cancelled() {
        return;
    }
    match runtime.store.reclaim_free_pages() {
        Ok(pages) => {
            runtime.clear_problem("retention.vacuum");
            if pages > 0 {
                tracing::info!(pages, "returned free store pages to the filesystem");
            }
        }
        Err(error) => {
            runtime.report_problem(
                "retention.vacuum".into(),
                format!("Returning free store pages failed: {error}; the next maintenance interval will retry. Run `aether doctor` and `aether diagnose`."),
            );
        }
    }
}
