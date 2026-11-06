//! Noesis on a subscription engine CLI with `allow_api = false`: the engine
//! model is offered ready and chosen, an unkeyed API model is not, and a
//! whole session (probe, plan, teach, practice) runs through a faux Claude
//! Code engine with its tool calls, sealed values and budget charges.

use std::collections::{HashMap, VecDeque};

use aether_core::noesis::QuestionStep;
use aether_engine::{EngineError as CliError, EngineEvent, EngineKind, EngineOutcome, EngineRun};
use aether_protocol::noesis::{NoesisBilling, NoesisStatus};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;

const ENGINE_MODEL: &str = "faux-engine/tutor";
const KEY_SECRET: &str = "Paris has been the capital since 987.";
const SOLUTION_SECRET: &str = "SOLUTION-FIVE-41";

/// The builtin engines are switched off so the test does not depend on
/// this machine's logins; `unkeyed` is an API provider with no key set.
const PROVIDERS: &str = r#"
[providers.claude-code]
enabled = false
[providers.codex]
enabled = false
[providers.faux-engine]
name = "Faux engine CLI"
protocol = "claude-code"
base_url = ""
[providers.faux-engine.auth]
none = true
[[providers.faux-engine.models]]
id = "tutor"
[providers.faux-engine.models.capabilities]
context_window = 200000
max_output_tokens = 8000
[providers.unkeyed]
name = "Unkeyed API"
protocol = "openai-chat-completions"
base_url = "http://127.0.0.1:9/v1"
[providers.unkeyed.auth]
api_key_env = ["AETHER_NOESIS_TEST_KEY_NEVER_SET"]
[[providers.unkeyed.models]]
id = "m"
[providers.unkeyed.models.capabilities]
context_window = 32000
max_output_tokens = 4000
"#;

/// One launch as the engine saw it.
#[derive(Debug, Clone)]
struct Launch {
    system: String,
    prompt: String,
    had_mcp: bool,
}

/// A Claude Code stand-in answering each launch with the next reply queued
/// for the agent its tool list names, and a plain line once that is empty.
#[derive(Default)]
struct FauxCli {
    replies: Mutex<HashMap<&'static str, VecDeque<Value>>>,
    launches: Mutex<Vec<Launch>>,
}

impl FauxCli {
    fn queue(&self, agent: &'static str, reply: Value) {
        self.replies
            .lock()
            .entry(agent)
            .or_default()
            .push_back(reply);
    }

    fn launches(&self) -> Vec<Launch> {
        self.launches.lock().clone()
    }

    fn agent_of(system: &str) -> &'static str {
        if system.contains("### noesis_probe_done") {
            "prober"
        } else if system.contains("### noesis_plan") {
            "teacher"
        } else {
            "practitioner"
        }
    }
}

