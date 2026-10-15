//! questions reducer regressions.
use super::*;
use aether_core::id::{InboxItemId, MessageId};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole};

use crate::app::approval_flow::NeedsYouKind;

fn feed(app: &mut App, seq: u64, agent: AgentId, kind: EventKind) -> Vec<Effect> {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        seq, agent, kind,
    )))))
}

fn asked(question: &str) -> EventKind {
    EventKind::QuestionAsked {
        question: question.into(),
    }
}

/// The daemon's two events for the operator's reply: delivered, then consumed.
fn operator_reply(agent: AgentId, text: &str) -> [EventKind; 2] {
    let item = InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        seq: 0,
        kind: InboxKind::Message {
            message_id: MessageId::new(),
            from: Actor::user(),
            content: vec![ContentBlock::text(text)],
        },
        delivered_at: now(),
    };
    let items = vec![item.id];
    [
        EventKind::InboxDelivered { item },
        EventKind::InboxConsumed { up_to: 0, items },
    ]
}

/// An agent beside the viewed root that nobody is looking at.
fn add_asker(app: &mut App) -> AgentId {
    let asker = AgentId::new();
    app.tree.nodes.insert(
        asker,
        TreeNode {
            agent: agent(asker, "asker"),
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
        },
    );
    asker
}

fn question_text(model: &CellModel) -> Option<&str> {
    match model {
        CellModel::Question { text, .. } => Some(text),
        _ => None,
    }
}

fn needs_you_agents(app: &App) -> Vec<AgentId> {
    app.needs_you().into_iter().map(|row| row.agent).collect()
}

#[test]
fn a_question_from_the_selected_agent_rings_and_needs_the_operator() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Focus(false));

    let effects = feed(&mut app, 9, id, asked("which colour?"));

    assert!(effects.contains(&Effect::Bell), "{effects:?}");
    assert!(
        effects.iter().any(
            |e| matches!(e, Effect::Notify { title, .. } if title.contains(&app.agent_name(id)))
        ),
        "the notification names the asking agent: {effects:?}"
    );
    let models = models(&app);
    assert!(
        models
            .iter()
            .any(|m| question_text(m) == Some("which colour?")),
        "{models:?}"
    );
    let rows = app.needs_you();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].agent, id);
    assert_eq!(
        rows[0].what,
        NeedsYouKind::Question {
            first_line: "which colour?".into()
        }
    );
}

/// The approval handler had the same bug; this locks
/// the correct pattern in for `ask_user`: notify by the asking agent's
/// own name, and never move the view the operator is not looking at.
#[test]
fn a_question_from_an_unselected_agent_notifies_by_its_own_name_and_does_not_move_the_view() {
    let (mut app, id) = app_with_agent();
    let asker = add_asker(&mut app);
    app.reduce(AppEvent::Focus(false));

    let effects = feed(&mut app, 9, asker, asked("which colour?"));

    assert!(effects.contains(&Effect::Bell), "{effects:?}");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Notify { title, .. } if title.contains("asker"))),
        "the notification must name the asking agent, not the selected one: {effects:?}"
    );
    assert_eq!(app.selected, Some(id));
    assert_eq!(
        app.scroll_log.since(0).count(),
        0,
        "an unrelated agent's question must not scroll the operator's view"
    );
    let cells: Vec<_> = app.transcripts[&asker].all_cells().collect();
    assert!(
        cells
            .iter()
            .any(|c| question_text(&c.model) == Some("which colour?")),
        "{cells:?}"
    );
}

/// A question from an agent nobody is looking at must still show
/// up fleet-wide, and drop out once closed - however it closes.
#[test]
fn needs_you_tracks_an_unselected_agents_question() {
    let (mut app, id) = app_with_agent();
    let asker = add_asker(&mut app);
    assert!(app.needs_you().is_empty());

    feed(&mut app, 9, asker, asked("which colour?"));
    assert_ne!(app.selected, Some(asker));
    assert_eq!(needs_you_agents(&app), vec![asker]);
    assert!(app.needs_you_agents().contains(&asker));
    assert!(!app.needs_you_agents().contains(&id));

    for (seq, kind) in (10..).zip(operator_reply(asker, "blue")) {
        feed(&mut app, seq, asker, kind);
    }
    assert!(
        app.needs_you().is_empty(),
        "an answered question must drop out"
    );
    assert!(!app.needs_you_agents().contains(&asker));

    feed(&mut app, 9, id, asked("which size?"));
    assert_eq!(needs_you_agents(&app), vec![id]);

    feed(
        &mut app,
        11,
        id,
        EventKind::AgentCompleted { summary: None },
    );
    assert!(
        app.needs_you().is_empty(),
        "a question closed by its asker ending must drop out"
    );
    assert!(!app.needs_you_agents().contains(&id));
}

