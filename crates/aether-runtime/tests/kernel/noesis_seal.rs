//! The transcript seal: a sealed tool runs with its real input, but neither
//! the stream nor the store ever carries the sealed fields or Noesis thinking.
use super::noesis::{Answer, Recording, noesis_harness, noesis_spec};
use super::*;

use aether_core::model::StopReason;
use aether_runtime::noesis_guard::NoesisGate;
use aether_runtime::noesis_seal::SEALED;
use aether_runtime::{BusMessage, DeltaKind};

type Inputs = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

/// A tool under any name that records the input it was run with.
#[derive(Debug)]
struct RecordInput {
    name: &'static str,
    seen: Inputs,
}

#[async_trait::async_trait]
impl aether_tools::Tool for RecordInput {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: self.name.into(),
                description: "records its input".into(),
                input_schema: json!({"type": "object"}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        self.seen.lock().unwrap().push(input);
        Ok(aether_tools::ToolExecution::Outcome(ToolOutcome::ok(
            "recorded",
        )))
    }
}

fn ask_input() -> serde_json::Value {
    json!({"question": "2+2?", "answer_key": "B", "explanation": "four is B"})
}

fn plain_input() -> serde_json::Value {
    json!({"answer_key": "visible"})
}

/// One response: a thinking block, then `noesis_ask` and `plain_probe` calls.
fn thinking_then_calls() -> aether_model::testing::ScriptedResponse {
    let mut events = vec![
        StreamEvent::MessageStart {
            provider_message_id: Some("scripted".into()),
            model: Some("scripted".into()),
        },
        StreamEvent::BlockStart {
            index: 0,
            block: BlockStart::Thinking {
                text: String::new(),
                redacted: false,
                signature: None,
            },
        },
        StreamEvent::ThinkingDelta {
            index: 0,
            text: "the secret reasoning".into(),
        },
        StreamEvent::BlockStop { index: 0 },
    ];
    for (index, (id, name, input)) in [
        (1, ("call_ask", "noesis_ask", ask_input())),
        (2, ("call_plain", "plain_probe", plain_input())),
    ] {
        events.push(StreamEvent::BlockStart {
            index,
            block: BlockStart::ToolUse {
                id: id.into(),
                name: name.into(),
                input: None,
            },
        });
        events.push(StreamEvent::ToolInputDelta {
            index,
            partial_json: input.to_string(),
        });
        events.push(StreamEvent::BlockStop { index });
    }
    events.extend([
        StreamEvent::Usage {
            usage: aether_core::model::Usage {
                input_tokens: 10,
                output_tokens: 20,
                ..Default::default()
            },
        },
        StreamEvent::Stop {
            reason: StopReason::ToolUse,
        },
        StreamEvent::MessageStop,
    ]);
    Ok(events)
}

struct Sealed {
    h: Harness,
    agent: AgentId,
    asked: Inputs,
    probed: Inputs,
    deltas: Vec<(InvocationId, u64, DeltaKind)>,
}

/// Run one agent through `thinking_then_calls`, collecting its deltas.
async fn run(noesis: bool) -> Sealed {
    let gate = NoesisGate::new();
    gate.enable(Recording::new(Answer::Admit));
    let (asked, probed) = (Inputs::default(), Inputs::default());
    let tools = (asked.clone(), probed.clone());
    let h = noesis_harness(
        vec![
            thinking_then_calls(),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        &gate,
        |_| {},
        move |registry| {
            registry.register(Arc::new(RecordInput {
                name: "noesis_ask",
                seen: tools.0,
            }));
            registry.register(Arc::new(RecordInput {
                name: "plain_probe",
                seen: tools.1,
            }));
        },
    )
    .await;
    let mut bus = h.runtime.subscribe();
    let spec = if noesis {
        noesis_spec("noesis.tutor", "ask")
    } else {
        spec("ask")
    };
    let (agent, _) = h
        .runtime
        .create_agent(spec, Actor::user())
        .expect("created");
    h.wait_until(agent.id, AgentStatus::is_terminal).await;
    let mut deltas = Vec::new();
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Delta {
            agent: of,
            invocation,
            seq,
            delta,
        } = message
            && of == agent.id
        {
            deltas.push((invocation, seq, delta));
        }
    }
    Sealed {
        h,
        agent: agent.id,
        asked,
        probed,
        deltas,
    }
}

