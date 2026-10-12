use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use aether_core::event::{Actor, Event, EventDraft, EventKind, OperatorOrigin};
use aether_core::id::{AgentId, ApprovalId, EventId, ToolCallId};
use aether_core::tool::SideEffectClass;
use aether_protocol::remote::{DeviceId, PushPayload, RemoteSettings};
use parking_lot::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::push::testing::{MemoryPushStore, RecordingTransport, StaticFleet, UserAgent};
use crate::push::transport::{PushResponse, TransportError};
use crate::push::{AgentFacts, HostPolicy, Notifier, NotifierConfig, PushRequest, Transport};

pub(super) const INSTANCE: &str = "inst_test";
pub(super) const SUBJECT: &str = "mailto:ops@example.com";
pub(super) const ENDPOINT: &str = "https://fcm.googleapis.com/fcm/send/abc123";
pub(super) const SECRET: &str = "rm -rf /srv/secret-project";
/// Long enough for the settle delay and a send, shorter than the quiet window.
pub(super) const SETTLED: Duration = Duration::from_secs(1);

/// A notifier over in-memory collaborators, with its delivery worker running.
pub(super) struct Harness {
    pub notifier: Notifier,
    pub store: Arc<MemoryPushStore>,
    pub fleet: Arc<StaticFleet>,
    pub transport: Arc<RecordingTransport>,
    pub dir: tempfile::TempDir,
    seq: std::sync::atomic::AtomicU64,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub(super) fn settings(push: bool) -> RemoteSettings {
    RemoteSettings {
        enabled: true,
        pairing: false,
        public_origins: vec!["https://box.example.ts.net".into()],
        push,
    }
}

pub(super) fn config(dir: &tempfile::TempDir, previews: bool) -> NotifierConfig {
    NotifierConfig {
        instance_id: INSTANCE.into(),
        previews,
        subject: Some(SUBJECT.into()),
        hosts: HostPolicy::default(),
        key_path: dir.path().join("push-vapid.key"),
    }
}

impl Harness {
    /// Push enabled, previews as given, a recording transport.
    pub(super) fn start(previews: bool) -> Self {
        let transport = Arc::new(RecordingTransport::default());
        let harness = Self::with_transport(previews, transport.clone(), transport);
        harness
            .notifier
            .apply_settings(&settings(true))
            .expect("enabling push");
        harness
    }

    /// Push still disabled; `carrier` is what the notifier sends through.
    pub(super) fn with_transport(
        previews: bool,
        transport: Arc<RecordingTransport>,
        carrier: Arc<dyn Transport>,
    ) -> Self {
        let harness = Self::without_worker(previews, transport, carrier);
        let (worker, stop) = (harness.notifier.clone(), harness.shutdown());
        tokio::spawn(async move { worker.run(stop).await });
        harness
    }

    /// Ends a delivery worker; cancelled when the harness is dropped.
    pub(super) fn shutdown(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// As [`Harness::with_transport`], for a test that runs the worker itself.
    pub(super) fn without_worker(
        previews: bool,
        transport: Arc<RecordingTransport>,
        carrier: Arc<dyn Transport>,
    ) -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let store = Arc::new(MemoryPushStore::default());
        let fleet = Arc::new(StaticFleet::default());
        let notifier = Notifier::new(
            config(&dir, previews),
            store.clone(),
            fleet.clone(),
            carrier,
        );
        let shutdown = CancellationToken::new();
        Self {
            notifier,
            store,
            fleet,
            transport,
            dir,
            seq: std::sync::atomic::AtomicU64::new(100),
            shutdown,
        }
    }

    /// Pair a browser: a fresh device subscribed at [`ENDPOINT`].
    pub(super) fn subscribe(&self) -> (DeviceId, UserAgent) {
        let device = DeviceId::new();
        let agent = UserAgent::generate();
        self.notifier
            .subscribe(device, agent.subscription(ENDPOINT), Vec::new())
            .expect("subscribing");
        (device, agent)
    }

    /// A root agent the fleet knows.
    pub(super) fn agent(&self, name: &str) -> AgentId {
        let agent = AgentId::new();
        self.fleet.set_agent(
            agent,
            AgentFacts {
                name: name.into(),
                root: true,
                block: None,
            },
        );
        agent
    }

    pub(super) fn event(&self, actor: Actor, agent: AgentId, kind: EventKind) -> Event {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let draft = EventDraft::new(actor, kind).agent(agent);
        Event::from_draft(draft, EventId::new(), seq, aether_core::time::now())
    }

    /// Put `kind` on the bus as the agent's own doing; returns its `seq`.
    pub(super) fn emit(&self, agent: AgentId, kind: EventKind) -> u64 {
        let event = self.event(Actor::agent(agent), agent, kind);
        self.notifier.observe(&event).expect("observing");
        event.seq
    }

    /// An approval that is still pending starts waiting.
    pub(super) fn ask_approval(&self, agent: AgentId) -> (ApprovalId, u64) {
        let approval = ApprovalId::new();
        self.fleet.set_pending(approval);
        let seq = self.emit(agent, approval_requested(approval));
        (approval, seq)
    }

    /// Every payload sent so far that `agent` can decrypt, in order.
    pub(super) fn received(&self, agent: &UserAgent) -> Vec<PushPayload> {
        self.transport
            .requests()
            .iter()
            .filter_map(|request| agent.open(&request.body).ok())
            .map(|plaintext| serde_json::from_slice(&plaintext).expect("a push payload"))
            .collect()
    }
}

/// A push service that keeps every request until the test answers it.
#[derive(Debug)]
pub(super) struct Held {
    pub asked: Mutex<Vec<PushRequest>>,
    answers: Mutex<VecDeque<u16>>,
    answered: Semaphore,
}

impl Held {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            asked: Mutex::default(),
            answers: Mutex::default(),
            answered: Semaphore::new(0),
        })
    }

    /// Answer the request held longest, or else the next to arrive.
    pub(super) fn answer(&self, status: u16) {
        self.answers.lock().push_back(status);
        self.answered.add_permits(1);
    }

    pub(super) fn endpoints(&self) -> Vec<String> {
        let asked = self.asked.lock();
        asked.iter().map(|r| r.endpoint.to_string()).collect()
    }
}

#[async_trait::async_trait]
impl Transport for Held {
    async fn post(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        self.asked.lock().push(request);
        let answered = self.answered.acquire().await.expect("never closed");
        answered.forget();
        let status = self.answers.lock().pop_front().expect("one per permit");
        Ok(PushResponse {
            status,
            retry_after: None,
        })
    }
}

pub(super) fn approval_requested(approval: ApprovalId) -> EventKind {
    EventKind::ApprovalRequested {
        approval,
        call: ToolCallId::new(),
        tool: "shell".into(),
        description: SECRET.into(),
        argument: None,
        side_effect: SideEffectClass::Mutation,
    }
}

pub(super) fn failed() -> EventKind {
    EventKind::AgentFailed {
        error: SECRET.into(),
    }
}

pub(super) fn by_device(device: DeviceId) -> Actor {
    Actor::connected_user(OperatorOrigin::Device {
        device,
        name: "phone".into(),
        connection: "c1".into(),
    })
}
