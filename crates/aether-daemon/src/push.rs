//! Web Push for this daemon: where subscriptions are kept, what the notifier may ask about the
//! fleet, and the task that feeds it the event bus. docs/remote.md says what is sent.

use std::path::Path;
use std::sync::Arc;

use aether_core::agent::{AgentStatus, BlockReason};
use aether_core::event::Event;
use aether_core::id::{AgentId, ApprovalId};
use aether_core::tool::ApprovalStatus;
use aether_model::config::{ConfigError, RemoteSection};
use aether_protocol::remote::{AttentionKind, DeviceId, PushSubscription};
use aether_remote::push::{
    AgentFacts, Fleet, HostPolicy, Notifier, NotifierConfig, OperatorBlock, PushError, PushStore,
    StoredSubscription, Transport,
};
use aether_runtime::{BusMessage, Runtime};
use aether_store::{EventFilter, PushSubscriptionRow, SqliteStore, Store};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// The VAPID private key sits beside the store file, under this extension.
const KEY_EXTENSION: &str = "push-vapid-key";
/// Events read per store query while catching up after a lag.
const REPLAY_PAGE: usize = 500;

/// The push half of `[remote]`. Refused at start, like the rest of the stanza.
pub(crate) fn configured(
    section: &RemoteSection,
    instance_id: &str,
    store_path: &Path,
) -> anyhow::Result<NotifierConfig> {
    let bad = |reason: String| ConfigError {
        path: "config.toml".into(),
        reason,
    };
    let hosts = HostPolicy::new(&section.push_hosts)
        .map_err(|why| bad(format!("[remote] push_hosts: {why}")))?;
    Ok(NotifierConfig {
        instance_id: instance_id.to_owned(),
        previews: section.push_previews,
        subject: section.push_subject().map_err(bad)?.map(str::to_owned),
        hosts,
        key_path: store_path.with_extension(KEY_EXTENSION),
    })
}

/// The daemon's push service. Every clone is the same service.
#[derive(Debug, Clone)]
pub struct Push {
    notifier: Notifier,
}

impl Push {
    /// Build the notifier and start its two tasks on `tasks`: one follows
    /// the event bus, one delivers. Both end with `shutdown`.
    pub(crate) fn start(
        config: NotifierConfig,
        subscriptions: Arc<SqliteStore>,
        transport: Arc<dyn Transport>,
        runtime: &Runtime,
        tasks: &mut JoinSet<()>,
        shutdown: &CancellationToken,
    ) -> anyhow::Result<Self> {
        let store = runtime.store().clone();
        let notifier = Notifier::new(
            config,
            Arc::new(SubscriptionTable(subscriptions)),
            Arc::new(StoreFleet(store.clone())),
            transport,
        );
        // Subscribed before the cursor is read, so no event falls between.
        let events = runtime.subscribe();
        let seen = store.last_seq()?;
        tasks.spawn(follow(
            notifier.clone(),
            events,
            store,
            seen,
            shutdown.clone(),
        ));
        let delivering = notifier.clone();
        let stop = shutdown.clone();
        tasks.spawn(async move { delivering.run(stop).await });
        Ok(Self { notifier })
    }

    /// Where the push commands, `configure_remote` and `revoke_device` act.
    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }
}

