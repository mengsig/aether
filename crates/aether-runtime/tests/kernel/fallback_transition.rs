//! fallback transition scenarios and fixtures.
use super::*;

/// §4.5: a retryable failure moves the turn to the next link of the chain,
/// says so as an event, and the invocation records the link that answered.
#[tokio::test]
async fn a_rate_limited_link_hands_the_turn_to_the_next_one() {
    let h = harness(vec![
        ScriptedClient::error(ModelErrorClass::RateLimit, "slow down"),
        ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "answered on the fallback"}),
        ),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
                ..spec("fall back when the first link is busy")
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;

    let events = h
        .store
        .events(0, 10_000, &EventFilter::all())
        .expect("events");
    let fallback = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ProviderFallback {
                from,
                to,
                reason,
                class,
            } => Some((from.clone(), to.clone(), reason.clone(), *class)),
            _ => None,
        })
        .expect("provider.fallback");
    assert_eq!(fallback.0, ModelRef::new("anthropic", "claude-sonnet-5-5"));
    assert_eq!(fallback.1, ModelRef::new("groq", "openai/gpt-oss-120b"));
    assert_eq!(fallback.2, "rate limited");
    assert_eq!(fallback.3, Some(ModelErrorClass::RateLimit));

    // The turn is recorded against the link that answered, so cost and usage
    // land on the provider that actually did the work.
    let completed = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::InvocationCompleted { model, .. } => Some(model.clone()),
            _ => None,
        })
        .expect("invocation.completed");
    assert_eq!(completed, ModelRef::new("groq", "openai/gpt-oss-120b"));
    let invocations = h
        .store
        .invocations_for_agent(agent.id)
        .expect("invocations");
    let answered = invocations
        .iter()
        .find(|i| i.error.is_none())
        .expect("one invocation answered");
    assert_eq!(answered.model, ModelRef::new("groq", "openai/gpt-oss-120b"));
    // The failed attempt is still on the record, against its own link.
    assert!(
        invocations.iter().any(
            |i| i.error.is_some() && i.model == ModelRef::new("anthropic", "claude-sonnet-5-5")
        ),
        "the failed link is not recorded: {invocations:?}"
    );
    // Both requests really went out, and the second named the fallback.
    let sent: Vec<ModelRef> = h
        .client
        .requests()
        .iter()
        .map(|r| r.model.clone())
        .collect();
    assert_eq!(
        sent,
        vec![
            ModelRef::new("anthropic", "claude-sonnet-5-5"),
            ModelRef::new("groq", "openai/gpt-oss-120b"),
        ]
    );
    assert!(h.client.requests().len() >= 2);
}

/// §4.5: provider health is what a person reads to answer "why is this
/// provider not carrying my work". A failure and a chain move must both show
/// up there, not only in the log.
#[tokio::test]
async fn provider_health_carries_recent_error_classes_and_the_last_fallback() {
    let h = harness(vec![
        ScriptedClient::error(ModelErrorClass::RateLimit, "slow down"),
        ScriptedClient::tool_call("complete_task", json!({"summary": "answered"})),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
                ..spec("surface what the provider did")
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;

    let health = h.runtime.provider_health().expect("provider health");
    let failed = health
        .iter()
        .find(|p| p.provider == "anthropic")
        .expect("the provider that failed is listed");
    assert_eq!(
        failed.recent_errors.len(),
        1,
        "one class seen once: {:?}",
        failed.recent_errors
    );
    assert_eq!(failed.recent_errors[0].class, ModelErrorClass::RateLimit);
    assert_eq!(failed.recent_errors[0].count, 1);
    let fallback = failed
        .last_fallback
        .as_ref()
        .expect("the chain move is on the provider it left");
    assert_eq!(fallback.to, ModelRef::new("groq", "openai/gpt-oss-120b"));
    assert_eq!(fallback.reason, "rate limited");
    assert_eq!(fallback.class, Some(ModelErrorClass::RateLimit));

    // The link that answered is clean: a fallback is not a mark against it.
    let answered = h
        .runtime
        .provider_health_for("groq")
        .expect("provider health");
    assert!(answered.recent_errors.is_empty(), "{answered:?}");
    assert!(answered.last_fallback.is_none(), "{answered:?}");

    // A provider that has done nothing still has a row for a listing.
    let idle = h
        .runtime
        .provider_health_for("openai")
        .expect("provider health");
    assert_eq!(idle.provider, "openai");
    assert_eq!(idle.running, 0);
    assert!(idle.recent_errors.is_empty());
}

/// A failure the next link cannot survive is not a reason to burn the chain.
#[tokio::test]
async fn an_unretryable_failure_does_not_walk_the_chain() {
    let h = harness(vec![ScriptedClient::error(
        ModelErrorClass::InvalidRequest,
        "malformed tool schema",
    )])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
                ..spec("do not fall back on a bad request")
            },
            Actor::user(),
        )
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(matches!(status, AgentStatus::Failed { .. }), "{status:?}");
    assert!(
        !types(h.store.as_ref(), agent.id).contains(&"provider.fallback".to_string()),
        "an invalid request must not be retried on another provider"
    );
    assert_eq!(h.client.requests().len(), 1);
}

