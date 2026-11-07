//! Acceptance tests for the teaching commands, over a scripted model client:
//! sealing, answers and learner evidence, the disable policy, redelivery after
//! a restart, the budget, deletion, and the one dispatch table.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use aether_core::event::{Actor, EventKind};
use aether_core::id::AgentId;
use aether_core::model::StreamEvent;
use aether_core::model::{ContentBlock, MessageRole, Usage};
use aether_core::noesis::NoesisSession;
use aether_model::testing::{ScriptedClient, ScriptedResponse};
use aether_noesis::budget::Billing;
use aether_noesis::teach::engine::ModelInfo;
use aether_noesis::teach::records::Answer;
use aether_noesis::teach::{AgentSpec, Engine, EngineError, Input, Step, TeachError, Teaching};
use aether_protocol::noesis::{
    BillingPatch, BudgetPatch, DisablePatch, ModelsPatch, NoesisSettingsPatch,
};
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};
use aether_runtime::BusMessage;
use aether_store::Store;
use serde_json::{Value, json};

use super::routes::{Group, group_of};
use super::{NoesisService, State};
use crate::Daemon;

mod engine_path;
mod evidence;

const MODEL: &str = "anthropic/claude-sonnet-5-5";
const ASK: &str = "noesis_ask";

/// The directories one daemon life after another reopens.
struct Home {
    config: tempfile::TempDir,
    data: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            config: tempfile::tempdir().expect("config dir"),
            data: tempfile::tempdir().expect("data dir"),
        }
    }
}

struct Harness {
    daemon: Daemon,
    client: Arc<ScriptedClient>,
}

impl Harness {
    /// Boot as the daemon does: the service first, then the runtime over a
    /// file store, then the owed answers resumed.
    async fn open(home: &Home, client: Arc<ScriptedClient>) -> Self {
        Self::open_with(home, client.clone(), client).await
    }

    /// [`Harness::open`] with `model_client` making the calls; `client`
    /// still records what was asked.
    async fn open_with(
        home: &Home,
        client: Arc<ScriptedClient>,
        model_client: Arc<dyn aether_model::ModelClient>,
    ) -> Self {
        Self::boot(home, client, Some(model_client), Default::default()).await
    }

    /// Boot with no stand-in for API providers: each model is reached as
    /// the daemon reaches it, an engine model through `engines`.
    async fn open_on_engines(
        home: &Home,
        engines: std::collections::HashMap<
            aether_engine::EngineKind,
            Arc<dyn aether_engine::Engine>,
        >,
    ) -> Self {
        Self::boot(home, Arc::new(ScriptedClient::new()), None, engines).await
    }

    async fn boot(
        home: &Home,
        client: Arc<ScriptedClient>,
        model_client: Option<Arc<dyn aether_model::ModelClient>>,
        engines: std::collections::HashMap<
            aether_engine::EngineKind,
            Arc<dyn aether_engine::Engine>,
        >,
    ) -> Self {
        let models = Arc::new(
            aether_model::Models::load(Some(home.config.path().to_path_buf())).expect("models"),
        );
        let gate = aether_runtime::noesis_guard::NoesisGate::new();
        let store: Arc<dyn Store> = Arc::new(
            aether_store::SqliteStore::open(home.data.path().join("aether.db")).expect("store"),
        );
        let service = NoesisService::new(
            home.config.path(),
            home.data.path(),
            models.clone(),
            gate.clone(),
        );
        let noesis = Arc::new(match model_client {
            Some(model_client) => service.with_model_client(model_client),
            None => service,
        });
        noesis.start_at_boot(&store).await.expect("boot");
        let runtime = aether_runtime::Runtime::start_with_engines(
            aether_runtime::RuntimeConfig {
                holder: "test".into(),
                default_model: Some(aether_core::model::ModelRef::new(
                    "anthropic",
                    "claude-sonnet-5-5",
                )),
                attachment_dir: Some(home.config.path().join("attachments")),
                noesis: gate,
                ..Default::default()
            },
            store,
            models.clone(),
            Arc::new(aether_tools::ToolRegistry::with_builtins()),
            None,
            engines,
        )
        .await
        .expect("runtime");
        noesis.resume_teaching(&runtime);
        let daemon = Daemon {
            runtime,
            models,
            instance_id: "inst".into(),
            version: "test".into(),
            push: None,
            remote: Default::default(),
            status_reads: Default::default(),
            tree_reads: Default::default(),
            watcher_reads: Default::default(),
            noesis,
        };
        Self { daemon, client }
    }

