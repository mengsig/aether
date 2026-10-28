//! A database another process keeps locked past SQLite's busy timeout. The
//! turn's own writes wait it out, in order, so what the turn holds only in
//! memory (a reply and its usage) is not lost to a failed turn.

use std::time::{Duration, Instant};

use aether_core::id::AgentId;

use crate::{Result, Runtime};

/// First pause after a refused write; each refusal doubles it.
const PAUSE_FLOOR: Duration = Duration::from_millis(250);
const PAUSE_CAP: Duration = Duration::from_secs(5);

/// How long one write keeps trying, busy timeouts included, before the
/// turn gives up on it and is recovered as a restart would recover it.
const PATIENCE: Duration = Duration::from_secs(120);

/// Run `write` until it lands, waiting out a locked database for up to
/// [`PATIENCE`]. `write` must be one atomic commit: a refused attempt
/// changed nothing, so it is built again and retried. Any other error, a
/// lock that outlasts the patience, or a shutdown returns the error.
pub(super) async fn write_through_lock<T>(
    runtime: &Runtime,
    agent_id: AgentId,
    what: &'static str,
    mut write: impl FnMut() -> Result<T>,
) -> Result<T> {
    let started = Instant::now();
    let mut pause = PAUSE_FLOOR;
    let mut refused = 0u32;
    loop {
        let error = match write() {
            Err(error) if error.is_store_contended() => error,
            landed => {
                if refused > 0 && landed.is_ok() {
                    tracing::info!(agent = %agent_id, what, refused, "the database lock cleared; the write landed");
                }
                return landed;
            }
        };
        refused += 1;
        if started.elapsed() >= PATIENCE {
            tracing::error!(agent = %agent_id, what, refused, error = %error, "the database stayed locked; giving the write up");
            return Err(error);
        }
        tracing::warn!(agent = %agent_id, what, refused, "the database is locked; writing again once it clears");
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = runtime.shutdown.cancelled() => return Err(error),
        }
        pause = (pause * 2).min(PAUSE_CAP);
        debug_assert!(pause >= PAUSE_FLOOR && pause <= PAUSE_CAP);
    }
}