#[async_trait::async_trait]
impl aether_engine::Engine for FauxCli {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "faux-claude-code/1".into()
    }

    async fn run(
        &self,
        turn: aether_engine::EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, CliError> {
        let agent = Self::agent_of(&turn.system_prompt);
        self.launches.lock().push(Launch {
            system: turn.system_prompt.clone(),
            prompt: turn.prompt.clone(),
            had_mcp: turn.mcp.is_some(),
        });
        let queued = self
            .replies
            .lock()
            .get_mut(agent)
            .and_then(VecDeque::pop_front);
        let reply = queued
            .unwrap_or_else(|| json!({"text": "Go on."}))
            .to_string();
        let (tx, events) = mpsc::channel(4);
        tokio::spawn(async move {
            let usage = Usage {
                input_tokens: 400,
                output_tokens: 60,
                ..Default::default()
            };
            let content = vec![ContentBlock::Text {
                text: reply.clone(),
            }];
            let said = EngineEvent::Assistant {
                content,
                usage: None,
                model: None,
            };
            let done = EngineEvent::Done(EngineOutcome {
                usage,
                closing_accounting: true,
                cost_usd: Some(0.002),
                text: Some(reply),
                ..Default::default()
            });
            for event in [said, done] {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(EngineRun {
            events,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

async fn engine_harness(home: &Home, cli: Arc<FauxCli>) -> Harness {
    std::fs::write(home.config.path().join("providers.toml"), PROVIDERS).expect("providers");
    let mut engines: HashMap<EngineKind, Arc<dyn aether_engine::Engine>> = HashMap::new();
    engines.insert(EngineKind::CLAUDE_CODE, cli);
    let harness = Harness::open_on_engines(home, engines).await;
    harness
        .configure(NoesisSettingsPatch {
            enabled: Some(true),
            billing: Some(BillingPatch {
                allow_api: Some(false),
            }),
            ..Default::default()
        })
        .await;
    harness
}

async fn noesis_status(harness: &Harness) -> NoesisStatus {
    match harness
        .send_json(json!({"cmd": "noesis_status"}))
        .await
        .expect("status")
    {
        Reply::NoesisStatus(status) => status,
        other => panic!("a status, got {other:?}"),
    }
}

fn tool_call(name: &str, input: Value) -> Value {
    json!({"name": name, "input": input})
}

/// The posed question's step, as the session view shows it.
fn step_of(session: &NoesisSession) -> Option<QuestionStep> {
    session
        .open_question
        .as_ref()
        .and_then(|question| question.step)
}

/// No event holds `secret` yet, and every entry of `agent` that carried it
/// as a tool input holds `"<sealed>"` in its place.
fn assert_sealed(harness: &Harness, agent: AgentId, secret: &str) {
    let logged = serde_json::to_string(&harness.events()).expect("json");
    assert!(!logged.contains(secret), "the sealed value reached the log");
    let transcript = serde_json::to_string(&harness.transcript(agent)).expect("json");
    assert!(
        !transcript.contains(secret),
        "the transcript holds the value"
    );
    assert!(transcript.contains("<sealed>"), "{transcript}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_model_is_offered_ready_and_chosen_and_an_unkeyed_api_model_is_not() {
    let home = Home::new();
    let harness = engine_harness(&home, Arc::new(FauxCli::default())).await;
    let status = noesis_status(&harness).await;
    let offer = |id: &str| {
        status
            .models
            .iter()
            .find(|offer| offer.id == id)
            .unwrap_or_else(|| panic!("{id} is offered: {:?}", status.models))
    };
    let engine = offer(ENGINE_MODEL);
    assert!(engine.ready, "{engine:?}");
    assert_eq!(engine.billing, NoesisBilling::Subscription);
    let unkeyed = offer("unkeyed/m");
    assert!(!unkeyed.ready, "no key, not ready: {unkeyed:?}");
    assert_eq!(unkeyed.billing, NoesisBilling::Api);
    let chosen = status.teaching_model.expect("a choice");
    assert_eq!(chosen.model.as_deref(), Some(ENGINE_MODEL), "{chosen:?}");

    harness
        .configure(NoesisSettingsPatch {
            models: Some(ModelsPatch {
                strong: Some("unkeyed/m".into()),
                cheap: None,
            }),
            ..Default::default()
        })
        .await;
    let refused = noesis_status(&harness)
        .await
        .teaching_model
        .expect("a choice");
    assert_eq!(refused.model, None);
    let why = refused.refusal.expect("a refusal");
    assert!(why.contains("noesis.billing.allow_api"), "{why}");
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_whole_session_runs_on_the_engine_with_sealed_answers_and_charged_steps() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let mut question = capital_question();
    question["explanation"] = json!(KEY_SECRET);
    cli.queue(
        "prober",
        json!({"text": "First, a quick check.", "tool_calls": [tool_call(ASK, question.clone())]}),
    );
    let strands = json!({"strands": [{"strand": "capitals", "floor": "knows Europe",
        "ceiling": "knows history", "misconceptions": [], "start": "France"}]});
    cli.queue(
        "prober",
        json!({"text": "", "tool_calls": [tool_call("noesis_probe_done", strands)]}),
    );
    let plan = json!({"nodes": [{"id": "a", "title": "Capitals"}], "edges": []});
    let mut check = question;
    check["plan_node"] = json!("a");
    check["intent"] = json!("check");
    cli.queue(
        "teacher",
        json!({"text": "Here is the plan.", "tool_calls": [
            tool_call("noesis_plan", plan), tool_call(ASK, check)]}),
    );
    cli.queue("teacher", json!({"text": "Well done; that is the lesson."}));
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();

    let created = harness.create(json!({"goal": "Learn capitals"})).await;
    let probe = harness.posed(&created.id).await;
    harness.settled().await;
    assert_eq!(step_of(&probe), Some(QuestionStep::Measure));
    let prober = agent_of(&probe);
    let row = harness.daemon.runtime.store().agent(prober).expect("row");
    assert_eq!(row.expect("the prober").model.to_string(), ENGINE_MODEL);
    assert_sealed(&harness, prober, KEY_SECRET);
    let first = &cli.launches()[0];
    assert!(!first.had_mcp, "the step gets no tool server");
    assert!(first.system.contains("### noesis_ask"), "{}", first.system);
    assert!(first.prompt.contains("Learn capitals"), "{}", first.prompt);

    harness.answer(&probe, "Paris").await.expect("answered");
    let teach = harness.posed(&created.id).await;
    harness.settled().await;
    assert_ne!(agent_of(&teach), prober, "the Teacher took over");
    assert_eq!(step_of(&teach), Some(QuestionStep::Check));
    let seen: Vec<Launch> = cli.launches();
    let graded = seen
        .iter()
        .find(|launch| launch.prompt.contains("## Tool results"))
        .expect("the Prober read its answer");
    assert!(
        graded.prompt.contains("\"correct\":true"),
        "{}",
        graded.prompt
    );
    assert!(
        seen.iter()
            .all(|launch| !launch.prompt.contains(KEY_SECRET)),
        "the engine never reads the sealed explanation back"
    );
    harness.answer(&teach, "Paris").await.expect("answered");
    harness.settled().await;

    let recorded = harness
        .events()
        .iter()
        .filter(|event| matches!(event.kind, EventKind::NoesisAnswerRecorded { .. }))
        .count();
    assert_eq!(recorded, 2, "both answers were graded");
    let spent = noesis_status(&harness).await.spend_today.interactive;
    assert!(spent.cost_usd > 0.0, "engine steps are charged: {spent:?}");
    assert!(
        cli.launches().len() >= 4,
        "every step was one engine launch"
    );
    let streamed = live_text(&mut bus, prober);
    assert!(streamed.contains("First, a quick check."), "{streamed}");
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_practice_problem_on_the_engine_keeps_its_solution_sealed_until_concluded() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let problem = json!({"statement": "What is 2 + 3?", "mode": "recall", "targets": [],
        "solution": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Try this one.", "tool_calls": [tool_call("noesis_problem", problem)]}),
    );
    let solved = json!({"outcome": "solved", "rung": 0, "retest": false});
    cli.queue(
        "practitioner",
        json!({"text": "Yes!", "tool_calls": [tool_call("noesis_conclude", solved)]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({"mode": "practice"})).await;
    harness.settled().await;
    let practitioner = agent_of(&created);
    assert_sealed(&harness, practitioner, SOLUTION_SECRET);

    let sent = json!({"cmd": "noesis_send", "session": created.id, "text": "Is it 5?",
        "idempotency_key": "practice-answer"});
    harness.send_json(sent).await.expect("sent");
    harness.settled().await;
    let concluded = harness.events().iter().find_map(|event| match &event.kind {
        EventKind::NoesisPracticeConcluded { solution, .. } => Some(solution.clone()),
        _ => None,
    });
    assert_eq!(
        concluded.as_deref(),
        Some(SOLUTION_SECRET),
        "revealed once concluded"
    );
    let transcript = serde_json::to_string(&harness.transcript(practitioner)).expect("json");
    assert!(
        !transcript.contains(SOLUTION_SECRET),
        "the transcript stays sealed"
    );
    assert!(
        cli.launches()
            .iter()
            .all(|launch| !launch.prompt.contains(SOLUTION_SECRET)),
        "the engine never reads the sealed solution back"
    );
    harness.close().await;
}

/// The text deltas published for `agent`; none carries a tool's input.
fn live_text(bus: &mut tokio::sync::broadcast::Receiver<BusMessage>, agent: AgentId) -> String {
    let mut text = String::new();
    while let Ok(message) = bus.try_recv() {
        let BusMessage::Delta {
            agent: from, delta, ..
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
        if let aether_runtime::DeltaKind::Text { text: chunk } = delta {
            text.push_str(&chunk);
        }
    }
    text
}