    async fn close(self) {
        self.daemon.noesis.shutdown().await;
        self.daemon.runtime.shutdown().await;
    }

    async fn send(&self, command: Command) -> Result<Reply, ErrorReply> {
        let submission = crate::authz::AuthContext::Local.submission(&command)?;
        super::dispatch(&self.daemon, command, Actor::user(), submission).await
    }

    async fn send_json(&self, command: Value) -> Result<Reply, ErrorReply> {
        let command: Command = serde_json::from_value(command).expect("a command");
        self.send(command).await
    }

    async fn configure(&self, patch: NoesisSettingsPatch) {
        let configure = Command::NoesisConfigure {
            patch,
            idempotency_key: None,
        };
        self.send(configure).await.expect("configure");
    }

    async fn create(&self, extra: Value) -> NoesisSession {
        let mut command = json!({
            "cmd": "noesis_create_session",
            "goal": "Learn about limits",
            "mode": "teach",
            "idempotency_key": format!("create-{}", ulid::Ulid::generate()),
        });
        merge(&mut command, extra);
        match self.send_json(command).await.expect("created") {
            Reply::NoesisSession(session) => session,
            other => panic!("a session, got {other:?}"),
        }
    }

    async fn session(&self, id: &str) -> Result<NoesisSession, ErrorReply> {
        match self
            .send_json(json!({"cmd": "noesis_get_session", "session": id}))
            .await?
        {
            Reply::NoesisSession(session) => Ok(session),
            other => panic!("a session, got {other:?}"),
        }
    }