fn tool_inputs(deltas: &[(InvocationId, u64, DeltaKind)]) -> Vec<&str> {
    deltas
        .iter()
        .filter_map(|(_, _, d)| match d {
            DeltaKind::ToolInput { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn thinking_deltas(deltas: &[(InvocationId, u64, DeltaKind)]) -> usize {
    deltas
        .iter()
        .filter(|(_, _, d)| matches!(d, DeltaKind::Thinking { .. }))
        .count()
}

/// Every invocation's seqs run 1..=n with no gap a withheld delta left.
fn assert_gapless(deltas: &[(InvocationId, u64, DeltaKind)]) {
    let mut per: std::collections::HashMap<InvocationId, Vec<u64>> = Default::default();
    for (invocation, seq, _) in deltas {
        per.entry(*invocation).or_default().push(*seq);
    }
    assert!(!per.is_empty(), "the turn streamed deltas");
    for seqs in per.values() {
        let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
        assert_eq!(seqs, &expected, "seqs are gapless");
    }
}

/// The committed assistant content blocks and requested calls of `agent`.
fn committed(h: &Harness, agent: AgentId) -> (Vec<ContentBlock>, Vec<ToolCall>) {
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent))
        .unwrap();
    let mut blocks = Vec::new();
    let mut calls = Vec::new();
    for event in events {
        match event.kind {
            EventKind::AgentMessage { content, .. } => blocks.extend(content),
            EventKind::ToolRequested { call, .. } => calls.push(call),
            _ => {}
        }
    }
    (blocks, calls)
}

/// Every snapshot and invocation record `agent`'s turns wrote, as JSON text.
fn records_text(h: &Harness, agent: AgentId) -> String {
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent))
        .unwrap();
    let mut records = Vec::new();
    for event in events {
        match event.kind {
            EventKind::SnapshotCreated { snapshot, .. } => {
                let found = h.store.snapshot(snapshot).unwrap().expect("snapshot");
                records.push(serde_json::to_value(found.messages).unwrap());
            }
            EventKind::InvocationCompleted { invocation, .. } => {
                let found = h.store.invocation(invocation).unwrap().expect("invocation");
                records.push(serde_json::to_value(found.content).unwrap());
            }
            _ => {}
        }
    }
    assert!(
        records.len() >= 3,
        "two snapshots and an invocation at least"
    );
    serde_json::to_string(&records).unwrap()
}

fn tool_use_input<'a>(blocks: &'a [ContentBlock], tool: &str) -> &'a serde_json::Value {
    blocks
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse { name, input, .. } if name == tool => Some(input),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {tool} call was committed"))
}

fn call_input<'a>(calls: &'a [ToolCall], tool: &str) -> &'a serde_json::Value {
    &calls
        .iter()
        .find(|c| c.name == tool)
        .unwrap_or_else(|| panic!("no {tool} call was requested"))
        .input
}

#[tokio::test]
async fn a_noesis_answer_runs_real_but_streams_and_persists_sealed() {
    let sealed = run(true).await;
    assert_eq!(
        *sealed.asked.lock().unwrap(),
        vec![ask_input()],
        "the tool saw the real answer"
    );
    assert_eq!(*sealed.probed.lock().unwrap(), vec![plain_input()]);

    assert!(
        !tool_inputs(&sealed.deltas).contains(&"noesis_ask"),
        "no sealed input streamed"
    );
    assert!(tool_inputs(&sealed.deltas).contains(&"plain_probe"));
    assert_eq!(
        thinking_deltas(&sealed.deltas),
        0,
        "a Noesis agent streams no thinking"
    );
    assert_gapless(&sealed.deltas);

    let (blocks, calls) = committed(&sealed.h, sealed.agent);
    let expected = json!({"question": "2+2?", "answer_key": SEALED, "explanation": SEALED});
    assert_eq!(tool_use_input(&blocks, "noesis_ask"), &expected);
    assert_eq!(call_input(&calls, "noesis_ask"), &expected);
    assert_eq!(tool_use_input(&blocks, "plain_probe"), &plain_input());
    assert_eq!(call_input(&calls, "plain_probe"), &plain_input());
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. })),
        "a Noesis agent's thinking is not persisted"
    );
    let all = sealed
        .h
        .store
        .events(0, 10_000, &EventFilter::default())
        .unwrap();
    let text = serde_json::to_string(&all).unwrap();
    assert!(
        !text.contains("four is B"),
        "the explanation is nowhere in the log"
    );
    assert!(!text.contains("the secret reasoning"));
    let records = records_text(&sealed.h, sealed.agent);
    assert!(
        !records.contains("four is B"),
        "nor in a snapshot or invocation"
    );
    assert!(!records.contains("the secret reasoning"));
    assert!(records.contains(SEALED));
}

