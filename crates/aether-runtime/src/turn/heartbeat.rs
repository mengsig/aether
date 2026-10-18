//! The turn's lease renewals. They stop the moment the turn lets go of its
//! heartbeat, and renewals the disk refused are reported, never dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::lease::Lease;
use aether_core::time::now;
use aether_store::{Commit, Mutation};

use super::StoreOutage;
use crate::Runtime;

/// A refused renewal is retried this soon, so the lease is good again soon
/// after the disk takes writes again.
const RENEWAL_RETRY: Duration = Duration::from_secs(5);

/// Renews the turn's lease while the turn holds it, and no longer: dropped
/// however the turn ends, an unwind included, it stops renewing.
pub(super) struct Heartbeat {
    task: tokio::task::JoinHandle<()>,
    stopped: Arc<AtomicBool>,
    /// Renewals refused since the last one landed. Locked across each
    /// attempt, so `stop` sees its outcome and nothing is recorded twice.
    refused: Arc<parking_lot::Mutex<Option<StoreOutage>>>,
}

impl Heartbeat {
    /// Stop renewing; returns the refusals no renewal has recorded yet.
    pub(super) fn stop(self) -> Option<StoreOutage> {
        self.stopped.store(true, Ordering::SeqCst);
        self.refused.lock().take()
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        // Read under the commit hold before each renewal, so none can land
        // after the turn's next commit - the one clearing the lease.
        self.stopped.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

pub(super) fn spawn_heartbeat(runtime: Arc<Runtime>, mut lease: Lease, agent: &Agent) -> Heartbeat {
    let ttl = runtime.config.lease_ttl;
    let (parent, run) = (agent.parent, agent.run);
    let stopped = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(parking_lot::Mutex::new(None::<StoreOutage>));
    let task = tokio::spawn({
        let (stopped, refused) = (stopped.clone(), refused.clone());
        async move {
            loop {
                let pause = match *refused.lock() {
                    Some(_) => RENEWAL_RETRY.min(ttl / 2),
                    None => ttl / 2,
                };
                tokio::time::sleep(pause).await;
                lease.renew(now(), ttl);
                let draft = EventDraft::new(
                    Actor::Scheduler,
                    EventKind::LeaseRenewed {
                        expires_at: lease.expires_at,
                    },
                )
                .agent(lease.agent_id)
                .run(run);
                let mut commit = Commit::new()
                    .event(draft)
                    .mutate(Mutation::SetLease(lease.clone()));
                let mut refused = refused.lock();
                // Carried by the renewal that lands, so it is recorded once.
                if let Some(outage) = refused.as_ref() {
                    commit.push_event(outage.draft(lease.agent_id, parent, run));
                }
                let hold = runtime.hold();
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                match runtime.apply_held(hold, commit) {
                    Ok(_) => {
                        if refused.take().is_some() {
                            tracing::warn!(agent = %lease.agent_id, "lease renewal landed again");
                        }
                    }
                    Err(e) => match refused.as_mut() {
                        Some(outage) => outage.refused_again(),
                        None => {
                            tracing::error!(agent = %lease.agent_id, error = %e, "lease renewal failed");
                            *refused = Some(StoreOutage::began(&e, "lease renewal"));
                        }
                    },
                }
            }
        }
    });
    Heartbeat {
        task,
        stopped,
        refused,
    }
}
