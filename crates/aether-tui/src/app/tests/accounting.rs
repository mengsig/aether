//! accounting reducer regressions.
use super::*;
use aether_core::model::{ContentBlock, MessageRole};

use crate::app::transcript::NoticeKind;

#[test]
fn billing_updates_refresh_totals_without_replacing_context() {
    let (mut app, id) = app_with_agent();
    app.context_tokens.insert(id, 100_000);
    let effects = app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        10,
        id,
        EventKind::AgentUsageUpdated {
            total: Usage {
                output_tokens: 500,
                ..Default::default()
            },
            turn_usage: Usage {
                output_tokens: 500,
                ..Default::default()
            },
        },
    )))));
    assert!(
        !effects.is_empty(),
        "billing corrections must refresh the tree"
    );
    assert_eq!(app.context_tokens.get(&id), Some(&100_000));
}

#[test]
fn context_updates_can_shrink_after_compaction() {
    let (mut app, id) = app_with_agent();
    for (seq, tokens) in [(10, 100_000), (11, 2_000)] {
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
            seq,
            id,
            EventKind::Custom {
                name: "context.usage".into(),
                payload: serde_json::json!({"tokens": tokens}),
            },
        )))));
        assert_eq!(app.context_tokens.get(&id), Some(&tokens));
    }
}

/// The context bar once kept the pre-fold count until the next turn reported usage.
#[test]
fn a_finished_fold_moves_the_context_reading_at_once() {
    let (mut app, id) = app_with_agent();
    app.context_tokens.insert(id, 109_292);
    let folds = [
        (
            10,
            EventKind::CompactionNative {
                provider: "claude-code".into(),
                automatic: false,
                before_tokens: Some(109_292),
                after_tokens: Some(3_596),
                duration_ms: Some(19_000),
            },
            3_596,
        ),
        (
            11,
            EventKind::CompactionCompleted {
                checkpoint: aether_core::id::CheckpointId::new(),
                version: 1,
                from_seq: 0,
                to_seq: 9,
                before_tokens: Some(3_596),
                after_tokens: Some(1_200),
            },
            1_200,
        ),
    ];
    for (seq, kind, after) in folds {
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
            seq, id, kind,
        )))));
        assert_eq!(app.context_tokens.get(&id), Some(&after));
        assert_eq!(app.tree.nodes[&id].agent.stats.context_tokens, Some(after));
    }
}

/// An engine notice in the viewed transcript containing `needle`.
fn engine_notice_contains(app: &App, needle: &str) -> bool {
    models(app).iter().any(|m| {
        matches!(m, CellModel::Notice { kind: NoticeKind::Engine, text } if text.contains(needle))
    })
}

/// The reason a native launcher's interrupt fired is computed by the
/// engine but was once dropped by the TUI's `Custom`-event catch-all:
/// native launchers stay off, and the turn must say so visibly. It must
/// reach the chat transcript.
#[test]
fn an_engine_interrupt_reason_reaches_the_chat_transcript() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        5,
        id,
        EventKind::Custom {
            name: "engine.interrupted".into(),
            payload: serde_json::json!({
                "engine": "claude-code",
                "reason": "hit the 90s idle stall window",
            }),
        },
    )))));
    assert!(
        engine_notice_contains(&app, "hit the 90s idle stall window"),
        "{:?}",
        models(&app)
    );
}

/// A native turn waiting out an outage must read as waiting, not running.
#[test]
fn a_transient_engine_retry_reaches_the_chat_transcript() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        5,
        id,
        EventKind::Custom {
            name: "engine.transient_retry".into(),
            payload: serde_json::json!({
                "engine": "claude-code",
                "class": "network",
                "waiting_for": "network",
                "attempt": 3,
                "delay_ms": 60_000,
            }),
        },
    )))));
    assert!(
        engine_notice_contains(&app, "waiting for network · retry 3 in 60s"),
        "{:?}",
        models(&app)
    );
}

/// Same for a kill-switch bypass: the reason (the native tool the
/// engine reached for) must not be silently dropped either.
#[test]
fn a_kill_switch_bypass_reason_reaches_the_chat_transcript() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        5,
        id,
        EventKind::Custom {
            name: "engine.kill_switch_bypassed".into(),
            payload: serde_json::json!({
                "engine": "codex",
                "tool": "subagent_spawn",
            }),
        },
    )))));
    assert!(
        engine_notice_contains(&app, "subagent_spawn"),
        "{:?}",
        models(&app)
    );
}

#[test]
fn a_model_call_sets_the_context_in_use_and_its_reply_lands() {
    let (mut app, id) = app_with_agent();
    let inv = InvocationId::new();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        5,
        id,
        EventKind::InvocationCompleted {
            invocation: inv,
            snapshot: aether_core::id::SnapshotId::new(),
            model: ModelRef::new("mock", "m"),
            usage: Usage {
                input_tokens: 900,
                output_tokens: 42,
                cache_read_tokens: 100,
                cost_usd: 0.0123,
                ..Default::default()
            },
            stop_reason: None,
            duration_ms: 1_234,
        },
    )))));
    assert_eq!(app.context_tokens.get(&id), Some(&1_000));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        6,
        id,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("done")],
            invocation: Some(inv),
        },
    )))));
    assert!(
        models(&app).contains(&CellModel::Assistant {
            text: "done".into()
        }),
        "{:?}",
        models(&app)
    );
}

#[test]
fn a_refused_store_write_reaches_the_chat_transcript() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        5,
        id,
        EventKind::Custom {
            name: "store.write_failed".into(),
            payload: serde_json::json!({
                "error": "store error: database or disk is full",
                "during": "lease renewal",
                "refused_writes": 3,
            }),
        },
    )))));
    assert!(models(&app).iter().any(|model| matches!(model,
        CellModel::Notice { kind: NoticeKind::Store, text }
        if text.as_str() == "the disk refused 3 write(s) for lease renewal: store error: database or disk is full"
    )), "{:?}", models(&app));
}
