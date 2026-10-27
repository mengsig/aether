//! usage reducer regressions.
use super::*;

#[test]
fn slash_commands_produce_requests_and_panels() {
    let (mut app, id) = app_with_agent();
    for c in "/effort max".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        matches!(&effects[0], Effect::Request { command: Command::UpdateAgent { id: a, patch, subtree: false, idempotency_key: None, }, then: Then::AgentPatched } if *a == id && patch.effort == Some(aether_core::model::Effort::Max))
    );
    for c in "/help".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    app.reduce(key(KeyCode::Enter));
    assert!(app.bottom.shortcuts, "/help shows the shortcuts panel");
    app.reduce(key(KeyCode::Esc));
    assert!(!app.bottom.shortcuts);
    for c in "/new ship it".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        matches!(&effects[0], Effect::Request { command: Command::CreateAgent(req), then: Then::AgentCreated(_) } if req.role == Role::ASSISTANT)
    );
    // The objective is the new agent's first message.
    let id = AgentId::new();
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(&effects),
        result: Ok(Reply::Agent(agent(id, "assistant"))),
    });
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request { command: Command::SendMessage { to, content, .. }, .. }
            if *to == id && content == "ship it"
    )));
}

/// `/report` once had no way in from the chat input, though the daemon
/// carried `GetRunReport`/`RunReport`. It must
/// fetch the selected agent's run and show the digest once it lands.
#[test]
fn report_opens_and_requests_the_selected_agents_run() {
    let (mut app, id) = app_with_agent();
    let run = app.tree.nodes[&id].agent.run;
    for c in "/report".chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(app.bottom.view_stack.len(), 1, "/report opens its view");
    assert!(
        app.report.loading,
        "the fetch is in flight as soon as it opens"
    );
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request { command: Command::GetRunReport { run: r }, then: Then::RunReport(t) }
                if *r == run && *t == run
        )),
        "must ask the daemon for this agent's own run: {effects:?}"
    );

    let report = Box::new(aether_core::report::RunReport {
        schema: aether_core::report::REPORT_SCHEMA,
        run,
        generated_at: now(),
        from_seq: 0,
        to_seq: 3,
        provenance: None,
        body: aether_core::report::RunBody {
            objective: Some("ship it".into()),
            outcome: aether_core::report::RunOutcome::Completed,
            started_at: now(),
            ended_at: Some(now()),
            duration_ms: 1_000,
            usage: Usage::default(),
            events: 3,
            agents: Vec::new(),
            tasks: Vec::new(),
            tools: Vec::new(),
            approvals: Vec::new(),
            failures: Vec::new(),
            artifacts: Vec::new(),
            open_loops: Vec::new(),
            timeline: Vec::new(),
        },
    });
    app.reduce(AppEvent::Reply {
        then: Then::RunReport(run),
        result: Ok(Reply::RunReport(report)),
    });
    assert!(!app.report.loading);
    let text = crate::testing::render_to(&app, 100, 30);
    assert!(text.contains("ship it"), "{text}");
    assert!(text.contains("completed"), "{text}");
}

#[test]
fn usage_keeps_newer_events_when_an_older_refresh_arrives() {
    use aether_core::provider::{ProviderHealth, ProviderUsage};
    let (mut app, id) = app_with_agent();
    let at = now();
    let old = ProviderUsage::from_engine_payload(
        &serde_json::json!({"primary":{"usedPercent":12}}),
        at - std::time::Duration::from_secs(5),
    )
    .unwrap();
    let new =
        ProviderUsage::from_engine_payload(&serde_json::json!({"primary":{"usedPercent":60}}), at)
            .unwrap();
    let provider = aether_protocol::ProviderInfo {
        headroom: Default::default(),
        provider: "codex".into(),
        name: "Codex".into(),
        protocol: "codex-app".into(),
        family: None,
        base_url: String::new(),
        engine: true,
        models: 1,
        open_model_list: true,
        enabled: true,
        configured: true,
        auth_source: Some("codex login".into()),
        health: ProviderHealth {
            usage: Some(old.clone()),
            quota: old.preferred_window(),
            ..ProviderHealth::idle("codex")
        },
    };
    app.on_reply(Then::Usage, Ok(Reply::Providers(vec![provider.clone()])));
    app.on_event(event(
        1,
        id,
        EventKind::ProviderQuota {
            provider: "codex".into(),
            window: new.preferred_window().unwrap(),
            usage: Some(new.clone()),
        },
    ));
    app.on_reply(Then::Usage, Ok(Reply::Providers(vec![provider])));
    app.on_event(event(
        2,
        id,
        EventKind::ProviderQuota {
            provider: "codex".into(),
            window: old.preferred_window().unwrap(),
            usage: Some(old),
        },
    ));
    assert_eq!(app.providers[0].health.usage.as_ref(), Some(&new));
    assert_eq!(app.providers[0].health.quota, new.preferred_window());
    assert_eq!(app.quotas.get("codex"), Some(&new));
}

#[test]
fn usage_refresh_failures_are_visible_and_retryable() {
    let (mut app, _) = app_with_agent();
    app.run_slash(SlashCommand::Usage);
    assert!(app.usage_loading);
    app.on_reply(Then::Usage, Err(ClientError::Disconnected));
    assert!(!app.usage_loading);
    assert!(app.usage_error.is_some());
    let text = crate::testing::render_to(&app, 100, 30);
    assert!(text.contains("Refresh failed"), "{text}");
    let effects = app.reduce(key(KeyCode::Char('r')));
    assert!(app.usage_loading);
    assert!(app.usage_error.is_none());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::Providers,
            then: Then::Usage
        }
    )));
    app.on_reply(Then::Usage, Ok(Reply::Providers(vec![])));
    assert!(!app.usage_loading);
    app.reduce(key(KeyCode::Esc));
    assert!(app.bottom.view_stack.is_empty());
}
