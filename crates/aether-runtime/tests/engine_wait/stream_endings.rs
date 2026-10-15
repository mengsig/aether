//! A native turn that ends some other way than a cancel - its own budget
//! check, an adapter's enforced stop, or a park - keeps what it streamed.
use super::stream_cancel::{
    TEXT, THINKING, assert_resumed_prompt, await_stream, emit, settle, stream,
};
use super::tui_projection::{models, projection};
use super::*;
use aether_core::id::InvocationId;
use aether_core::model::{MessageRole, ModelErrorClass, StreamEvent};
use aether_tui::app::transcript::{CellModel, NoticeKind};

const REASON: &str = "Codex used native delegation tool \"spawn_agent\"; stopped the engine";

/// The one cancelled record for `invocation`: zero usage, the partial as
/// content, an error naming why, and no assistant message added for it.
fn assert_operator_record(h: &Harness, agent: AgentId, invocation: InvocationId, error: &str) {
    let row = h
        .store
        .invocation(invocation)
        .expect("read invocation")
        .expect("the partial is durable");
    assert_eq!(row.usage, Usage::default(), "a partial must not bill again");
    assert_eq!(row.content.len(), 2, "{:?}", row.content);
    assert!(row.error.as_deref().is_some_and(|e| e.contains(error)));
    let records = events(h.store.as_ref()).into_iter().filter(|event| {
        matches!(&event.kind, EventKind::InvocationFailed { invocation: id, cancelled: true, retryable: false, partial, class, .. }
            if *id == invocation && *partial == row.content && *class == Some(ModelErrorClass::Cancelled))
    });
    assert_eq!(records.count(), 1);
    let history = h.store.transcript(agent, 0).expect("model transcript");
    assert!(history.iter().all(|entry| {
        let text = entry.message.text();
        !text.contains(TEXT) && !text.contains(THINKING)
    }));
}

/// The partial is drawn exactly once, the same way live and on reload.
fn assert_drawn_once(live: &aether_tui::App, reloaded: &aether_tui::App, agent: AgentId) {
    assert_eq!(models(live, agent), models(reloaded, agent));
    assert_drawn_once_untimed(live, reloaded, agent);
}

/// As above, but blind to how long reasoning took: only a live stream times
/// it, so a message the engine committed is timed live and not on reload.
fn assert_drawn_once_untimed(live: &aether_tui::App, reloaded: &aether_tui::App, agent: AgentId) {
    let untimed = |app| {
        let mut cells = models(app, agent);
        for cell in &mut cells {
            if let CellModel::Reasoning { duration_ms, .. } = cell {
                *duration_ms = None;
            }
        }
        cells
    };
    assert_eq!(untimed(live), untimed(reloaded));
    assert!(live.transcripts[&agent].stream.is_empty());
    let cells = models(reloaded, agent);
    let thinking = cells
        .iter()
        .filter(|cell| matches!(cell, CellModel::Reasoning { text, .. } if text == THINKING));
    let text = cells
        .iter()
        .filter(|cell| matches!(cell, CellModel::Assistant { text: t } if t == TEXT));
    assert_eq!((thinking.count(), text.count()), (1, 1), "{cells:#?}");
}

