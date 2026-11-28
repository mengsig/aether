//! A native engine whose sign-in the provider refused suspends the agent on
//! `credentials` instead of failing it, says so once, and resumes on a probe.
use super::*;

pub(super) const REFUSED: &str = r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}} · Please run /login"#;

fn refused() -> Vec<EngineEvent> {
    vec![EngineEvent::Done(EngineOutcome {
        is_error: true,
        error: Some(REFUSED.into()),
        ..Default::default()
    })]
}

fn custom_events(h: &Harness, id: AgentId, name: &str) -> usize {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .iter()
        .filter(|event| matches!(&event.kind, EventKind::Custom { name: n, .. } if n == name))
        .count()
}

#[tokio::test]
async fn a_refused_sign_in_suspends_the_agent_until_a_probe_answers() {
    let engine = QueuedEngine::new(vec![refused(), refused(), answered("signed in again")]);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.exhaustion_probe_floor = Duration::from_millis(400);
        config.engine_auth_retries = 0;
    })
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let suspended = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() != AgentStatusKind::Running && a.stats.turns > 0
    })
    .await;
    let AgentStatus::Blocked {
        reason: BlockReason::Exhausted { class, detail, .. },
    } = &suspended.status
    else {
        panic!(
            "a refused sign-in must suspend, not fail: {:?}",
            suspended.status
        );
    };
    assert_eq!(*class, ExhaustionClass::Credentials);
    assert!(detail.contains("authentication_error"), "{detail}");
    let recovered = wait_for(h.store.as_ref(), id, |a| {
        custom_events(&h, a.id, "exhaustion.recovered") == 1
            && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert!(
        !matches!(
            recovered.status,
            AgentStatus::Blocked { .. } | AgentStatus::Failed { .. }
        ),
        "{:?}",
        recovered.status
    );
    assert_eq!(engine.prompts.lock().unwrap().len(), 3);
    assert_eq!(
        custom_events(&h, id, "exhaustion.suspended"),
        1,
        "the operator hears once per episode, not once per probe"
    );
    assert_eq!(custom_events(&h, id, "exhaustion.probe_failed"), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_refused_sign_in_with_a_fallback_benches_the_provider_it_leaves() {
    let engine = QueuedEngine::new(vec![refused(), answered("the fallback answered")]);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_auth_retries = 0;
    })
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(ModelRef::new("claude-code", "haiku")),
                fallbacks: vec![ModelRef::new("codex", "gpt-6.1-sol")],
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    h.runtime
        .send_message(agent.id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), agent.id, |a| {
        a.stats.turns == 1 && matches!(a.status, AgentStatus::Waiting { .. })
    })
    .await;
    assert_eq!(engine.prompts.lock().unwrap().len(), 2, "the fallback ran");
    let benched: Vec<String> = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ProviderBackoff {
                provider, reason, ..
            } => Some(format!("{provider}: {reason}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        benched,
        ["claude-code: auth from the provider"],
        "other turns must not relaunch a CLI whose sign-in was refused"
    );
    h.runtime.shutdown().await;
}

fn custom_payloads(h: &Harness, id: AgentId, name: &str) -> Vec<serde_json::Value> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name: n, payload } if n == name => Some(payload),
            _ => None,
        })
        .collect()
}

