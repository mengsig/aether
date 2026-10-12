//! The notifier: the push commands' behaviour, the event bus's way in, and
//! the state the delivery worker reads.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use aether_core::event::Event;
use aether_protocol::remote::{
    AttentionKind, DeviceId, PushConfig, PushSubscription, RemoteSettings,
};
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::notice::{self, Fleet, Notice, Signal};
use super::outbox::{Offered, Outbox};
use super::subscription::{self, HostPolicy};
use super::transport::Transport;
use super::vapid::VapidKey;
use super::{PushError, PushStore, StoredSubscription, delivery};
use crate::auth::DeviceStore;

/// What a notifier is built from: the static `[remote]` push settings and
/// where the VAPID key lives.
#[derive(Debug, Clone)]
pub struct NotifierConfig {
    pub instance_id: String,
    /// Let payloads carry a short preview of content.
    pub previews: bool,
    /// The VAPID `sub` claim; the first public origin when unset.
    pub subject: Option<String>,
    pub hosts: HostPolicy,
    /// The private key file, created when push is first enabled.
    pub key_path: PathBuf,
}

/// What `RemoteSettings` currently says about push.
#[derive(Debug, Default)]
struct Settings {
    enabled: bool,
    first_origin: Option<String>,
}

#[derive(Debug)]
pub(super) struct Inner {
    pub(super) config: NotifierConfig,
    pub(super) store: Arc<dyn PushStore>,
    pub(super) fleet: Arc<dyn Fleet>,
    pub(super) transport: Arc<dyn Transport>,
    /// When present, a revoked or unknown device is never sent to.
    pub(super) devices: OnceLock<Arc<dyn DeviceStore>>,
    settings: Mutex<Settings>,
    key: Mutex<Option<Arc<VapidKey>>>,
    pub(super) outbox: Mutex<Outbox>,
    /// Wakes the delivery worker when the outbox changed.
    pub(super) wake: Notify,
}

/// Sends notifications. Fed every event; decides which ones matter.
#[derive(Debug, Clone)]
pub struct Notifier {
    inner: Arc<Inner>,
}

impl Notifier {
    /// A notifier with push disabled until [`Notifier::apply_settings`] says otherwise.
    pub fn new(
        config: NotifierConfig,
        store: Arc<dyn PushStore>,
        fleet: Arc<dyn Fleet>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        assert!(
            !config.instance_id.is_empty(),
            "a daemon has an instance id"
        );
        assert!(
            config.key_path.file_name().is_some(),
            "the VAPID key path names a file"
        );
        Self {
            inner: Arc::new(Inner {
                config,
                store,
                fleet,
                transport,
                devices: OnceLock::new(),
                settings: Mutex::default(),
                key: Mutex::default(),
                outbox: Mutex::default(),
                wake: Notify::new(),
            }),
        }
    }

    /// From now on, send only to devices `devices` knows and has not
    /// revoked. The listener gives its device store here, once.
    pub fn guard_devices(&self, devices: Arc<dyn DeviceStore>) {
        let given = self.inner.devices.set(devices);
        assert!(given.is_ok(), "the device store is given once");
    }

    pub fn store(&self) -> &Arc<dyn PushStore> {
        &self.inner.store
    }

    /// Follow the runtime settings: at daemon start and on every accepted
    /// `configure_remote`. Enabling push for the first time creates the key.
    pub fn apply_settings(&self, settings: &RemoteSettings) -> Result<(), PushError> {
        if settings.push {
            self.inner.key(true)?;
        }
        *self.inner.settings.lock() = Settings {
            enabled: settings.push,
            first_origin: settings.public_origins.first().cloned(),
        };
        if !settings.push {
            self.inner.outbox.lock().clear();
        }
        Ok(())
    }

    /// What `get_push_config` answers for `device`.
    pub fn config(&self, device: Option<DeviceId>) -> Result<PushConfig, PushError> {
        let stored = match device {
            Some(device) => self.inner.store.subscription(device)?,
            None => None,
        };
        Ok(PushConfig {
            enabled: self.inner.enabled(),
            vapid_public_key: self
                .inner
                .key(false)?
                .map(|key| key.public_key().to_owned()),
            previews: self.inner.config.previews,
            subscribed: stored.is_some(),
            kinds: stored.map(|stored| stored.kinds).unwrap_or_default(),
        })
    }