fn notices(app: &aether_tui::App, agent: AgentId, kind: NoticeKind) -> Vec<String> {
    models(app, agent)
        .into_iter()
        .filter_map(|cell| match cell {
            CellModel::Notice { kind: k, text } if k == kind => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// The turn itself finds the grant crossed, so `EngineVerdict::Budget` ends
/// it rather than the scheduler's sweep.
#[tokio::test]
async fn a_native_partial_cut_by_the_turns_own_budget_check_matches_reload() {
    let mut h = harness(Duration::from_secs(5)).await;
    let mut bus = h.runtime.subscribe();
    let agent = start_agent(&h, budgeted_spec(40_000));
    let turn = turn_of(&mut h, agent).await;
    let mut live = projection(h.store.agent(agent).unwrap().unwrap(), vec![]);
    let invocation = stream(&turn, &mut bus, &mut live, true, false).await;
    let usage = Usage {
        input_tokens: 41_000,
        ..Default::default()
    };
    emit(&turn, EngineEvent::UsageDelta { usage }).await;
    let stopped = wait_until(h.store.as_ref(), agent, |row| !row.is_live()).await;
    assert!(failed_with(&stopped).contains("token budget of 40000"));
    let history = settle(&h, &mut bus, &mut live, agent).await;
    let reloaded = projection(stopped, history);
    assert_drawn_once(&live, &reloaded, agent);
    assert_operator_record(&h, agent, invocation, "cancelled");
    h.runtime.shutdown().await;
}

/// An adapter stops the engine on its own authority (Codex reaching for its
/// native delegation tool). The partial survives, under that reason.
#[tokio::test]
async fn an_adapter_enforced_interruption_keeps_its_partial_and_its_reason() {
    for committed in [false, true] {
        let mut h = harness(Duration::from_secs(5)).await;
        let mut bus = h.runtime.subscribe();
        let agent = start_coordinator(&h);
        let turn = turn_of(&mut h, agent).await;
        let mut live = projection(h.store.agent(agent).unwrap().unwrap(), vec![]);
        let invocation = stream(&turn, &mut bus, &mut live, true, committed).await;
        let enforced = EngineOutcome {
            is_error: true,
            interrupted: true,
            error: Some(REASON.into()),
            ..Default::default()
        };
        emit(&turn, EngineEvent::Done(enforced)).await;
        let ended = wait_until(h.store.as_ref(), agent, |row| {
            row.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        let history = settle(&h, &mut bus, &mut live, agent).await;
        let session = ended.engine_session.clone();
        let reloaded = projection(ended, history);
        assert_drawn_once(&live, &reloaded, agent);
        assert_enforced_ending(&reloaded, agent);
        assert_operator_record(&h, agent, invocation, REASON);
        assert_resumed_prompt(&mut h, agent, session).await;
        h.runtime.shutdown().await;
    }
}

/// Neither a caller's cancel nor a provider failure: the engine's notice
/// carries the reason, then the turn reads as interrupted.
fn assert_enforced_ending(app: &aether_tui::App, agent: AgentId) {
    let engine = notices(app, agent, NoticeKind::Engine);
    assert_eq!(engine.len(), 1, "{engine:?}");
    assert!(engine[0].contains(REASON), "{engine:?}");
    assert_eq!(notices(app, agent, NoticeKind::Interrupted).len(), 1);
    assert!(notices(app, agent, NoticeKind::Cancelled).is_empty());
    let cells = models(app, agent);
    assert!(
        !cells
            .iter()
            .any(|cell| matches!(cell, CellModel::Error { .. })),
        "{cells:#?}"
    );
}

/// Streams after the park, then finishes the turn. With `late_commit` the
/// engine commits that message too, so nothing is left uncommitted.
async fn stream_after_park(h: &Harness, turn: &TurnStarted, tool: &str, late_commit: bool) {
    emit(
        turn,
        EngineEvent::Assistant {
            content: vec![ContentBlock::text("Before the park")],
            usage: None,
            model: None,
        },
    )
    .await;
    let input = match tool {
        "wait_for" => json!({"timeout_seconds": 3600}),
        _ => json!({"summary": "worked"}),
    };
    let (reply, is_error) = call(h, turn, tool, input).await;
    assert!(!is_error, "{reply}");
    // A wait's park asks the engine to stop once the reply reached it; a
    // completion's park has already started when the call returns.
    if tool == "wait_for" {
        wait_until_asked_to_stop(&turn.observed, Duration::from_secs(5)).await;
    }
    emit(
        turn,
        EngineEvent::Delta(StreamEvent::ThinkingDelta {
            index: 0,
            text: THINKING.into(),
        }),
    )
    .await;
    emit(
        turn,
        EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: TEXT.into(),
        }),
    )
    .await;
    if late_commit {
        let thinking = ContentBlock::Thinking {
            text: THINKING.into(),
            signature: None,
            redacted: false,
        };
        let content = vec![thinking, ContentBlock::text(TEXT)];
        let late = EngineEvent::Assistant {
            content,
            usage: None,
            model: None,
        };
        emit(turn, late).await;
    }
}

fn worker_spec() -> CreateAgentSpec {
    CreateAgentSpec {
        objective: Some("finish, then keep talking".into()),
        start: true,
        model: Some(ModelRef::new("claude-code", "haiku")),
        config: Some(AgentConfig {
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The park's own decision stands: the wait it recorded, or the result the
/// task already holds, with nothing streamed after it folded in.
fn assert_park_decision(h: &Harness, ended: &aether_core::agent::Agent, tool: &str) {
    if tool == "wait_for" {
        let deadline = |c: &WaitCondition| matches!(c, WaitCondition::Deadline { .. });
        let condition = condition_of(&ended.status);
        let any = matches!(condition, WaitCondition::Any { of } if of.iter().any(deadline));
        assert!(deadline(condition) || any, "{condition:?}");
        return;
    }
    assert_eq!(ended.status.kind(), AgentStatusKind::Completed);
    let task = ended.current_task.expect("the worker's task");
    let task = h.store.task(task).expect("read task").expect("task");
    let result = task.result().expect("the completed result");
    assert_eq!(result.summary, "worked");
}

async fn park_case(tool: &str, late_commit: bool) {
    let mut h = harness(Duration::from_secs(30)).await;
    let mut bus = h.runtime.subscribe();
    let agent = match tool {
        "wait_for" => start_coordinator(&h),
        _ => start_agent(&h, worker_spec()),
    };
    let turn = turn_of(&mut h, agent).await;
    let mut live = projection(h.store.agent(agent).unwrap().unwrap(), vec![]);
    stream_after_park(&h, &turn, tool, late_commit).await;
    let invocation = await_stream(&mut bus, &mut live, true).await;
    turn.end
        .send(())
        .expect("the engine finishes after its stop");
    let ended = wait_until(h.store.as_ref(), agent, |row| {
        matches!(
            row.status.kind(),
            AgentStatusKind::Waiting | AgentStatusKind::Completed
        )
    })
    .await;
    let history = settle(&h, &mut bus, &mut live, agent).await;
    assert_park_decision(&h, &ended, tool);
    let reloaded = projection(ended, history);
    if late_commit {
        assert_drawn_once_untimed(&live, &reloaded, agent);
    } else {
        assert_drawn_once(&live, &reloaded, agent);
    }
    if late_commit {
        let failed = events(h.store.as_ref()).into_iter().any(|event| {
            matches!(&event.kind, EventKind::InvocationFailed { invocation: id, .. } if *id == invocation)
        });
        assert!(!failed, "a committed message left a partial behind");
    } else {
        assert_operator_record(&h, agent, invocation, "after the turn ended");
    }
    let replies: Vec<String> = h
        .store
        .transcript(agent, 0)
        .expect("model transcript")
        .into_iter()
        .filter(|entry| entry.message.role == MessageRole::Assistant)
        .map(|entry| entry.message.text())
        .filter(|text| !text.is_empty())
        .collect();
    let mut expected = vec!["Before the park".to_owned()];
    if late_commit {
        expected.push(TEXT.to_owned());
    }
    assert_eq!(replies, expected);
    h.runtime.shutdown().await;
}

/// What an engine says after its turn parked is the operator's to see, but
/// never the turn's decision and never model history unless it committed it.
#[tokio::test]
async fn text_streamed_after_a_park_is_kept_as_an_operator_record() {
    for tool in ["wait_for", "complete_task"] {
        for late_commit in [false, true] {
            park_case(tool, late_commit).await;
        }
    }
}
