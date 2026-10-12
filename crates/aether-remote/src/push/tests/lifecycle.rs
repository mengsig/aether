//! Subscriptions through the commands: created, replaced, removed, revoked.

use std::collections::HashMap;
use std::sync::Arc;

use aether_core::event::{Actor, EventKind};
use aether_protocol::remote::{AttentionKind, DeviceId, DeviceRole, PushConfig, PushSubscription};
use parking_lot::Mutex;

use super::harness::{ENDPOINT, Harness, SETTLED, failed, settings};
use crate::auth::{AuthError, DeviceRecord, DeviceStore};
use crate::push::notice::DEVICE_REVOKED_EVENT;
use crate::push::testing::{RecordingTransport, UserAgent};
use crate::push::{PushError, PushStore};

#[tokio::test(start_paused = true)]
async fn push_is_off_until_enabled_and_enabling_creates_the_key_once() {
    let transport = Arc::new(RecordingTransport::default());
    let harness = Harness::with_transport(false, transport.clone(), transport);
    let device = DeviceId::new();
    let agent = UserAgent::generate();
    let key_path = harness.dir.path().join("push-vapid.key");

    assert_eq!(
        harness.notifier.config(Some(device)).expect("config"),
        PushConfig::default()
    );
    assert!(!key_path.exists(), "no key before push was ever enabled");
    let refused = harness
        .notifier
        .subscribe(device, agent.subscription(ENDPOINT), Vec::new());
    assert_eq!(refused, Err(PushError::Disabled));
    assert_eq!(
        harness.notifier.test(device).await,
        Err(PushError::Disabled)
    );

    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    let enabled = harness.notifier.config(None).expect("config");
    let key = enabled
        .vapid_public_key
        .clone()
        .expect("a key once enabled");
    assert!(enabled.enabled && !enabled.subscribed);
    assert!(key_path.exists());

    harness
        .notifier
        .apply_settings(&settings(false))
        .expect("disabling");
    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling again");
    let again = harness.notifier.config(None).expect("config");
    assert_eq!(
        again.vapid_public_key,
        Some(key),
        "the key outlives a disable"
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_holds_one_subscription_that_a_new_one_replaces() {
    let harness = Harness::start(true);
    let device = DeviceId::new();
    let first = UserAgent::generate();
    let kinds = vec![
        AttentionKind::Approval,
        AttentionKind::Question,
        AttentionKind::Approval,
    ];

    let config = harness
        .notifier
        .subscribe(device, first.subscription(ENDPOINT), kinds.clone())
        .expect("subscribing");
    assert!(config.enabled && config.subscribed && config.previews);
    assert_eq!(
        config.kinds,
        [AttentionKind::Approval, AttentionKind::Question]
    );
    assert_eq!(harness.store.writes(), 1);

    harness
        .notifier
        .subscribe(device, first.subscription(ENDPOINT), kinds)
        .expect("the same again");
    assert_eq!(
        harness.store.writes(),
        1,
        "subscribing the same again changes nothing"
    );

    let second = UserAgent::generate();
    let moved = "https://updates.push.services.mozilla.com/wpush/v2/xyz";
    let config = harness
        .notifier
        .subscribe(device, second.subscription(moved), Vec::new())
        .expect("replacing");
    assert!(config.kinds.is_empty());
    let stored = harness.store.subscriptions().expect("listing");
    assert_eq!(stored.len(), 1, "at most one subscription per device");
    assert_eq!(stored[0].subscription, second.subscription(moved));

    let other = DeviceId::new();
    assert!(
        !harness
            .notifier
            .config(Some(other))
            .expect("config")
            .subscribed
    );
    assert!(!harness.notifier.config(None).expect("config").subscribed);
}

#[tokio::test(start_paused = true)]
async fn unsubscribing_removes_it_and_succeeds_when_there_is_none() {
    let harness = Harness::start(false);
    let (device, _agent) = harness.subscribe();
    let root = harness.agent("root");
    harness.emit(root, failed());

    let config = harness.notifier.unsubscribe(device).expect("unsubscribing");
    assert!(config.enabled && !config.subscribed);
    assert_eq!(harness.store.subscriptions().expect("listing"), []);
    assert_eq!(
        harness.notifier.queued(),
        0,
        "what was queued for it is dropped"
    );
    harness
        .notifier
        .unsubscribe(device)
        .expect("nothing to remove is not an error");

    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.transport.sent(), 0);
    assert_eq!(
        harness.notifier.test(device).await,
        Err(PushError::NotSubscribed)
    );
}

#[tokio::test(start_paused = true)]
async fn a_subscription_is_refused_unless_it_is_https_to_a_push_service_with_real_keys() {
    let harness = Harness::start(false);
    let device = DeviceId::new();
    let agent = UserAgent::generate();
    let endpoints = [
        (
            "http://fcm.googleapis.com/fcm/send/abc",
            PushError::BadEndpoint,
        ),
        ("https://127.0.0.1/push", PushError::BadEndpoint),
        (
            "https://user:pw@fcm.googleapis.com/x",
            PushError::BadEndpoint,
        ),
        ("https://fcm.googleapis.com:8443/x", PushError::BadEndpoint),
        (
            "https://router.lan/hook",
            PushError::HostNotAllowed {
                host: "router.lan".into(),
            },
        ),
        (
            "https://example.com/hook",
            PushError::HostNotAllowed {
                host: "example.com".into(),
            },
        ),
    ];
    for (endpoint, expected) in endpoints {
        let refused = harness
            .notifier
            .subscribe(device, agent.subscription(endpoint), Vec::new());
        assert_eq!(refused, Err(expected), "{endpoint}");
    }
    let bad_keys = PushSubscription {
        p256dh: "AAAA".into(),
        ..agent.subscription(ENDPOINT)
    };
    assert_eq!(
        harness.notifier.subscribe(device, bad_keys, Vec::new()),
        Err(PushError::BadKeys)
    );
    assert_eq!(harness.store.writes(), 0, "nothing refused is stored");
}

#[tokio::test(start_paused = true)]
async fn a_stored_endpoint_the_policy_no_longer_allows_is_never_contacted() {
    let harness = Harness::start(false);
    let (device, agent) = harness.subscribe();
    let mut stored = harness
        .store
        .subscription(device)
        .expect("reading")
        .expect("stored");
    stored.subscription = agent.subscription("https://internal.example/hook");
    harness
        .store
        .put_subscription(&stored)
        .expect("writing behind the notifier's back");

    harness.emit(harness.agent("root"), failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.transport.sent(), 0);
    assert_eq!(
        harness.notifier.queued(),
        0,
        "the notice is dropped, not retried"
    );
    assert!(matches!(
        harness.notifier.test(device).await,
        Err(PushError::Service(_))
    ));
    assert_eq!(harness.transport.sent(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_device_hears_only_the_kinds_it_asked_for() {
    let harness = Harness::start(false);
    let device = DeviceId::new();
    let agent = UserAgent::generate();
    harness
        .notifier
        .subscribe(
            device,
            agent.subscription(ENDPOINT),
            vec![AttentionKind::Approval],
        )
        .expect("subscribing");
    let root = harness.agent("root");

    harness.emit(root, failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.transport.sent(), 0, "it did not ask about failures");

    let (approval, _) = harness.ask_approval(root);
    tokio::time::sleep(SETTLED).await;
    let received = harness.received(&agent);
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].approval, Some(approval));

    harness
        .notifier
        .test(device)
        .await
        .expect("a test always goes out");
    assert_eq!(harness.received(&agent)[1].kind, AttentionKind::Test);
}

#[tokio::test(start_paused = true)]
async fn revoking_a_device_deletes_its_subscription_and_what_was_queued() {
    let harness = Harness::start(false);
    let (revoked, _) = harness.subscribe();
    let (kept, kept_agent) = harness.subscribe();
    let root = harness.agent("root");
    harness.emit(root, failed());
    assert_eq!(harness.notifier.queued(), 2);

    let payload =
        serde_json::json!({"device": revoked.to_string(), "name": "phone", "reason": "lost"});
    let audit = EventKind::Custom {
        name: DEVICE_REVOKED_EVENT.into(),
        payload,
    };
    harness
        .notifier
        .observe(&harness.event(Actor::user(), root, audit))
        .expect("observing");

    let left: Vec<DeviceId> = harness
        .store
        .subscriptions()
        .expect("listing")
        .iter()
        .map(|stored| stored.device)
        .collect();
    assert_eq!(left, [kept]);
    tokio::time::sleep(SETTLED).await;
    assert_eq!(
        harness.transport.sent(),
        1,
        "only the device that is still paired"
    );
    assert_eq!(harness.received(&kept_agent).len(), 1);

    harness
        .notifier
        .forget_device(kept)
        .expect("the direct way");
    assert_eq!(harness.store.subscriptions().expect("listing"), []);
}

#[tokio::test(start_paused = true)]
async fn revocation_is_honoured_even_while_push_is_disabled() {
    let harness = Harness::start(false);
    let (device, _) = harness.subscribe();
    harness
        .notifier
        .apply_settings(&settings(false))
        .expect("disabling");

    let payload = serde_json::json!({"device": device.to_string()});
    let audit = EventKind::Custom {
        name: DEVICE_REVOKED_EVENT.into(),
        payload,
    };
    let root = harness.agent("root");
    harness
        .notifier
        .observe(&harness.event(Actor::user(), root, audit))
        .expect("observing");
    assert_eq!(harness.store.subscriptions().expect("listing"), []);
}

/// Device records as the daemon's device table would answer.
#[derive(Debug, Default)]
struct Devices {
    records: Mutex<HashMap<DeviceId, DeviceRecord>>,
}

impl Devices {
    fn enrol(&self, id: DeviceId, revoked: bool) {
        let now = aether_core::time::now();
        let record = DeviceRecord {
            id,
            name: "phone".into(),
            role: DeviceRole::Operator,
            public_key: vec![4; 65],
            created_at: now,
            last_seen_at: None,
            revoked_at: revoked.then_some(now),
        };
        self.records.lock().insert(id, record);
    }
}

impl DeviceStore for Devices {
    fn device(&self, id: DeviceId) -> Result<Option<DeviceRecord>, AuthError> {
        Ok(self.records.lock().get(&id).cloned())
    }

    fn devices(&self, _include_revoked: bool) -> Result<Vec<DeviceRecord>, AuthError> {
        Ok(self.records.lock().values().cloned().collect())
    }

    fn accepted(&self, device: &DeviceRecord, _origin: &str) -> Result<(), AuthError> {
        unreachable!(
            "push never authenticates a device, and was asked to for {}",
            device.id
        )
    }

    fn refused(&self, device: DeviceId, _origin: &str, _reason: &AuthError) {
        unreachable!("push never authenticates a device, and was asked to for {device}")
    }
}

#[tokio::test(start_paused = true)]
async fn with_a_device_table_a_revoked_or_unknown_device_is_never_sent_to() {
    use crate::push::testing::{MemoryPushStore, StaticFleet};
    use crate::push::{AgentFacts, Notifier};

    let dir = tempfile::tempdir().expect("a directory");
    let store = Arc::new(MemoryPushStore::default());
    let fleet = Arc::new(StaticFleet::default());
    let transport = Arc::new(RecordingTransport::default());
    let devices = Arc::new(Devices::default());
    let config = super::harness::config(&dir, false);
    let notifier = Notifier::new(config, store.clone(), fleet.clone(), transport.clone());
    notifier.guard_devices(devices.clone());
    notifier.apply_settings(&settings(true)).expect("enabling");
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (worker, stop) = (notifier.clone(), shutdown.clone());
    tokio::spawn(async move { worker.run(stop).await });

    let (paired, revoked, unknown) = (DeviceId::new(), DeviceId::new(), DeviceId::new());
    devices.enrol(paired, false);
    devices.enrol(revoked, true);
    for device in [paired, revoked, unknown] {
        let subscription = UserAgent::generate().subscription(ENDPOINT);
        notifier
            .subscribe(device, subscription, Vec::new())
            .expect("subscribing");
    }
    let agent = aether_core::id::AgentId::new();
    fleet.set_agent(
        agent,
        AgentFacts {
            name: "root".into(),
            root: true,
            block: None,
        },
    );
    let draft = aether_core::event::EventDraft::new(Actor::agent(agent), failed()).agent(agent);
    let event = aether_core::event::Event::from_draft(
        draft,
        aether_core::id::EventId::new(),
        1,
        aether_core::time::now(),
    );
    notifier.observe(&event).expect("observing");
    tokio::time::sleep(SETTLED).await;

    assert_eq!(transport.sent(), 1, "only the device still enrolled");
    let left: Vec<DeviceId> = store
        .subscriptions()
        .expect("listing")
        .iter()
        .map(|s| s.device)
        .collect();
    assert_eq!(left, [paired], "the others lost their subscriptions");
    assert!(matches!(
        notifier.test(revoked).await,
        Err(PushError::NotSubscribed)
    ));
    shutdown.cancel();
}