/// A chain keeps an agent schedulable while its primary is backed off: the
/// next turn skips the held-back link instead of waiting behind it.
#[tokio::test]
async fn a_backed_off_primary_is_skipped_on_the_next_turn() {
    let h = harness(vec![
        // The retry-after keeps the primary benched for the whole test: with
        // the default 5s window a slow tool call outlives it, and the second
        // turn then picks the primary because it is free, not because the
        // backoff was ignored.
        Err(
            aether_core::model::ModelError::new(ModelErrorClass::RateLimit, "slow down")
                .with_retry_after(Duration::from_secs(60)),
        ),
        ScriptedClient::tool_call("bash", json!({"command": "echo parked > parked.txt"})),
        ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "answered past the backoff"}),
        ),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
                ..spec("run past a backed off primary")
            },
            Actor::user(),
        )
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );
    let sent: Vec<ModelRef> = h
        .client
        .requests()
        .iter()
        .map(|r| r.model.clone())
        .collect();
    assert_eq!(
        sent,
        vec![
            ModelRef::new("anthropic", "claude-sonnet-5-5"),
            ModelRef::new("groq", "openai/gpt-oss-120b"),
            // Second turn: anthropic is still backed off, so it is not called.
            ModelRef::new("groq", "openai/gpt-oss-120b"),
        ]
    );
    let skips = h
        .store
        .events(0, 10_000, &EventFilter::all())
        .expect("events")
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::ProviderFallback { class: None, .. }))
        .count();
    assert_eq!(
        skips, 1,
        "the skipped link must be visible as provider.fallback"
    );
}

/// Admission and execution must agree on the link. With `max_concurrent = 1`
/// on anthropic and both agents chained anthropic,groq, the scheduler admits
/// the second turn on groq - so the turn has to run there, or a provider
/// capped at one carries two.
#[tokio::test]
async fn a_capped_primary_sends_the_second_turn_to_the_fallback() {
    let h = harness_full(
        vec![slow_turn("one"), slow_turn("two")],
        |_| {},
        |c| c.max_concurrent_turns = 4,
        Some("[providers.anthropic]\nmax_concurrent = 1\n"),
        None,
        None,
        |_| {},
    )
    .await;
    h.client.set_delay(Duration::from_millis(150));
    let chained = |objective: &str| CreateAgentSpec {
        model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
        ..spec(objective)
    };
    let (a, _) = h
        .runtime
        .create_agent(chained("part one"), Actor::user())
        .expect("a");
    let (b, _) = h
        .runtime
        .create_agent(chained("part two"), Actor::user())
        .expect("b");
    h.wait_until(a.id, |s| s.is_terminal()).await;
    h.wait_until(b.id, |s| s.is_terminal()).await;

    let sent: Vec<ModelRef> = h
        .client
        .requests()
        .iter()
        .map(|r| r.model.clone())
        .collect();
    assert_eq!(
        sent.iter().filter(|m| m.provider == "anthropic").count(),
        1,
        "a provider capped at one turn carried more than one: {sent:?}"
    );
    assert!(
        sent.iter().any(|m| m.provider == "groq"),
        "the capped link was not handed to the fallback: {sent:?}"
    );
    let fell_back = h
        .store
        .events(0, 10_000, &EventFilter::all())
        .expect("events")
        .iter()
        .any(|e| {
            matches!(&e.kind, EventKind::ProviderFallback { from, to, reason, .. }
                if from.provider == "anthropic"
                    && to.provider == "groq"
                    && reason.contains("limit"))
        });
    assert!(fell_back, "the move off the capped link was not recorded");
}

/// A persisted agent whose chain is seeded straight into the store, so a link
/// can be unusable at run time the way a real one becomes unusable: the
/// provider was disabled, or its credential removed, after the agent was made.
fn seed_ready_with_chain(
    store: &dyn Store,
    id: AgentId,
    model: ModelRef,
    fallbacks: Vec<ModelRef>,
) {
    let at = now();
    let mut agent = aether_core::agent::Agent {
        archived: false,
        id,
        parent: None,
        role: "worker".into(),
        name: "w".into(),
        scope: Scope::default(),
        model,
        fallbacks,
        status: AgentStatus::Created,
        paused: false,
        paused_via: None,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig {
            approval: ApprovalPolicy::none(),
            ..Default::default()
        },
        stats: Default::default(),
        run: Default::default(),
        created_at: at,
        updated_at: at,
    };
    agent.transition(AgentStatus::Ready, at).expect("ready");
    let mut commit = Commit::new().event(
        EventDraft::new(
            Actor::user(),
            EventKind::AgentMessage {
                role: aether_core::model::MessageRole::User,
                content: vec![ContentBlock::text("answer me")],
                invocation: None,
            },
        )
        .agent(id),
    );
    commit.push_mutation(Mutation::UpsertAgent(agent));
    store.commit(commit).expect("seed");
}