/// Hand the notifier every event until `shutdown`. A receiver that lagged
/// catches up from the store, so a burst costs no notification.
async fn follow(
    notifier: Notifier,
    mut events: Receiver<BusMessage>,
    store: Arc<dyn Store>,
    mut seen: u64,
    shutdown: CancellationToken,
) {
    loop {
        let message = tokio::select! {
            () = shutdown.cancelled() => return,
            message = events.recv() => message,
        };
        match message {
            Ok(BusMessage::Event(event)) => observe(&notifier, &event, &mut seen),
            Ok(BusMessage::Delta { .. }) => {}
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(
                    missed,
                    "push lagged behind the event bus; replaying from the store"
                );
                catch_up(&notifier, store.as_ref(), &mut seen, &shutdown).await;
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// Show the notifier one event, once. Its failure costs that notification
/// and is logged: the events after it must still be seen.
fn observe(notifier: &Notifier, event: &Event, seen: &mut u64) {
    if event.seq <= *seen {
        return;
    }
    *seen = event.seq;
    if let Err(error) = notifier.observe(event) {
        tracing::warn!(seq = event.seq, %error, "an event was not turned into a push notification");
    }
}

/// Replay what the bus dropped, a page at a time, yielding between pages.
async fn catch_up(
    notifier: &Notifier,
    store: &dyn Store,
    seen: &mut u64,
    shutdown: &CancellationToken,
) {
    while !shutdown.is_cancelled() {
        let page = match store.events(*seen, REPLAY_PAGE, &EventFilter::all()) {
            Ok(page) => page,
            Err(error) => {
                tracing::error!(after = *seen, %error, "push could not replay missed events");
                return;
            }
        };
        let Some(last) = page.last() else {
            return;
        };
        assert!(page.len() <= REPLAY_PAGE, "the store honours the page size");
        assert!(last.seq > *seen, "a replayed page moves the cursor on");
        for event in &page {
            observe(notifier, event, seen);
        }
        tokio::task::yield_now().await;
    }
}

/// What a `push_subscriptions` row holds beside its device.
#[derive(Serialize, Deserialize)]
struct SubscriptionBody {
    subscription: PushSubscription,
    kinds: Vec<AttentionKind>,
}

/// The store's subscription table, as the notifier reads and writes it.
#[derive(Debug)]
struct SubscriptionTable(Arc<SqliteStore>);

fn store_error(error: impl std::fmt::Display) -> PushError {
    PushError::Store(error.to_string())
}

/// The subscription in `row`. An unreadable row is logged and skipped so it cannot silence push for
/// other devices; its owner replaces it by subscribing again.
fn readable(row: PushSubscriptionRow) -> Option<StoredSubscription> {
    let device = row.device;
    match serde_json::from_value::<SubscriptionBody>(row.body) {
        Ok(body) => Some(StoredSubscription {
            device,
            subscription: body.subscription,
            kinds: body.kinds,
        }),
        Err(error) => {
            tracing::warn!(%device, %error, "skipping an unreadable push subscription");
            None
        }
    }
}

/// What `stored` is in its row, beside the device.
fn encode(stored: &StoredSubscription) -> Result<serde_json::Value, PushError> {
    let body = SubscriptionBody {
        subscription: stored.subscription.clone(),
        kinds: stored.kinds.clone(),
    };
    serde_json::to_value(&body).map_err(store_error)
}

impl PushStore for SubscriptionTable {
    fn subscription(&self, device: DeviceId) -> Result<Option<StoredSubscription>, PushError> {
        let row = self.0.push_subscription(device).map_err(store_error)?;
        Ok(row.and_then(readable))
    }

    fn subscriptions(&self) -> Result<Vec<StoredSubscription>, PushError> {
        let rows = self.0.push_subscriptions().map_err(store_error)?;
        Ok(rows.into_iter().filter_map(readable).collect())
    }

    fn put_subscription(&self, stored: &StoredSubscription) -> Result<(), PushError> {
        let row = PushSubscriptionRow {
            device: stored.device,
            updated_at: aether_core::time::now(),
            body: encode(stored)?,
        };
        self.0.put_push_subscription(&row).map_err(store_error)
    }

    fn remove_subscription(&self, device: DeviceId) -> Result<(), PushError> {
        let _existed = self
            .0
            .remove_push_subscription(device)
            .map_err(store_error)?;
        Ok(())
    }

    fn remove_exact_subscription(&self, dead: &StoredSubscription) -> Result<bool, PushError> {
        self.0
            .remove_push_subscription_if(dead.device, &encode(dead)?)
            .map_err(store_error)
    }
}

/// The runtime's store, answering what the notifier asks about the fleet.
struct StoreFleet(Arc<dyn Store>);

impl std::fmt::Debug for StoreFleet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreFleet")
    }
}

/// A block only the operator's attention helps; the others clear themselves.
fn operator_block(status: &AgentStatus) -> Option<OperatorBlock> {
    let AgentStatus::Blocked { reason } = status else {
        return None;
    };
    match reason {
        BlockReason::Inspection { .. } => Some(OperatorBlock::Inspection),
        BlockReason::Exhausted { model, probes, .. } => Some(OperatorBlock::Exhausted {
            provider: model.provider.clone(),
            first: *probes == 0,
        }),
        BlockReason::Dependency { .. } | BlockReason::Resource { .. } => None,
    }
}

impl Fleet for StoreFleet {
    fn agent(&self, agent: AgentId) -> Result<Option<AgentFacts>, PushError> {
        let Some(found) = self.0.agent(agent).map_err(store_error)? else {
            return Ok(None);
        };
        assert_eq!(found.id, agent, "the store answers for the agent asked for");
        Ok(Some(AgentFacts {
            root: found.parent.is_none(),
            block: operator_block(&found.status),
            name: found.name,
        }))
    }

    fn approval_pending(&self, approval: ApprovalId) -> Result<bool, PushError> {
        let request = self.0.approval(approval).map_err(store_error)?;
        Ok(request.is_some_and(|request| request.status == ApprovalStatus::Pending))
    }
}

#[cfg(test)]
pub(crate) mod tests;
