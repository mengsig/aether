use std::time::Duration;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::model::ModelRef;
use aether_protocol::remote::RemoteSettings;
use aether_remote::push::notice::{DEVICE_REVOKED_EVENT, Urgency};
use aether_remote::push::testing::{RecordingTransport, UserAgent, verify_authorization};
use aether_remote::push::{PushRequest, PushResponse};
use aether_runtime::{CreateAgentSpec, RuntimeConfig};
use aether_store::Commit;

use super::*;
use crate::Daemon;

pub(crate) const ENDPOINT: &str = "https://fcm.googleapis.com/fcm/send/abc123";
const ORIGIN: &str = "https://box.example.ts.net";

/// A daemon over an in-memory store, its push service on a recording transport.
pub(crate) struct Fixture {
    pub daemon: Daemon,
    pub sqlite: Arc<SqliteStore>,
    pub transport: Arc<RecordingTransport>,
    shutdown: CancellationToken,
    _tasks: JoinSet<()>,
    _dirs: [tempfile::TempDir; 2],
}

/// Poll until `ready` answers. Real time: the store and the worker are live.
pub(crate) async fn eventually<T>(what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

impl Fixture {
    pub(crate) async fn start() -> Self {
        let config_dir = tempfile::tempdir().expect("config dir");
        let data_dir = tempfile::tempdir().expect("data dir");
        let sqlite = Arc::new(SqliteStore::open_in_memory().expect("store"));
        let models = Arc::new(
            aether_model::Models::load(Some(config_dir.path().to_path_buf())).expect("models"),
        );
        let runtime = Runtime::start_with_engines(
            RuntimeConfig {
                holder: "test".into(),
                default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                ..Default::default()
            },
            sqlite.clone(),
            models.clone(),
            Arc::new(aether_tools::ToolRegistry::with_builtins()),
            None,
            Default::default(),
        )
        .await
        .expect("runtime");
        let transport = Arc::new(RecordingTransport::default());
        let shutdown = CancellationToken::new();
        let mut tasks = JoinSet::new();
        let store_path = data_dir.path().join("aether.db");
        let config = configured(&RemoteSection::default(), "inst", &store_path).expect("config");
        let push = Push::start(
            config,
            sqlite.clone(),
            transport.clone(),
            &runtime,
            &mut tasks,
            &shutdown,
        )
        .expect("push");
        let daemon = Daemon {
            runtime,
            models,
            instance_id: "inst".into(),
            version: "test".into(),
            push: Some(push),
            remote: Default::default(),
        };
        Self {
            daemon,
            sqlite,
            transport,
            shutdown,
            _tasks: tasks,
            _dirs: [config_dir, data_dir],
        }
    }

    pub(crate) fn notifier(&self) -> &Notifier {
        self.daemon.push.as_ref().expect("push").notifier()
    }

    pub(crate) fn set_push(&self, push: bool) {
        let settings = RemoteSettings {
            enabled: true,
            pairing: false,
            public_origins: vec![ORIGIN.into()],
            push,
        };
        self.notifier().apply_settings(&settings).expect("settings");
    }

    /// Enable push and subscribe a fresh device to every kind.
    pub(crate) fn subscribed_device(&self) -> (DeviceId, UserAgent) {
        self.set_push(true);
        let device = DeviceId::new();
        let user_agent = UserAgent::generate();
        self.notifier()
            .subscribe(device, user_agent.subscription(ENDPOINT), Vec::new())
            .expect("subscribed");
        (device, user_agent)
    }

    fn agent(&self, name: &str) -> AgentId {
        let spec = CreateAgentSpec {
            role: "worker".into(),
            name: Some(name.into()),
            start: false,
            ..Default::default()
        };
        let (agent, _) = self
            .daemon
            .runtime
            .create_agent(spec, Actor::user())
            .expect("agent");
        agent.id
    }

    /// Commit a question through the runtime, which publishes it on the bus.
    fn ask(&self, agent: AgentId, question: &str) -> u64 {
        let kind = EventKind::QuestionAsked {
            question: question.into(),
        };
        let run = self
            .daemon
            .runtime
            .store()
            .agent(agent)
            .unwrap()
            .unwrap()
            .run;
        let draft = EventDraft::new(Actor::agent(agent), kind)
            .agent(agent)
            .run(run);
        self.daemon
            .runtime
            .apply(Commit::new().event(draft))
            .expect("question")[0]
            .seq
    }

    /// Record the audit event `revoke_device` writes for `device`.
    fn revoke(&self, device: DeviceId) {
        let revoked = EventKind::Custom {
            name: DEVICE_REVOKED_EVENT.into(),
            payload: serde_json::json!({"device": device.to_string(), "name": "phone", "reason": "lost"}),
        };
        let commit = Commit::new().event(EventDraft::new(Actor::user(), revoked));
        self.daemon.runtime.apply(commit).expect("audit event");
    }

    /// Wait for the store to hold no subscription for `device`.
    pub(crate) async fn unsubscribed(&self, device: DeviceId) {
        eventually("the subscription to go", || {
            let gone = self
                .sqlite
                .push_subscription(device)
                .expect("store")
                .is_none();
            gone.then_some(())
        })
        .await;
    }

    async fn requests(&self, count: usize) -> Vec<PushRequest> {
        eventually("a push request", || {
            let requests = self.transport.requests();
            (requests.len() >= count).then_some(requests)
        })
        .await
    }

    pub(crate) async fn stop(self) {
        self.shutdown.cancel();
        self.daemon.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_question_on_the_bus_reaches_the_subscribed_device() {
    let fixture = Fixture::start().await;
    let (_device, user_agent) = fixture.subscribed_device();
    let agent = fixture.agent("builder");
    let seq = fixture.ask(agent, "Delete /srv/secret-project?");

    let requests = fixture.requests(1).await;
    let request = &requests[0];
    assert_eq!(request.endpoint.as_str(), ENDPOINT);
    assert_eq!(request.ttl, 3600);
    assert_eq!(request.urgency, Urgency::High);
    let token = verify_authorization(&request.authorization).expect("a valid VAPID token");
    assert_eq!(token.claims["aud"], "https://fcm.googleapis.com");
    assert_eq!(
        token.claims["sub"], ORIGIN,
        "the first public origin stands in for a contact"
    );

    let payload = user_agent.payload(&request.body);
    assert_eq!(payload.kind, AttentionKind::Question);
    assert_eq!(payload.instance_id, "inst");
    assert_eq!(payload.seq, seq);
    assert_eq!(payload.question_seq, Some(seq));
    assert_eq!(payload.agent, Some(agent));
    assert_eq!(payload.agent_name.as_deref(), Some("builder"));
    assert_eq!(payload.url, format!("#attention?item=question-{seq}"));
    assert_eq!(payload.preview, None, "previews are off by default");
    fixture.stop().await;
}

#[tokio::test]
async fn nothing_is_sent_for_what_happened_while_push_was_disabled() {
    let fixture = Fixture::start().await;
    let (_device, user_agent) = fixture.subscribed_device();
    let (marker, _) = fixture.subscribed_device();
    let quiet = fixture.agent("quiet");
    let heard = fixture.agent("heard");

    fixture.set_push(false);
    fixture.ask(quiet, "Anyone there?");
    // The follower takes events in order: once the marker's revocation took
    // effect, the question before it was seen, and dropped.
    fixture.revoke(marker);
    fixture.unsubscribed(marker).await;
    assert_eq!(fixture.notifier().queued(), 0);

    fixture.set_push(true);
    fixture.ask(heard, "Now?");
    let requests = fixture.requests(1).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(user_agent.payload(&requests[0].body).agent, Some(heard));
    fixture.stop().await;
}

#[tokio::test]
async fn a_revocation_on_the_bus_deletes_the_subscription() {
    let fixture = Fixture::start().await;
    let (device, _user_agent) = fixture.subscribed_device();
    let (other, _) = fixture.subscribed_device();
    fixture.revoke(device);
    fixture.unsubscribed(device).await;
    assert!(
        fixture
            .sqlite
            .push_subscription(other)
            .expect("store")
            .is_some()
    );

    fixture.ask(fixture.agent("builder"), "Proceed?");
    let requests = fixture.requests(1).await;
    assert_eq!(
        requests.len(),
        1,
        "only the device that is still paired is told"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_subscription_the_push_service_calls_gone_is_removed() {
    let fixture = Fixture::start().await;
    let (device, _user_agent) = fixture.subscribed_device();
    fixture.transport.answer(410);
    fixture.ask(fixture.agent("builder"), "Proceed?");

    fixture.unsubscribed(device).await;
    assert_eq!(
        fixture.transport.sent(),
        1,
        "a gone subscription is not retried"
    );
    assert_eq!(fixture.notifier().queued(), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn a_failing_push_service_keeps_the_subscription_and_retries() {
    let fixture = Fixture::start().await;
    let (device, _user_agent) = fixture.subscribed_device();
    fixture.transport.answer_with(Ok(PushResponse {
        status: 503,
        retry_after: None,
    }));
    fixture.ask(fixture.agent("builder"), "Proceed?");

    fixture.requests(1).await;
    eventually("the failed send to be put back", || {
        (fixture.notifier().queued() == 1).then_some(())
    })
    .await;
    assert!(
        fixture
            .sqlite
            .push_subscription(device)
            .expect("store")
            .is_some()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn events_the_bus_dropped_are_replayed_from_the_store() {
    let fixture = Fixture::start().await;
    let (_device, user_agent) = fixture.subscribed_device();
    let agent = fixture.agent("builder");
    let store = fixture.daemon.runtime.store().clone();
    let mut seen = store.last_seq().expect("cursor");
    // Committed behind the bus, as if the receiver had lagged past them.
    let mut last = 0;
    for question in ["First?", "Second?"] {
        let kind = EventKind::QuestionAsked {
            question: question.into(),
        };
        let run = store.agent(agent).unwrap().unwrap().run;
        let draft = EventDraft::new(Actor::agent(agent), kind)
            .agent(agent)
            .run(run);
        last = store.commit(Commit::new().event(draft)).expect("commit")[0].seq;
    }

    let shutdown = CancellationToken::new();
    catch_up(fixture.notifier(), store.as_ref(), &mut seen, &shutdown).await;
    assert_eq!(seen, last);

    let requests = fixture.requests(1).await;
    let payload = user_agent.payload(&requests[0].body);
    assert_eq!(
        payload.question_seq,
        Some(last),
        "the burst became one notification"
    );

    let before = fixture.notifier().queued();
    observe(
        fixture.notifier(),
        &store.event_by_seq(last).expect("store").expect("event"),
        &mut seen,
    );
    assert_eq!(
        fixture.notifier().queued(),
        before,
        "an event already seen is not seen twice"
    );
    fixture.stop().await;
}

#[test]
fn the_push_stanza_is_checked_and_the_key_sits_beside_the_store() {
    let store_path = Path::new("/var/lib/aether/aether.db");
    let section = RemoteSection {
        push_previews: true,
        push_subject: "mailto:ops@example.com".into(),
        push_hosts: vec!["push.example.org".into(), "*.push.example.net".into()],
        ..Default::default()
    };
    let config = configured(&section, "inst", store_path).expect("a valid stanza");
    assert_eq!(
        config.key_path,
        Path::new("/var/lib/aether/aether.push-vapid-key")
    );
    assert!(config.previews);
    assert_eq!(config.subject.as_deref(), Some("mailto:ops@example.com"));
    assert!(config.hosts.allows("push.example.org"));
    assert!(config.hosts.allows("eu.push.example.net"));
    assert!(!config.hosts.allows("example.org"));

    for bad in ["localhost", "https://push.example.org", "*", "10.0.0.1"] {
        let section = RemoteSection {
            push_hosts: vec![bad.into()],
            ..Default::default()
        };
        let refused = configured(&section, "inst", store_path).expect_err(bad);
        assert!(
            refused.to_string().contains("[remote] push_hosts"),
            "{refused}"
        );
    }
}

#[test]
fn only_blocks_the_operator_clears_are_reported() {
    use aether_core::agent::ExhaustionClass;
    use aether_core::id::TaskId;

    let exhausted = |probes| AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            class: ExhaustionClass::UsageLimit,
            since: aether_core::time::now(),
            reported_reset: None,
            next_probe: aether_core::time::now(),
            probes,
            detail: "limit reached".into(),
        },
    };
    let first = OperatorBlock::Exhausted {
        provider: "anthropic".into(),
        first: true,
    };
    let again = OperatorBlock::Exhausted {
        provider: "anthropic".into(),
        first: false,
    };
    assert_eq!(operator_block(&exhausted(0)), Some(first));
    assert_eq!(operator_block(&exhausted(2)), Some(again));

    let inspection = BlockReason::Inspection {
        detail: "a call has no outcome".into(),
    };
    assert_eq!(
        operator_block(&AgentStatus::Blocked { reason: inspection }),
        Some(OperatorBlock::Inspection)
    );
    let dependency = BlockReason::Dependency { on: TaskId::new() };
    assert_eq!(
        operator_block(&AgentStatus::Blocked { reason: dependency }),
        None
    );
    assert_eq!(operator_block(&AgentStatus::Running), None);
}

#[test]
fn a_subscription_survives_the_table_and_an_unreadable_row_is_an_error() {
    let sqlite = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let table = SubscriptionTable(sqlite.clone());
    let device = DeviceId::new();
    let stored = StoredSubscription {
        device,
        subscription: UserAgent::generate().subscription(ENDPOINT),
        kinds: vec![AttentionKind::Approval, AttentionKind::Question],
    };
    table.put_subscription(&stored).expect("written");
    assert_eq!(
        table.subscription(device).expect("read"),
        Some(stored.clone())
    );
    assert_eq!(table.subscriptions().expect("listed"), vec![stored.clone()]);

    let renewed = StoredSubscription {
        subscription: UserAgent::generate().subscription(ENDPOINT),
        ..stored.clone()
    };
    table.put_subscription(&renewed).expect("replaced");
    assert!(
        !table.remove_exact_subscription(&stored).expect("compared"),
        "the one it replaced is no longer there to remove"
    );
    assert_eq!(
        table.subscription(device).expect("read"),
        Some(renewed.clone())
    );
    assert!(table.remove_exact_subscription(&renewed).expect("removed"));
    assert_eq!(table.subscription(device).expect("read"), None);

    let broken = PushSubscriptionRow {
        device,
        updated_at: aether_core::time::now(),
        body: serde_json::json!({"kinds": []}),
    };
    sqlite.put_push_subscription(&broken).expect("written");
    assert_eq!(
        table
            .subscription(device)
            .expect("a bad row is not an error"),
        None,
        "no subscription in the row"
    );
    assert!(table.subscriptions().expect("listed").is_empty());

    table.remove_subscription(device).expect("removed");
    table
        .remove_subscription(device)
        .expect("removing nothing is fine");
    assert_eq!(table.subscription(device).expect("read"), None);
}

#[tokio::test]
async fn an_unreadable_subscription_row_does_not_silence_other_devices() {
    let fixture = Fixture::start().await;
    let (_device, user_agent) = fixture.subscribed_device();
    let broken = PushSubscriptionRow {
        device: DeviceId::new(),
        updated_at: aether_core::time::now(),
        body: serde_json::json!({"kinds": []}),
    };
    fixture
        .sqlite
        .put_push_subscription(&broken)
        .expect("written");

    let agent = fixture.agent("builder");
    fixture.ask(agent, "Proceed?");
    let requests = fixture.requests(1).await;
    assert_eq!(requests.len(), 1, "the readable subscription was served");
    assert_eq!(user_agent.payload(&requests[0].body).agent, Some(agent));
    fixture.stop().await;
}