    /// The session once its question is posed.
    async fn posed(&self, id: &str) -> NoesisSession {
        for _ in 0..500 {
            let session = self.session(id).await.expect("session");
            if session.open_question.is_some() {
                return session;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no question was posed in {id}");
    }

    async fn answer(&self, session: &NoesisSession, label: &str) -> Result<Reply, ErrorReply> {
        let question = session.open_question.as_ref().expect("an open question");
        let options = question.options.as_ref().expect("a choice question");
        let chosen = options
            .iter()
            .find(|o| o.label == label)
            .expect("the option");
        self.send_json(json!({
            "cmd": "noesis_answer",
            "session": session.id,
            "question": question.id,
            "answer": {"choices": [chosen.value], "dont_know": false},
            "idempotency_key": format!("answer-{}", question.id),
        }))
        .await
    }

    /// Wait until no teaching call runs or waits.
    async fn settled(&self) {
        for _ in 0..500 {
            if self.daemon.noesis.teaching_calls.load(Ordering::SeqCst) == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("a teaching call never ended");
    }

    fn events(&self) -> Vec<aether_core::event::Event> {
        let filter = aether_store::EventFilter::default();
        self.daemon
            .runtime
            .store()
            .events(0, 100_000, &filter)
            .expect("events")
    }

    fn transcript(&self, agent: AgentId) -> Vec<aether_core::model::Message> {
        let entries = self
            .daemon
            .runtime
            .store()
            .transcript(agent, 0)
            .expect("transcript");
        entries.into_iter().map(|entry| entry.message).collect()
    }

    fn last_seq(&self) -> u64 {
        self.daemon.runtime.store().last_seq().expect("last seq")
    }

    async fn is_draining(&self) -> bool {
        matches!(
            &*self.daemon.noesis.state.lock().await,
            State::Draining { .. }
        )
    }

    async fn is_off(&self) -> bool {
        matches!(&*self.daemon.noesis.state.lock().await, State::Off)
    }
}

fn merge(into: &mut Value, extra: Value) {
    let (Some(into), Value::Object(extra)) = (into.as_object_mut(), extra) else {
        return;
    };
    into.extend(extra);
}

/// Enabled, on an API model with API billing allowed, with `edit` applied.
fn teaching_on(edit: impl FnOnce(&mut NoesisSettingsPatch)) -> NoesisSettingsPatch {
    let mut patch = NoesisSettingsPatch {
        enabled: Some(true),
        models: Some(ModelsPatch {
            strong: Some(MODEL.into()),
            cheap: None,
        }),
        billing: Some(BillingPatch {
            allow_api: Some(true),
        }),
        ..Default::default()
    };
    edit(&mut patch);
    patch
}

fn capital_question() -> Value {
    json!({
        "kind": "choice",
        "prompt": "Which city is the capital of France?",
        "options": [
            {"value": "opt1", "label": "Paris"},
            {"value": "opt2", "label": "Rome"},
            {"value": "opt3", "label": "Madrid"}
        ],
        "answer_key": ["opt1"],
        "explanation": "Paris has been the capital since 987."
    })
}

/// `response` with every usage reading costing `usd`.
fn costing(response: ScriptedResponse, usd: f64) -> ScriptedResponse {
    response.map(|events| {
        events
            .into_iter()
            .map(|event| match event {
                StreamEvent::Usage { usage } => StreamEvent::Usage {
                    usage: Usage {
                        cost_usd: usd,
                        ..usage
                    },
                },
                other => other,
            })
            .collect()
    })
}

fn agent_of(session: &NoesisSession) -> AgentId {
    session.agent
}

/// The `noesis_ask` calls an agent made, as persisted.
fn asks(transcript: &[aether_core::model::Message]) -> Vec<(String, Value)> {
    transcript
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } if name == ASK => {
                Some((id.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The results given for call `id`, with their text.
fn results_for(transcript: &[aether_core::model::Message], id: &str) -> Vec<String> {
    transcript
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == id => Some(format!("{content:?}")),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_probe_question_is_answered_and_the_learner_model_shows_it() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    client.push(ScriptedClient::text("Right. Next, limits."));
    let harness = Harness::open(&home, client).await;
    harness.configure(teaching_on(|_| {})).await;
    let mut bus = harness.daemon.runtime.subscribe();

    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;

    let agent = agent_of(&posed);
    let transcript = harness.transcript(agent);
    let asked = asks(&transcript);
    assert_eq!(asked.len(), 1, "{transcript:?}");
    assert_eq!(
        asked[0].1["answer_key"],
        json!("<sealed>"),
        "{:?}",
        asked[0].1
    );
    assert_eq!(asked[0].1["explanation"], json!("<sealed>"));
    let logged = serde_json::to_string(&harness.events()).expect("json");
    assert!(
        !logged.contains("Paris has been the capital"),
        "the key never reaches the log"
    );

    harness.answer(&posed, "Paris").await.expect("answered");
    harness.settled().await;
    let results = results_for(&harness.transcript(agent), &asked[0].0);
    assert_eq!(
        results.len(),
        1,
        "the answer is the parked call's one result"
    );
    assert!(results[0].contains("\\\"correct\\\":true"), "{results:?}");
    assert_eq!(
        harness.client.requests().len(),
        2,
        "the agent read the answer"
    );

    let events = harness.events();
    let recorded = events.iter().find_map(|event| match &event.kind {
        EventKind::NoesisAnswerRecorded { session, result } if *session == created.id => {
            Some(result.clone())
        }
        _ => None,
    });
    let recorded = recorded.expect("noesis.answer_recorded");
    assert_eq!(recorded.correct, Some(true));
    assert!(recorded.answer_key.is_some(), "revealed once answered");
    let updated: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::NoesisLearnerUpdated { concepts } => Some(concepts.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(updated.len(), 1, "one append for the one answer");
    let Reply::NoesisLearner { concepts } = harness
        .send_json(json!({"cmd": "noesis_learner", "concepts": updated[0]}))
        .await
        .expect("learner")
    else {
        panic!("a learner reply");
    };
    assert_eq!(concepts.len(), updated[0].len());
    // A probe answer calibrates the learner model; it never counts as mastery.
    assert!(
        concepts
            .iter()
            .all(|state| state.calibration.is_some() && state.as_of_evidence > 0),
        "{concepts:?}"
    );

    assert_deltas_sealed_and_gapless(&mut bus, agent);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_message_starts_no_turn_and_a_sent_one_starts_one() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::text("What would you like to learn first?"));
    client.push(ScriptedClient::text("Let us start with limits."));
    let harness = Harness::open(&home, client).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    harness.settled().await;
    assert_eq!(harness.client.requests().len(), 1, "the opening turn");

    let blank = json!({"cmd": "noesis_send", "session": created.id, "text": "   ",
                       "idempotency_key": "s1"});
    let refused = harness.send_json(blank).await.expect_err("blank");
    assert_eq!(refused.code, ErrorCode::InvalidArgument, "{refused:?}");
    let unknown = json!({"cmd": "noesis_send", "session": "ns_missing", "text": "hi",
                         "idempotency_key": "s2"});
    let missing = harness.send_json(unknown).await.expect_err("unknown");
    assert_eq!(missing.code, ErrorCode::NotFound, "{missing:?}");
    harness.settled().await;
    assert_eq!(
        harness.client.requests().len(),
        1,
        "no billable turn for a refusal"
    );

    let sent = json!({"cmd": "noesis_send", "session": created.id, "text": "Limits, please",
                      "idempotency_key": "s3"});
    harness.send_json(sent).await.expect("sent");
    harness.settled().await;
    assert_eq!(
        harness.client.requests().len(),
        2,
        "one turn for the message"
    );
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_committed_turn_queues_the_session_for_extraction_in_auto_mode() {
    use aether_protocol::noesis::{ExtractionPatch, SessionExtraction};
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    let prose = "A limit describes where a function heads as its input approaches a point. ";
    client.push(ScriptedClient::text(&prose.repeat(400)));
    let harness = Harness::open(&home, client).await;
    harness
        .configure(teaching_on(|patch| {
            patch.extraction = Some(ExtractionPatch {
                sessions: Some(SessionExtraction::Auto),
                ..Default::default()
            });
        }))
        .await;
    let created = harness.create(json!({})).await;
    harness.settled().await;
    let Reply::NoesisJobs { jobs } = harness
        .send_json(json!({"cmd": "noesis_jobs"}))
        .await
        .expect("jobs")
    else {
        panic!("a jobs reply");
    };
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].kind, aether_core::noesis::JobKind::ExtractSession);
    assert!(format!("{:?}", jobs[0]).contains(&created.id), "{jobs:?}");
    harness.close().await;
}

/// No `tool_input` delta reaches the bus for the agent, and each of its
/// invocations is numbered 1, 2, 3, … with no gap.
fn assert_deltas_sealed_and_gapless(
    bus: &mut tokio::sync::broadcast::Receiver<BusMessage>,
    agent: AgentId,
) {
    let mut seqs: std::collections::HashMap<_, Vec<u64>> = Default::default();
    while let Ok(message) = bus.try_recv() {
        let BusMessage::Delta {
            agent: from,
            invocation,
            seq,
            delta,
        } = message
        else {
            continue;
        };
        if from != agent {
            continue;
        }
        assert!(
            !matches!(delta, aether_runtime::DeltaKind::ToolInput { .. }),
            "a Noesis tool's input is never streamed"
        );
        seqs.entry(invocation).or_default().push(seq);
    }
    for numbered in seqs.values() {
        let expected: Vec<u64> = (1..=numbered.len() as u64).collect();
        assert_eq!(*numbered, expected, "invocation_seq is gapless");
    }
}

fn teaching_samples() -> Vec<Value> {
    vec![
        json!({"cmd": "noesis_create_session", "goal": "g", "mode": "teach", "idempotency_key": "k1"}),
        json!({"cmd": "noesis_send", "session": "ns_1", "text": "hi", "idempotency_key": "k2"}),
        json!({"cmd": "noesis_answer", "session": "ns_1", "question": "q_1",
               "answer": {"dont_know": true}, "idempotency_key": "k3"}),
        json!({"cmd": "noesis_get_session", "session": "ns_1"}),
        json!({"cmd": "noesis_list_sessions"}),
        json!({"cmd": "noesis_delete_session", "session": "ns_1", "graph": "preserve",
               "idempotency_key": "k4"}),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_every_teaching_command_is_refused_with_no_store_change() {
    let home = Home::new();
    let harness = Harness::open(&home, Arc::new(ScriptedClient::new())).await;
    let before = harness.last_seq();
    for sample in teaching_samples() {
        let error = harness
            .send_json(sample.clone())
            .await
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::Unavailable, "{sample}");
        assert_eq!(error.reason.as_deref(), Some("noesis_disabled"), "{sample}");
    }
    assert_eq!(harness.last_seq(), before, "nothing was recorded");
    assert!(
        !home.data.path().join("noesis.db").exists(),
        "nothing was opened"
    );
    assert!(harness.client.requests().is_empty());
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disable_mid_turn_refuses_at_once_and_cancels_after_the_grace() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    let _held = client.hold_next_response();
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client).await;
    harness
        .configure(teaching_on(|patch| {
            patch.disable = Some(DisablePatch {
                grace_seconds: Some(1),
            });
        }))
        .await;
    let created = harness.create(json!({})).await;
    for _ in 0..500 {
        if !harness.client.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        harness.client.requests().len(),
        1,
        "the turn's model call is in flight"
    );

    let disable = NoesisSettingsPatch {
        enabled: Some(false),
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(2), harness.configure(disable))
        .await
        .expect("a disable never waits for a teaching turn");
    for sample in teaching_samples() {
        let error = harness
            .send_json(sample)
            .await
            .expect_err("refused at once");
        assert_eq!(error.reason.as_deref(), Some("noesis_disabled"));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(harness.is_draining().await, "the turn keeps its grace");

    for _ in 0..300 {
        if harness.is_off().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        harness.is_off().await,
        "cancelled and stopped after the grace"
    );
    harness.settled().await;
    let transcript = harness.transcript(created.agent);
    assert!(
        transcript.iter().all(|m| m.role != MessageRole::Assistant),
        "the cancelled call recorded no reply"
    );
    harness.close().await;
}

/// An engine whose every delivery fails: the answer is committed, owed.
struct Unreachable;

impl Engine for Unreachable {
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: MODEL.into(),
            billing: Billing::Api,
            ready: false,
        }]
    }

    fn spawn(&mut self, _spec: AgentSpec) -> Result<String, EngineError> {
        Err(EngineError::NotSent("unreachable".into()))
    }

    fn deliver(&mut self, _agent: &str, _input: Input) -> Result<Option<u64>, EngineError> {
        Err(EngineError::NotSent("the daemon went down".into()))
    }

    fn estimate(&self, _agent: &str) -> aether_noesis::budget::Spend {
        aether_noesis::budget::Spend::default()
    }

    fn step(&mut self, _agent: &str) -> Result<Step, EngineError> {
        Err(EngineError::NotSent("unreachable".into()))
    }
}

/// Record the answer as the daemon does, and lose its delivery.
async fn answer_undelivered(harness: &Harness, session: &NoesisSession, label: &str) {
    let question = session.open_question.as_ref().expect("open");
    let options = question.options.as_ref().expect("options");
    let chosen = options.iter().find(|o| o.label == label).expect("option");
    let state = harness.daemon.noesis.state.lock().await;
    let State::Running(live) = &*state else {
        panic!("running");
    };
    let desk = live.desk.lock().await;
    let noesis = desk.noesis.as_ref().expect("open");
    let mut teaching = Teaching::new(noesis, Unreachable);
    let answer = Answer {
        choices: vec![chosen.value.clone()],
        text: None,
        dont_know: false,
        confidence: None,
        thinking: None,
    };
    let answered = teaching.answer(&session.id, &question.id, &answer);
    assert!(
        matches!(answered, Err(TeachError::Engine(_))),
        "{answered:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_redelivers_an_owed_answer_exactly_once() {
    let home = Home::new();
    let first = Arc::new(ScriptedClient::new());
    first.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, first).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    answer_undelivered(&harness, &posed, "Paris").await;
    let (call, _) = asks(&harness.transcript(posed.agent)).remove(0);
    assert!(
        results_for(&harness.transcript(posed.agent), &call).is_empty(),
        "owed"
    );
    harness.close().await;

    let second = Arc::new(ScriptedClient::new());
    second.push(ScriptedClient::text("Right."));
    let harness = Harness::open(&home, second).await;
    for _ in 0..500 {
        if harness.client.requests().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    harness.settled().await;
    assert_eq!(
        results_for(&harness.transcript(posed.agent), &call).len(),
        1
    );
    assert_eq!(harness.client.requests().len(), 1, "the agent read it");
    harness.close().await;

    let third = Harness::open(&home, Arc::new(ScriptedClient::new())).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    third.settled().await;
    assert_eq!(
        results_for(&third.transcript(posed.agent), &call).len(),
        1,
        "delivered exactly once"
    );
    assert!(third.client.requests().is_empty(), "nothing was owed");
    third.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_invocation_is_charged_and_a_spent_budget_refuses_before_the_call() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(costing(
        ScriptedClient::tool_call(ASK, capital_question()),
        0.05,
    ));
    client.push(ScriptedClient::text("never sent"));
    let harness = Harness::open(&home, client).await;
    harness
        .configure(teaching_on(|patch| {
            patch.budget = Some(BudgetPatch {
                interactive_usd_daily: Some(0.01),
                ..Default::default()
            });
        }))
        .await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    let Reply::NoesisStatus(status) = harness.send(Command::NoesisStatus).await.expect("status")
    else {
        panic!("a status");
    };
    assert_eq!(
        status.spend_today.interactive.cost_usd, 0.05,
        "charged to Noesis"
    );
    assert_eq!(harness.client.requests().len(), 1);

    harness.answer(&posed, "Rome").await.expect("recorded");
    harness.settled().await;
    assert_eq!(
        harness.client.requests().len(),
        1,
        "refused before the model call"
    );
    let (call, _) = asks(&harness.transcript(posed.agent)).remove(0);
    assert_eq!(
        results_for(&harness.transcript(posed.agent), &call).len(),
        1,
        "the answer is still delivered"
    );
    assert_stalled_and_refused(&harness, &created.id).await;
    harness.close().await;
}

/// A turn the budget stopped is recorded on the session and announced; a new
/// session or message is refused up front, naming spent, limit and setting.
async fn assert_stalled_and_refused(harness: &Harness, id: &str) {
    use aether_core::noesis::StallReason;
    const SETTING: &str = "noesis.budget.interactive_usd_daily";
    let session = harness.session(id).await.expect("session");
    let stall = session
        .stalled
        .expect("the refused turn stalls the session");
    assert_eq!(stall.reason, StallReason::BudgetExhausted);
    assert!(stall.message.contains(SETTING), "{}", stall.message);
    let announced = harness.events().into_iter().any(|event| {
        matches!(&event.kind, EventKind::NoesisSessionStalled { stall: s, .. } if *s == stall)
    });
    assert!(announced, "noesis.session_stalled is published");
    let usage = session.usage.expect("usage of a session read");
    assert_eq!(usage.api.cost_usd, 0.05, "an API model's use is charged");
    assert_eq!(usage.subscription, Usage::default());

    let send = json!({"cmd": "noesis_send", "session": id, "text": "hello?",
                      "idempotency_key": "send-spent"});
    let create = json!({"cmd": "noesis_create_session", "goal": "Learn more",
                        "mode": "teach", "idempotency_key": "create-spent"});
    for command in [send, create] {
        let refused = harness
            .send_json(command)
            .await
            .expect_err("refused up front");
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert_eq!(
            refused.reason.as_deref(),
            Some(aether_protocol::NOESIS_BUDGET_EXHAUSTED)
        );
        for named in ["$0.05", "$0.01", SETTING] {
            assert!(refused.message.contains(named), "{}", refused.message);
        }
    }
    assert_eq!(harness.client.requests().len(), 1, "no model was asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_removes_the_rows_and_the_bundle_and_keeps_the_transcript() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client).await;
    harness
        .configure(teaching_on(|patch| patch.engineering_context = Some(true)))
        .await;
    let bundle = json!({"context": {"title": "Release notes", "items": [{
        "ref": {"kind": "pr", "id": "401"},
        "excerpt": "Wire the teaching service.",
        "status": "reviewed"
    }]}});
    let created = harness.create(bundle).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    assert_eq!(
        evidence::bundle_rows(&home, &created.id),
        1,
        "the bundle is kept with the session"
    );
    let transcript = harness.transcript(posed.agent);
    assert!(
        format!("{transcript:?}").contains("Release notes"),
        "the agent read it"
    );

    let nodes = json!({"cmd": "noesis_delete_session", "session": created.id,
                       "graph": "delete_nodes", "idempotency_key": "d0"});
    let error = harness.send_json(nodes).await.expect_err("F8's");
    assert_eq!(error.code, ErrorCode::Unimplemented);
    assert!(
        harness.session(&created.id).await.is_ok(),
        "nothing was deleted"
    );

    let delete = json!({"cmd": "noesis_delete_session", "session": created.id,
                        "graph": "preserve", "idempotency_key": "d1"});
    harness.send_json(delete).await.expect("deleted");
    let missing = harness.session(&created.id).await.expect_err("gone");
    assert_eq!(missing.code, ErrorCode::NotFound);
    let Reply::NoesisSessions { sessions, .. } = harness
        .send_json(json!({"cmd": "noesis_list_sessions"}))
        .await
        .expect("list")
    else {
        panic!("a page");
    };
    assert!(sessions.is_empty());
    assert_eq!(
        evidence::bundle_rows(&home, &created.id),
        0,
        "the bundle went with it"
    );
    assert_eq!(
        harness.transcript(posed.agent),
        transcript,
        "the transcript stays"
    );
    harness.close().await;
}

/// Every `noesis_*` command the protocol schema names.
fn schema_commands() -> BTreeSet<String> {
    fn walk(value: &Value, names: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                if let Some(Value::Object(cmd)) =
                    fields.get("properties").and_then(|p| p.get("cmd"))
                    && let Some(Value::String(name)) = cmd.get("const")
                    && name.starts_with("noesis_")
                {
                    names.insert(name.clone());
                }
                fields.values().for_each(|v| walk(v, names));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, names)),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    walk(&aether_protocol::schema::schema(), &mut names);
    names
}

#[test]
fn the_dispatch_table_places_every_noesis_command_in_one_group() {
    let samples = [
        (json!({"cmd": "noesis_status"}), Group::Service),
        (
            json!({"cmd": "noesis_configure", "patch": {}}),
            Group::Service,
        ),
        (json!({"cmd": "noesis_learner"}), Group::Learner),
        (
            json!({"cmd": "noesis_learner_evidence", "concept": "k_1"}),
            Group::Learner,
        ),
        (
            json!({"cmd": "noesis_correct_learner", "concept": "k_1", "state": "fragile", "reason": "r"}),
            Group::Learner,
        ),
        (json!({"cmd": "noesis_graph"}), Group::Graph),
        (
            json!({"cmd": "noesis_delete_proposition", "proposition": "p_1"}),
            Group::Graph,
        ),
        (
            json!({"cmd": "noesis_delete_source", "source": "src_1", "graph": "preserve"}),
            Group::Graph,
        ),
        (
            json!({"cmd": "noesis_extract", "session": "ns_1"}),
            Group::Graph,
        ),
        (
            json!({"cmd": "noesis_import_source", "name": "n",
                   "artifact": aether_core::id::ArtifactId::new().to_string()}),
            Group::Graph,
        ),
        (json!({"cmd": "noesis_jobs"}), Group::Graph),
        (
            json!({"cmd": "noesis_cancel_job", "job": "job_1"}),
            Group::Graph,
        ),
    ];
    let teaching = teaching_samples()
        .into_iter()
        .map(|json| (json, Group::Teaching));
    let mut placed = BTreeSet::new();
    for (json, group) in samples.into_iter().chain(teaching) {
        let command: Command =
            serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("{json}: {e}"));
        assert!(command.is_noesis(), "{json}");
        assert_eq!(group_of(&command), group, "{json}");
        assert!(
            placed.insert(command.name().to_string()),
            "one sample each: {json}"
        );
    }
    let named = schema_commands();
    assert!(
        named.contains("noesis_status") && named.contains("noesis_answer"),
        "{named:?}"
    );
    assert_eq!(placed, named, "every Noesis command has its group");
}

/// A pause, cancel or settings change racing a teaching call's reply.
#[path = "teaching_race_tests.rs"]
mod race;
