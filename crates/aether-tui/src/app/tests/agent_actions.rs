//! agent actions reducer regressions.
use super::*;
use aether_protocol::Delta;

use crate::app::transcript::NoticeKind;

/// Ending an agent is final, so `/cancel` asks first with "No" under the cursor.
#[test]
fn slash_cancel_asks_first_then_ends_the_agent() {
    let (mut app, id) = app_with_agent();
    assert!(app.run_command(SlashCommand::Cancel).is_empty());
    assert_eq!(app.bottom.view_stack.len(), 1, "the confirmation opens");
    assert!(
        app.reduce(key(KeyCode::Enter)).is_empty(),
        "No is the default"
    );
    assert!(app.bottom.view_stack.is_empty());

    app.run_command(SlashCommand::Cancel);
    let effects = app.reduce(key(KeyCode::Char('1')));
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::Cancel { agent, .. }, .. }] if *agent == id
    ));
    assert!(app.bottom.view_stack.is_empty());
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("requesting stop for root"))
    );
}

#[test]
fn root_agents_work_in_the_launch_directory_and_quota_is_tracked() {
    let mut app = app_with_status(TuiOptions {
        cwd: Some(PathBuf::from("/proj")),
        ..Default::default()
    });
    match &app.run_command(SlashCommand::New { objective: None })[..] {
        [
            Effect::Request {
                command: Command::CreateAgent(c),
                ..
            },
        ] => assert_eq!(
            c.scope.as_ref().and_then(|s| s.workspace_root.clone()),
            Some(PathBuf::from("/proj"))
        ),
        other => panic!("{other:?}"),
    }
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        9,
        id,
        EventKind::ProviderQuota {
            provider: "mock".into(),
            window: QuotaWindow {
                used_percent: 58,
                window_minutes: Some(300),
                resets_at: None,
            },
            usage: None,
        },
    )))));
    let quota = app
        .quotas
        .get("mock")
        .and_then(|usage| usage.preferred_window())
        .expect("quota for the agent's provider");
    assert_eq!(quota.used_percent, 58);
    assert_eq!(quota.window_label().as_deref(), Some("5h"));
}

#[test]
fn esc_interrupts_and_the_streamed_text_is_kept() {
    let (mut app, id) = app_with_agent();
    // Esc only interrupts a running agent.
    let mut running = agent(id, "root");
    running.status = AgentStatus::Running;
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(running))),
    });
    let effects = app.reduce(key(KeyCode::Esc));
    assert!(matches!(
        &effects[..],
        [Effect::Request { command: Command::Interrupt { agent , idempotency_key: None, }, then: Then::Interrupted(a) }]
            if *agent == id && *a == id
    ));
    let inv = InvocationId::new();
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation: inv,
        delta: Delta::Text {
            text: "half an ans".into(),
        },
    }));
    app.reduce(AppEvent::Reply {
        then: Then::Interrupted(id),
        result: Ok(Reply::Interrupted { was_running: true }),
    });
    assert!(
        !app.live_flash().is_some_and(|n| n.contains("nterrupt")),
        "the Interrupted cell says it once; no footer flash too: {:?}",
        app.live_flash()
    );
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        3,
        id,
        EventKind::AgentInterrupted { turn: 1 },
    )))));
    assert!(app.transcripts[&id].stream.is_empty());
    let models = models(&app);
    let n = models.len();
    assert_eq!(
        models[n - 2],
        CellModel::Assistant {
            text: "half an ans".into()
        },
        "{models:?}"
    );
    assert!(
        matches!(
            &models[n - 1],
            CellModel::Notice { kind: NoticeKind::Interrupted, text } if text.starts_with("Interrupted")
        ),
        "{models:?}"
    );
}