fn claude_auth(h: &Harness) -> serde_json::Value {
    let usage = h.runtime.provider_usage().unwrap();
    usage["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["provider"] == "claude-code")
        .map(|row| row["auth"].clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_refused_sign_in_relaunches_a_fresh_engine_and_continues_when_it_answers() {
    let engine = QueuedEngine::new(vec![refused(), answered("signed in on the second try")]);
    let h = harness_with_engine_config(engine.clone(), |_| {}).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let done = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert!(
        matches!(done.status, AgentStatus::Waiting { .. }),
        "{:?}",
        done.status
    );
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        2,
        "one fresh relaunch"
    );
    let retries = custom_payloads(&h, id, "engine.auth_retry");
    assert_eq!(retries.len(), 1, "{retries:?}");
    assert_eq!(retries[0]["kind"], "provider_auth");
    assert_eq!(retries[0]["attempt"], 1);
    assert_eq!(custom_events(&h, id, "exhaustion.suspended"), 0);
    assert!(claude_auth(&h).is_null(), "{}", claude_auth(&h));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_sign_in_refused_on_every_relaunch_suspends_as_provider_auth_until_a_probe_answers() {
    let mut runs: Vec<_> = (0..5).map(|_| refused()).collect();
    runs.push(answered("signed in again"));
    let engine = QueuedEngine::new(runs);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.exhaustion_probe_floor = Duration::from_millis(600);
    })
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let suspended = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > 0 && a.status.kind() == AgentStatusKind::Blocked
    })
    .await;
    let AgentStatus::Blocked { reason } = &suspended.status else {
        unreachable!("waited for blocked");
    };
    assert!(
        matches!(
            reason,
            BlockReason::Exhausted {
                class: ExhaustionClass::Credentials,
                ..
            }
        ),
        "{reason:?}"
    );
    assert!(
        reason.describe().starts_with("provider_auth: claude-code"),
        "{}",
        reason.describe()
    );
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        5,
        "four relaunches, then suspend"
    );
    assert_eq!(custom_payloads(&h, id, "engine.auth_retry").len(), 4);
    let notice = custom_payloads(&h, id, "exhaustion.suspended");
    assert_eq!(notice.len(), 1);
    assert_eq!(notice[0]["kind"], "provider_auth");
    assert_eq!(notice[0]["provider"], "claude-code");
    let detail = notice[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("refused the sign-in on 5 launches"),
        "{detail}"
    );
    assert_eq!(claude_auth(&h)["kind"], "provider_auth");
    assert_eq!(claude_auth(&h)["status"], "auth_failed");
    wait_for(h.store.as_ref(), id, |a| {
        custom_events(&h, a.id, "exhaustion.recovered") == 1
            && a.status.kind() != AgentStatusKind::Running
    })
    .await;
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        6,
        "a probe launches once: the provider is already known refused"
    );
    assert!(claude_auth(&h).is_null(), "{}", claude_auth(&h));
    h.runtime.shutdown().await;
}

fn blocked_on_credentials(agent: &aether_core::agent::Agent) -> bool {
    matches!(
        agent.status,
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                class: ExhaustionClass::Credentials,
                ..
            }
        }
    )
}

async fn send_and_suspend(h: &Harness, id: AgentId) {
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, blocked_on_credentials).await;
}

