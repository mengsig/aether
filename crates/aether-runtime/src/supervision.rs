//! Bound background unwinds and cancellation without detaching a replacement worker.

use std::{future::Future, panic::AssertUnwindSafe, time::Duration};

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Restart a recreatable worker after failure, with bounded exponential backoff.
pub async fn restart<F, W>(
    name: &str,
    shutdown: CancellationToken,
    report: impl Fn(String),
    mut work: F,
) where
    F: FnMut() -> W,
    W: Future<Output = ()>,
{
    let mut backoff = Duration::from_secs(1);
    while !shutdown.is_cancelled() {
        let started = tokio::time::Instant::now();
        let outcome = attempt(&shutdown, async { work().await }).await;
        if shutdown.is_cancelled() {
            if let Err(error) = outcome {
                report(format!(
                    "Background task {name} failed during shutdown: {error}; run `aether doctor`"
                ));
            }
            return;
        }
        if started.elapsed() >= MAX_BACKOFF {
            backoff = Duration::from_secs(1);
        }
        let reason = outcome
            .err()
            .unwrap_or_else(|| "stopped unexpectedly".into());
        report(format!(
            "Background task {name} failed: {reason}; restarting in {} seconds. Run `aether doctor` and `aether diagnose` if it repeats.",
            backoff.as_secs()
        ));
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Isolate a worker whose owned state cannot safely be reconstructed after panic.
pub async fn isolate(
    name: &str,
    shutdown: CancellationToken,
    expected_exit: bool,
    report: impl Fn(String),
    work: impl Future<Output = ()>,
) {
    let outcome = attempt(&shutdown, work).await;
    let error = match outcome {
        Err(error) => Some(error),
        Ok(()) if !expected_exit && !shutdown.is_cancelled() => Some("stopped unexpectedly".into()),
        Ok(()) => None,
    };
    if let Some(error) = error {
        report(format!(
            "Background task {name} is disabled: {error}. Run `aether doctor` and `aether diagnose`, then restart with `aether stop` and `aether daemon`."
        ));
    }
}

async fn attempt(
    shutdown: &CancellationToken,
    work: impl Future<Output = ()>,
) -> Result<(), String> {
    let work = AssertUnwindSafe(work).catch_unwind();
    tokio::pin!(work);
    let result = tokio::select! {
        result = &mut work => result,
        _ = shutdown.cancelled() => {
            tokio::time::timeout(SHUTDOWN_GRACE, &mut work).await
                .map_err(|_| "did not stop within five seconds; its future was dropped".to_owned())?
        }
    };
    result.map_err(|panic| crate::turn::panic_message(panic.as_ref()))
}

#[cfg(test)]
mod tests;