/// A question raised by an agent nobody was looking at must still be
/// there the first time the operator opens that agent, in order before
/// the reply that followed it.
#[test]
fn a_question_survives_its_agents_history_loading_afterwards() {
    let (mut app, _) = app_with_agent();
    let asker = add_asker(&mut app);
    feed(&mut app, 9, asker, asked("which colour?"));

    let effects = app.select(asker);
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::GetEvents { .. },
            then: Then::Backfill { agent: a, .. },
        } if *a == asker
    )));
    let history = vec![
        event(9, asker, asked("which colour?")),
        event(
            11,
            asker,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("thanks - blue it is")],
                invocation: None,
            },
        ),
    ];
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, asker),
        result: Ok(Reply::Events(history)),
    });

    let models = models(&app);
    let question_at = models
        .iter()
        .position(|m| question_text(m) == Some("which colour?"))
        .unwrap_or_else(|| panic!("question dropped when the history loaded: {models:?}"));
    let reply_at = models
        .iter()
        .position(|m| matches!(m, CellModel::Assistant { text } if text.contains("blue")))
        .expect("the loaded message is present");
    assert!(
        question_at < reply_at,
        "the question must render before the reply that answers it: {models:?}"
    );
}

/// A question that arrives live while the history is still loading is
/// replayed over the rebuilt transcript, not lost with the old one.
#[test]
fn a_question_asked_while_history_loads_is_kept() {
    let (mut app, _) = app_with_agent();
    let asker = add_asker(&mut app);
    app.select(asker);
    feed(&mut app, 9, asker, asked("which colour?"));
    let history = vec![event(
        8,
        asker,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("let me ask")],
            invocation: None,
        },
    )];
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, asker),
        result: Ok(Reply::Events(history)),
    });
    let models = models(&app);
    let texts: Vec<_> = models.iter().filter_map(question_text).collect();
    assert_eq!(texts, vec!["which colour?"], "{models:?}");
    assert!(
        matches!(&models[0], CellModel::Assistant { text } if text == "let me ask"),
        "{models:?}"
    );
}

/// A turn that asks and then ends leaves a question nobody can
/// answer - `send_message` refuses a terminal agent. The cell must stop
/// awaiting a reply, and bump its revision so its layout is redone.
#[test]
fn a_question_stops_awaiting_a_reply_once_its_asker_ends() {
    for ending in [
        EventKind::AgentCompleted {
            summary: Some("done".into()),
        },
        EventKind::AgentFailed {
            error: "broke".into(),
        },
        EventKind::AgentCancelled {
            reason: Some("no longer needed".into()),
        },
    ] {
        let (mut app, id) = app_with_agent();
        ask_a_question(&mut app, id);
        let before = the_question(&app).revision;

        feed(&mut app, 10, id, ending);

        let question = the_question(&app);
        assert!(matches!(
            question.model,
            CellModel::Question {
                state: crate::app::transcript::QuestionState::AskerEnded,
                ..
            }
        ));
        assert!(!question_answered(&app));
        for width in [80, 120] {
            let rows =
                crate::ui::cells::ChatCell::display_lines(&question.model, width, &app.theme);
            let text = rows
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("unanswered; this agent has ended"), "{text}");
            assert!(!text.contains("reply below"));
        }
        let plain = crate::plain::describe(&question);
        assert_eq!(
            plain.last().map(String::as_str),
            Some("unanswered; this agent has ended")
        );
        assert!(
            the_question(&app).revision > before,
            "the revision must change or the cached lines stay on screen"
        );
    }
}

/// Nor does a question keep asking once the operator has answered
/// it - the agent has been woken and is running on that answer.
#[test]
fn a_question_stops_awaiting_a_reply_once_the_operator_answers() {
    let (mut app, id) = app_with_agent();
    ask_a_question(&mut app, id);
    for (seq, kind) in (10..).zip(operator_reply(id, "blue")) {
        feed(&mut app, seq, id, kind);
    }
    assert!(
        question_answered(&app),
        "still awaiting after the answer arrived: {:?}",
        models(&app)
    );

    // The asker finishing the work it was told to do must not reopen it.
    feed(
        &mut app,
        20,
        id,
        EventKind::AgentCompleted {
            summary: Some("painted it blue".into()),
        },
    );
    assert!(
        question_answered(&app),
        "the ending reopened an answered question: {:?}",
        models(&app)
    );
}

/// A reload rebuilds every cell from the event log, so it has to work out
/// on its own that the answer came after the question. The answer's live
/// frames are deliberately absent: only the history carries them.
#[test]
fn a_reloaded_transcript_sees_that_a_question_was_answered() {
    let (mut app, _) = app_with_agent();
    let asker = add_asker(&mut app);
    feed(&mut app, 9, asker, asked("which colour?"));
    app.select(asker);
    let mut history = vec![event(9, asker, asked("which colour?"))];
    history.extend(
        (10..)
            .zip(operator_reply(asker, "blue"))
            .map(|(seq, kind)| event(seq, asker, kind)),
    );
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, asker),
        result: Ok(Reply::Events(history)),
    });
    let models = models(&app);
    assert!(
        models
            .iter()
            .any(|m| matches!(m, CellModel::User { text, .. } if text == "blue")),
        "the answer itself is in the transcript: {models:?}"
    );
    assert!(
        question_answered(&app),
        "the reload brought the answered question back as open: {models:?}"
    );
}