/// Any turn answered on the provider ends what every suspended row says
/// about its sign-in, and releases those agents to probe now.
#[tokio::test]
async fn an_unrelated_answer_clears_the_refusal_and_releases_every_suspended_agent() {
    let engine = QueuedEngine::new(vec![
        refused(),
        refused(),
        answered("an unrelated turn"),
        answered("first probe"),
        answered("second probe"),
    ]);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_auth_retries = 0;
        config.exhaustion_probe_floor = Duration::from_secs(600);
    })
    .await;
    let (first, second, unrelated) = (chat_agent(&h), chat_agent(&h), chat_agent(&h));
    send_and_suspend(&h, first).await;
    send_and_suspend(&h, second).await;
    assert_eq!(claude_auth(&h)["status"], "auth_failed");
    h.runtime
        .send_message(unrelated, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), unrelated, |a| {
        a.stats.turns > 0 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(claude_auth(&h).is_null(), "{}", claude_auth(&h));
    for id in [first, second] {
        // Without the release their next probe is ten minutes away.
        wait_for(h.store.as_ref(), id, |a| {
            custom_events(&h, a.id, "exhaustion.recovered") == 1
                && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
    }
    assert_eq!(engine.prompts.lock().unwrap().len(), 5);
    h.runtime.shutdown().await;
}

/// After a restart the suspended row still says the sign-in is refused, so
/// the canary's refused probe suspends again without relaunching.
#[tokio::test]
async fn after_a_restart_a_refused_probe_suspends_without_relaunching() {
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let before = QueuedEngine::new(vec![refused()]);
    let h = build_harness(
        before.clone(),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(store.clone()),
        |config| {
            config.engine_auth_retries = 0;
            config.exhaustion_probe_floor = Duration::from_millis(800);
        },
    )
    .await;
    let id = chat_agent(&h);
    send_and_suspend(&h, id).await;
    h.runtime.shutdown().await;

    let after = QueuedEngine::new(vec![refused(), refused(), refused()]);
    let h = build_harness(
        after.clone(),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(store),
        |config| config.exhaustion_probe_floor = Duration::from_secs(600),
    )
    .await;
    assert_eq!(
        claude_auth(&h)["status"],
        "auth_failed",
        "read from the row"
    );
    wait_for(h.store.as_ref(), id, |a| {
        custom_events(&h, a.id, "exhaustion.probe_failed") == 1 && blocked_on_credentials(a)
    })
    .await;
    assert_eq!(
        after.prompts.lock().unwrap().len(),
        1,
        "the probe launched once"
    );
    assert!(custom_payloads(&h, id, "engine.auth_retry").is_empty());
    h.runtime.shutdown().await;
}

/// A turn waiting to relaunch suspends as soon as another turn on the
/// provider gives up, instead of sleeping out its wait and launching again.
#[tokio::test]
async fn a_waiting_relaunch_suspends_once_another_turn_gives_up() {
    let usage_limit = vec![EngineEvent::Done(EngineOutcome {
        is_error: true,
        error: Some("You've hit your usage limit".into()),
        ..Default::default()
    })];
    // The prober's usage limit, the waiter's refusal, then the prober's refused probe.
    let engine = QueuedEngine::new(vec![usage_limit, refused(), refused(), refused()]);
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.engine_auth_retry_floor = Duration::from_secs(8);
        config.exhaustion_probe_floor = Duration::from_millis(1500);
    })
    .await;
    let (prober, waiter) = (chat_agent(&h), chat_agent(&h));
    h.runtime
        .send_message(prober, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), prober, |a| {
        a.status.kind() == AgentStatusKind::Blocked
    })
    .await;
    let started = std::time::Instant::now();
    h.runtime
        .send_message(waiter, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    // Waiting at least 5.6 s to relaunch when the probe is refused at about 1.5 s.
    wait_for(h.store.as_ref(), waiter, blocked_on_credentials).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(blocked_on_credentials(
        &h.store.agent(prober).unwrap().unwrap()
    ));
    assert_eq!(custom_payloads(&h, waiter, "engine.auth_retry").len(), 1);
    assert_eq!(
        engine.prompts.lock().unwrap().len(),
        3,
        "no relaunch after the wait"
    );
    h.runtime.shutdown().await;
}

/// The answer that ended a refusal is durable: after a restart, a row still
/// suspended from before it says nothing about the sign-in.
#[tokio::test]
async fn an_answer_still_fences_older_suspended_rows_after_a_restart() {
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let engine = QueuedEngine::new(vec![refused(), answered("an unrelated turn")]);
    let h = build_harness(
        engine.clone(),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(store.clone()),
        |config| {
            config.engine_auth_retries = 0;
            config.exhaustion_probe_floor = Duration::from_secs(600);
        },
    )
    .await;
    let (parked, unrelated) = (chat_agent(&h), chat_agent(&h));
    send_and_suspend(&h, parked).await;
    // Paused, so the release cannot let it probe before the restart.
    h.runtime.pause_agent(parked, false, Actor::user()).unwrap();
    assert_eq!(claude_auth(&h)["status"], "auth_failed");
    h.runtime
        .send_message(unrelated, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), unrelated, |a| {
        a.stats.turns > 0 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(claude_auth(&h).is_null(), "{}", claude_auth(&h));
    h.runtime.shutdown().await;

    let h = build_harness(
        QueuedEngine::new(Vec::new()),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(store),
        |_| {},
    )
    .await;
    assert!(blocked_on_credentials(
        &h.store.agent(parked).unwrap().unwrap()
    ));
    assert!(claude_auth(&h).is_null(), "{}", claude_auth(&h));
    h.runtime.shutdown().await;
}
