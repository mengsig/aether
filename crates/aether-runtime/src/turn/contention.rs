//! A database another process keeps locked past SQLite's busy timeout. The
//! turn's own writes wait it out, in order, so what the turn holds only in
//! memory (a reply and its usage) is not lost to a failed turn.

use std::time::{Duration, Instant};

use aether_core::event::Event;
use aether_core::id::AgentId;
use aether_store::{Commit, Store};
use tokio_util::sync::CancellationToken;

use crate::{Result, Runtime};

/// First pause after a refused write; each refusal doubles it.
const PAUSE_FLOOR: Duration = Duration::from_millis(250);
const PAUSE_CAP: Duration = Duration::from_secs(5);

/// How long one write keeps trying, busy timeouts included, before the
/// turn gives up on it and is recovered as a restart would recover it.
const PATIENCE: Duration = Duration::from_secs(120);

/// The backoff one write follows while the database stays locked.
struct Patience {
    agent_id: AgentId,
    what: &'static str,
    started: Instant,
    pause: Duration,
    refused: u32,
}

impl Patience {
    fn new(agent_id: AgentId, what: &'static str) -> Self {
        Self {
            agent_id,
            what,
            started: Instant::now(),
            pause: PAUSE_FLOOR,
            refused: 0,
        }
    }

    /// The pause before the next attempt, or `None` once the patience is spent.
    fn refused(&mut self, error: &dyn std::fmt::Display) -> Option<Duration> {
        let (agent, what) = (self.agent_id, self.what);
        self.refused += 1;
        if self.started.elapsed() >= PATIENCE {
            tracing::error!(%agent, what, refused = self.refused, %error, "the database stayed locked; giving the write up");
            return None;
        }
        tracing::warn!(%agent, what, refused = self.refused, "the database is locked; writing again once it clears");
        let pause = self.pause;
        self.pause = (pause * 2).min(PAUSE_CAP);
        debug_assert!(pause >= PAUSE_FLOOR && pause <= PAUSE_CAP);
        Some(pause)
    }

    fn landed(&self) {
        if self.refused > 0 {
            tracing::info!(agent = %self.agent_id, what = self.what, refused = self.refused, "the database lock cleared; the write landed");
        }
    }
}

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
    let mut patience = Patience::new(agent_id, what);
    loop {
        let error = match write() {
            Err(error) if error.is_store_contended() => error,
            landed => {
                if landed.is_ok() {
                    patience.landed();
                }
                return landed;
            }
        };
        let Some(pause) = patience.refused(&error) else {
            return Err(error);
        };
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = runtime.shutdown.cancelled() => return Err(error),
        }
    }
}

/// [`write_through_lock`] for a commit that cannot be built again: the
/// engine path changes the turn's state as it builds one. The caller keeps
/// the commit gate throughout, so nothing lands between the attempts.
pub(crate) fn commit_through_lock(
    store: &dyn Store,
    shutdown: &CancellationToken,
    commit: Commit,
    agent_id: AgentId,
    what: &'static str,
) -> Result<Vec<Event>> {
    debug_assert!(
        !commit.is_empty(),
        "apply_held lands an empty commit itself"
    );
    let mut patience = Patience::new(agent_id, what);
    loop {
        let error = match store.commit(commit.clone()) {
            Err(error) if error.is_contended() => error,
            landed => {
                let events = landed?;
                patience.landed();
                return Ok(events);
            }
        };
        let Some(pause) = patience.refused(&error) else {
            return Err(error.into());
        };
        if shutdown.is_cancelled() {
            return Err(error.into());
        }
        std::thread::sleep(pause);
    }
}