/// The first event of a type anywhere in the fleet.
async fn wait_for_kind(store: &dyn Store, type_name: &str) -> aether_core::event::Event {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(e) = store
            .events(0, 10_000, &EventFilter::all())
            .expect("events")
            .into_iter()
            .find(|e| e.type_name() == type_name)
        {
            return e;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {type_name}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// §4.5: a link that cannot be built - here a provider that ships disabled -
/// is skipped for the next one, not turned into a failed turn.
#[tokio::test]
async fn an_unusable_link_is_skipped_while_the_chain_has_more() {
    let id = AgentId::new();
    let h = harness_with(
        vec![ScriptedClient::text("answered on the usable link")],
        |s| {
            seed_ready_with_chain(
                s,
                id,
                ModelRef::new("openai-codex", "sol"),
                vec![ModelRef::new("anthropic", "claude-sonnet-5-5")],
            )
        },
    )
    .await;

    let event = wait_for_kind(h.store.as_ref(), "provider.fallback").await;
    let EventKind::ProviderFallback {
        from, to, reason, ..
    } = &event.kind
    else {
        panic!("not a fallback: {:?}", event.kind);
    };
    assert_eq!(from, &ModelRef::new("openai-codex", "sol"));
    assert_eq!(to, &ModelRef::new("anthropic", "claude-sonnet-5-5"));
    assert!(reason.contains("unusable"), "{reason}");

    // The turn really ran, and only on the link that could carry it.
    let sent: Vec<ModelRef> = h
        .client
        .requests()
        .iter()
        .map(|r| r.model.clone())
        .collect();
    assert_eq!(sent, vec![ModelRef::new("anthropic", "claude-sonnet-5-5")]);
    assert!(
        !types(h.store.as_ref(), id).contains(&"invocation.failed".to_string()),
        "an unusable link must not be recorded as a failed invocation"
    );
}

/// The other half: when no link is usable the turn fails, it does not hang.
#[tokio::test]
async fn a_chain_with_no_usable_link_fails_the_turn() {
    let id = AgentId::new();
    let h = harness_with(vec![], |s| {
        seed_ready_with_chain(
            s,
            id,
            ModelRef::new("openai-codex", "sol"),
            vec![ModelRef::new("openai-codex", "astra")],
        )
    })
    .await;
    let status = h.wait_until(id, |s| s.is_terminal()).await;
    assert!(matches!(status, AgentStatus::Failed { .. }), "{status:?}");
    assert!(h.client.requests().is_empty(), "no link could be called");
}

/// VISION §10: a provider's error text reaches durable records, so a rejected
/// key must not travel with it. The error here is built by the real
/// `map_http_error` from a 401 body that quotes the key, then walked through
/// the runtime; every surface that stores it is greped for the secret.
#[tokio::test]
async fn a_rejected_key_never_reaches_a_durable_record() {
    const PLANTED: &str = "sk-proj-NOTAREALKEYbutkeyshaped0123456789";
    let body = format!(
        r#"{{"error":{{"message":"Incorrect API key provided: {PLANTED}. You can find your API key at https://platform.openai.com/account/api-keys.","type":"invalid_request_error","code":"invalid_api_key"}}}}"#
    );
    let rejected = aether_model::http::map_http_error(401, &Default::default(), &body);
    let h = harness(vec![Err(rejected)]).await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("fail on a rejected key"), Actor::user())
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;

    let mut surfaces: Vec<(String, String)> = vec![("agent status".into(), format!("{status:?}"))];
    for e in h
        .store
        .events(0, 10_000, &EventFilter::all())
        .expect("events")
    {
        surfaces.push((format!("event {}", e.type_name()), format!("{:?}", e.kind)));
    }
    for i in h
        .store
        .invocations_for_agent(agent.id)
        .expect("invocations")
    {
        surfaces.push(("invocation row".into(), format!("{i:?}")));
    }
    for (what, text) in &surfaces {
        assert!(
            !text.contains(PLANTED),
            "the rejected key leaked into the {what}: {text}"
        );
    }
    // Guard against a vacuous pass: the masked error must really be stored.
    assert!(
        surfaces.iter().any(|(_, t)| t.contains("REDACTED")),
        "the rejected-key error never reached a durable record, so nothing was proven"
    );
}