#[tokio::test]
async fn an_import_is_sealed_like_a_live_commit() {
    let gate = NoesisGate::new();
    gate.enable(Recording::new(Answer::Admit));
    let h = noesis_harness(vec![], &gate, |_| {}, |_| {}).await;
    let mut held = noesis_spec("noesis.tutor", "imported into");
    held.start = false;
    let (tutor, _) = h
        .runtime
        .create_agent(held, Actor::user())
        .expect("created");
    let content = vec![
        ContentBlock::Thinking {
            text: "the secret reasoning".into(),
            signature: None,
            redacted: false,
        },
        ContentBlock::ToolUse {
            id: "call_ask".into(),
            name: "mcp__noesis__noesis_ask".into(),
            input: ask_input(),
        },
    ];
    let said = EventKind::AgentMessage {
        role: aether_core::model::MessageRole::Assistant,
        content,
        invocation: None,
    };
    let imported = aether_store::ImportedEvent {
        id: EventId::new(),
        draft: EventDraft::new(Actor::agent(tutor.id), said).agent(tutor.id),
        at: now(),
    };
    let written = h
        .runtime
        .apply_imported(vec![imported], vec![])
        .expect("import");
    assert_eq!(
        written.len(),
        1,
        "an emptied or sealed import keeps its event"
    );
    let (blocks, _) = committed(&h, tutor.id);
    let expected = json!({"question": "2+2?", "answer_key": SEALED, "explanation": SEALED});
    assert_eq!(
        tool_use_input(&blocks, "mcp__noesis__noesis_ask"),
        &expected
    );
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. }))
    );
}

#[tokio::test]
async fn an_ordinary_agent_keeps_its_thinking_and_only_the_sealed_tool_is_sealed() {
    let sealed = run(false).await;
    assert_eq!(*sealed.asked.lock().unwrap(), vec![ask_input()]);
    assert!(!tool_inputs(&sealed.deltas).contains(&"noesis_ask"));
    assert!(tool_inputs(&sealed.deltas).contains(&"plain_probe"));
    assert_eq!(
        thinking_deltas(&sealed.deltas),
        1,
        "ordinary thinking streams"
    );
    assert_gapless(&sealed.deltas);

    let (blocks, calls) = committed(&sealed.h, sealed.agent);
    assert!(blocks.iter().any(|b| matches!(
        b,
        ContentBlock::Thinking { text, .. } if text == "the secret reasoning"
    )));
    assert_eq!(tool_use_input(&blocks, "plain_probe"), &plain_input());
    assert_eq!(call_input(&calls, "plain_probe"), &plain_input());
    assert_eq!(
        call_input(&calls, "noesis_ask")["answer_key"],
        json!(SEALED),
        "the seal is by tool, whoever calls it"
    );
    let records = records_text(&sealed.h, sealed.agent);
    assert!(
        records.contains("the secret reasoning"),
        "ordinary thinking is kept"
    );
    assert!(!records.contains("four is B"));
}

/// Settled later from the sealed transcript, a parked call would run on
/// `"<sealed>"`: a sealed tool that needs an approval fails instead.
#[tokio::test]
async fn a_sealed_tool_that_needs_an_approval_fails_rather_than_parks() {
    let gate = NoesisGate::new();
    gate.enable(Recording::new(Answer::Admit));
    let asked = Inputs::default();
    let seen = asked.clone();
    let h = noesis_harness(
        vec![
            ScriptedClient::tool_call("noesis_ask", ask_input()),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        &gate,
        |_| {},
        move |registry| {
            registry.register(Arc::new(RecordInput {
                name: "noesis_ask",
                seen,
            }));
        },
    )
    .await;
    let mut tutor = noesis_spec("noesis.tutor", "ask");
    tutor.approval = Some(aether_core::agent::ApprovalPolicy {
        require_for: [SideEffectClass::ReadOnly].into(),
    });
    let (agent, _) = h
        .runtime
        .create_agent(tutor, Actor::user())
        .expect("created");
    assert_eq!(
        h.store.agent_subsystem(agent.id).unwrap().as_deref(),
        Some(aether_runtime::noesis_guard::NOESIS_SUBSYSTEM)
    );
    let failed = |events: &[aether_core::event::Event]| {
        events.iter().find_map(|e| match &e.kind {
            EventKind::ToolFailed { error, .. } => Some(error.clone()),
            _ => None,
        })
    };
    let read = || h.store.events(0, 10_000, &EventFilter::default()).unwrap();
    h.wait_for("the sealed call's failure", || failed(&read()).is_some())
        .await;
    let events = read();
    assert!(
        asked.lock().unwrap().is_empty(),
        "the sealed call never ran"
    );
    assert!(
        !events.iter().any(|e| matches!(
            &e.kind,
            EventKind::ApprovalRequested { tool, .. } if tool == "noesis_ask"
        )),
        "the sealed call was not parked for approval"
    );
    assert!(failed(&events).is_some_and(|error| error.contains("sealed")));
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        !text.contains("four is B"),
        "the answer is nowhere in the log"
    );
}