    /// `subscribe_push`: store the device's one subscription, replacing any.
    pub fn subscribe(
        &self,
        device: DeviceId,
        subscription: PushSubscription,
        kinds: Vec<AttentionKind>,
    ) -> Result<PushConfig, PushError> {
        if !self.inner.enabled() {
            return Err(PushError::Disabled);
        }
        self.inner.config.hosts.check(&subscription.endpoint)?;
        subscription::recipient(&subscription)?;
        let mut wanted = Vec::with_capacity(kinds.len());
        for kind in kinds {
            if !wanted.contains(&kind) {
                wanted.push(kind);
            }
        }
        let stored = StoredSubscription {
            device,
            subscription,
            kinds: wanted,
        };
        if self.inner.store.subscription(device)?.as_ref() != Some(&stored) {
            self.inner.store.put_subscription(&stored)?;
        }
        self.config(Some(device))
    }

    /// `unsubscribe_push`: succeeds when there was nothing to remove.
    pub fn unsubscribe(&self, device: DeviceId) -> Result<PushConfig, PushError> {
        self.forget_device(device)?;
        self.config(Some(device))
    }

    /// Delete the device's subscription and what is queued for it. Revoking
    /// a device calls this.
    pub fn forget_device(&self, device: DeviceId) -> Result<(), PushError> {
        self.inner.outbox.lock().forget(device);
        self.inner.store.remove_subscription(device)
    }

    /// `test_push`: send one notification now and report what the push
    /// service said. It bypasses the queue, so a failure is not retried.
    pub async fn test(&self, device: DeviceId) -> Result<(), PushError> {
        if !self.inner.enabled() {
            return Err(PushError::Disabled);
        }
        if self.inner.store.subscription(device)?.is_none() {
            return Err(PushError::NotSubscribed);
        }
        let notice = notice::test_notice(&self.inner.config.instance_id, aether_core::time::now());
        delivery::send_now(&self.inner, device, &notice).await
    }

    /// Take one event off the bus. Never waits on the network: a notice is
    /// queued and the delivery worker sends it.
    pub fn observe(&self, event: &Event) -> Result<(), PushError> {
        match notice::classify(event) {
            Signal::Nothing => Ok(()),
            Signal::Revoked(device) => self.forget_device(device),
            Signal::Retract(approval) => {
                self.inner.outbox.lock().retract(approval);
                Ok(())
            }
            Signal::Candidate(_) if !self.inner.enabled() => Ok(()),
            Signal::Candidate(_) => {
                let config = &self.inner.config;
                let composed = notice::compose(
                    event,
                    self.inner.fleet.as_ref(),
                    &config.instance_id,
                    config.previews,
                )?;
                match composed {
                    Some(notice) => self.enqueue(notice),
                    None => Ok(()),
                }
            }
        }
    }

    /// Queue `notice` for every device that wants it and did not cause it.
    fn enqueue(&self, notice: Notice) -> Result<(), PushError> {
        let subscriptions = self.inner.store.subscriptions()?;
        let notice = Arc::new(notice);
        let now = Instant::now();
        let mut outbox = self.inner.outbox.lock();
        for stored in &subscriptions {
            if Some(stored.device) == notice.origin || !stored.wants(notice.payload.kind) {
                continue;
            }
            let offered = outbox.offer(stored.device, Arc::clone(&notice), now);
            if matches!(offered, Offered::Evicted | Offered::Full) {
                tracing::warn!(
                    device = %stored.device,
                    queued = outbox.len(),
                    "push outbox is full; an undelivered notification was dropped"
                );
            }
        }
        drop(outbox);
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Deliver what is queued until `shutdown`. One task runs this.
    pub async fn run(&self, shutdown: CancellationToken) {
        delivery::run(Arc::clone(&self.inner), shutdown).await;
    }

    /// How many notices are waiting or being sent.
    pub fn queued(&self) -> usize {
        self.inner.outbox.lock().len()
    }
}

impl Inner {
    pub(super) fn enabled(&self) -> bool {
        self.settings.lock().enabled
    }

    /// The VAPID `sub` claim: the configured contact, else the first origin.
    pub(super) fn subject(&self) -> Option<String> {
        self.config
            .subject
            .clone()
            .or_else(|| self.settings.lock().first_origin.clone())
    }

    /// The VAPID key, read from its file once. `create` makes it when it
    /// does not exist yet; without it a missing key is `None`.
    pub(super) fn key(&self, create: bool) -> Result<Option<Arc<VapidKey>>, PushError> {
        let mut cached = self.key.lock();
        if let Some(key) = cached.as_ref() {
            return Ok(Some(Arc::clone(key)));
        }
        let path = &self.config.key_path;
        let loaded = if create {
            VapidKey::load_or_create(path).map(Some)
        } else {
            VapidKey::load(path)
        };
        let key = loaded
            .map_err(|e| PushError::Key(e.to_string()))?
            .map(Arc::new);
        cached.clone_from(&key);
        Ok(key)
    }
}
