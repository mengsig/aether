//! provider probes scenarios and fixtures.
use super::*;

// ---------------------------------------------------------------------------
// Recovery and one probe per provider
// ---------------------------------------------------------------------------

/// A spent link that also says when to come back, so the bench it lays on the
/// provider lifts with the probe instead of on the configured floor.
fn spent_until(after: Duration) -> aether_model::testing::ScriptedResponse {
    Err(
        aether_core::model::ModelError::new(ModelErrorClass::UsageLimit, "5-hour limit reached")
            .with_retry_after(after),
    )
}

/// Another agent already parked on `model`'s provider. It gets no episode of
/// its own: the canary logic reads rows, and this keeps the arithmetic of the
/// test in the test rather than in a second scripted failure.
fn park_on_exhaustion(h: &Harness, id: AgentId, model: ModelRef, next_probe: Timestamp) {
    let mut agent = h.store.agent(id).expect("agent").expect("exists");
    agent.status = AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            model,
            class: aether_core::agent::ExhaustionClass::UsageLimit,
            since: now(),
            reported_reset: None,
            next_probe,
            probes: 0,
            detail: "parked by the test".into(),
        },
    };
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .expect("park");
}

/// A suspended child parked for the rest of the test, next to `h`'s own agent.
async fn extra_parked_agent(h: &Harness, objective: &str, next_probe: Timestamp) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..spec(objective)
            },
            Actor::user(),
        )
        .expect("create");
    park_on_exhaustion(
        h,
        agent.id,
        ModelRef::new("anthropic", "claude-sonnet-5-5"),
        next_probe,
    );
    agent.id
}

fn child_resumptions(store: &dyn Store, id: AgentId) -> Vec<AgentId> {
    store
        .inbox(id, 0)
        .expect("inbox")
        .into_iter()
        .filter_map(|item| match item.kind {
            aether_core::inbox::InboxKind::ChildResumed { agent } => Some(agent),
            _ => None,
        })
        .collect()
}

/// Design test 3: the provider answering is news for whoever was told about
/// the suspension, but it is not an interruption - the ancestor hears it on
/// its own next turn, not by having this one cut short.
#[tokio::test]
async fn recovery_tells_the_suspensions_ancestors_without_interrupting_them() {
    let h = harness_configured(
        vec![ScriptedClient::tool_call(
            "delegate",
            json!({
                "model": "groq/openai/gpt-oss-120b", "effort": "low", "role": "worker",
                "objective": "do the sub task", "wait": true
            }),
        )],
        |_| {},
        |config| {
            config.exhaustion_probe_floor = Duration::from_millis(20);
            config.max_concurrent_turns = 2;
        },
    )
    .await;
    h.client
        .push_for("groq", spent_until(Duration::from_millis(200)));
    h.client.push_for(
        "groq",
        ScriptedClient::tool_call("wait_for", json!({"timeout_seconds": 3600})),
    );
    // Waking the parent must take an urgent item, not any item: it parks on a
    // message from an idle agent that is never asked anything, so never speaks.
    let (silent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                objective: None,
                name: Some("silent".into()),
                ..spec("unused")
            },
            Actor::user(),
        )
        .expect("silent agent");
    // Woken by the urgent suspension, it parks again.
    h.client.push(ScriptedClient::tool_call(
        "wait_for",
        json!({"message_from": silent.id.to_string()}),
    ));
    let (parent, _) = h
        .runtime
        .create_agent(spec("delegate and wait"), Actor::user())
        .expect("parent");
    let child = child_of(&h, parent.id).await;
    h.wait_until(child, blocked_on_exhaustion).await;

    // The probe answers: the episode closes once, and the parent is told once.
    h.wait_for("the child to resume", || {
        !exhaustion_records(h.store.as_ref(), child, "recovered").is_empty()
    })
    .await;
    let recovered = exhaustion_records(h.store.as_ref(), child, "recovered");
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    let payload = &recovered[0].1;
    assert_eq!(payload["provider"], json!("groq"));
    assert_eq!(payload["model"], json!("openai/gpt-oss-120b"));
    assert_eq!(payload["probes"], json!(0));
    assert!(payload["since"].is_string(), "{payload}");
    assert_eq!(payload["notified_ancestors"], json!([parent.id]));
    assert_eq!(child_resumptions(h.store.as_ref(), parent.id), vec![child]);
    assert_eq!(
        child_suspensions(h.store.as_ref(), parent.id).len(),
        1,
        "one suspension, one resumption"
    );

    // The parent took one turn to delegate and one for the urgent suspension.
    // Several ticks must pass without the resumption buying a third.
    let until = tokio::time::Instant::now() + Duration::from_millis(1_500);
    while tokio::time::Instant::now() < until {
        let agent = h.store.agent(parent.id).expect("agent").expect("exists");
        assert!(
            matches!(agent.status, AgentStatus::Waiting { .. }),
            "a resumption must not interrupt a wait: {:?}",
            agent.status
        );
        assert_eq!(agent.stats.turns, 2, "no extra turn was taken");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Test 9: one agent per provider goes first. The rest are held back while it
/// is in flight and follow one tick at a time once it gets through.
#[tokio::test]
async fn one_probe_per_provider_then_the_rest_follow() {
    let h = harness_configured(
        vec![
            spent_until(Duration::from_millis(200)),
            ScriptedClient::tool_call("complete_task", json!({"summary": "the canary got in"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "and so did the next"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "and the last"})),
        ],
        |_| {},
        |config| {
            config.exhaustion_probe_floor = Duration::from_millis(20);
            // Slots to spare: the serialization below is the canary's doing.
            config.max_concurrent_turns = 3;
        },
    )
    .await;
    // Parked well past the canary's own probe, so only recovery releases them.
    let later = now() + chrono::Duration::seconds(3_600);
    let (_, canary, _) = parent_and_suspending_child(&h, spec("probe for everyone")).await;
    let others = [
        extra_parked_agent(&h, "wait for the canary", later).await,
        extra_parked_agent(&h, "wait for the canary too", later).await,
    ];
    h.wait_until(canary, blocked_on_exhaustion).await;
    assert_eq!(
        h.runtime
            .provider_health_for("anthropic")
            .expect("provider health")
            .suspended_agents,
        3,
        "every agent parked on the provider is counted against it"
    );

    // The second agent through is held mid-turn: the third must stay parked
    // for as long as a probe is in flight on the provider.
    let release = h.client.hold_response_at(3);
    h.wait_until(canary, |s| s.is_terminal()).await;
    h.wait_for("one of the others to be released", || {
        others
            .iter()
            .any(|id| !blocked_on_exhaustion(&h.store.agent(*id).expect("a").expect("e").status))
    })
    .await;
    let until = tokio::time::Instant::now() + Duration::from_millis(750);
    while tokio::time::Instant::now() < until {
        let parked = others
            .iter()
            .filter(|id| blocked_on_exhaustion(&h.store.agent(**id).expect("a").expect("e").status))
            .count();
        assert_eq!(parked, 1, "only one probe at a time on a spent provider");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    release.cancel();

    for id in others {
        let status = h.wait_until(id, |s| s.is_terminal()).await;
        assert!(
            matches!(status, AgentStatus::Completed { .. }),
            "{status:?}"
        );
    }
    assert_eq!(
        h.client.peak_in_flight("anthropic"),
        1,
        "the provider was never asked twice at once"
    );
}

/// A probe that fails costs only the agent that took it: everyone else keeps
/// the schedule they were parked on.
#[tokio::test]
async fn a_failed_probe_reschedules_only_the_canary() {
    let h = harness_configured(
        vec![
            spent_until(Duration::from_millis(150)),
            spent_until(Duration::from_secs(3_600)),
        ],
        |_| {},
        |config| config.exhaustion_probe_floor = Duration::from_millis(20),
    )
    .await;
    let later = now() + chrono::Duration::seconds(3_600);
    let (_, canary, _) = parent_and_suspending_child(&h, spec("probe and fail")).await;
    let other = extra_parked_agent(&h, "stay parked", later).await;
    let first = schedule_of(&exhausted_reason(
        &h.wait_until(canary, blocked_on_exhaustion).await,
    ));

    h.wait_for("the probe to fail", || {
        !exhaustion_records(h.store.as_ref(), canary, "probe_failed").is_empty()
    })
    .await;
    let after = schedule_of(&exhausted_reason(
        &h.store.agent(canary).expect("a").expect("e").status,
    ));
    assert_eq!(after.1, 1, "the canary carries the failed probe");
    assert!(after.0 > first.0, "{after:?} did not move past {first:?}");
    assert_eq!(
        schedule_of(&exhausted_reason(
            &h.store.agent(other).expect("a").expect("e").status
        )),
        (later, 0),
        "a failed probe must not rewrite anyone else's row"
    );
    assert!(
        exhaustion_records(h.store.as_ref(), other, "probe_failed").is_empty(),
        "the other agent never probed"
    );
}

/// The recovered fence (§5): an episode closes once. Neither a second
/// invocation inside the probe turn nor a whole later turn reopens it.
#[tokio::test]
async fn an_episode_recovers_once_however_many_invocations_follow() {
    let h = harness_configured(
        vec![
            spent_until(Duration::from_millis(150)),
            // The probe turn: two completed invocations before it parks.
            ScriptedClient::tool_call("inspect_state", json!({})),
            ScriptedClient::tool_call("wait_for", json!({"timeout_seconds": 3600})),
            // A later turn, after a person speaks.
            ScriptedClient::tool_call("complete_task", json!({"summary": "done at last"})),
        ],
        |_| {},
        |config| config.exhaustion_probe_floor = Duration::from_millis(20),
    )
    .await;
    let (parent, child, _) = parent_and_suspending_child(&h, spec("probe, then carry on")).await;
    h.wait_until(child, blocked_on_exhaustion).await;
    h.wait_until(child, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    h.runtime
        .send_message(child, vec![ContentBlock::text("carry on")], Actor::user())
        .expect("message");
    let status = h.wait_until(child, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );

    assert_eq!(
        exhaustion_records(h.store.as_ref(), child, "recovered").len(),
        1,
        "the episode was already closed by the first invocation that answered"
    );
    assert_eq!(child_resumptions(h.store.as_ref(), parent), vec![child]);
}

/// An episode belongs to the link whose quota was spent, not to the
/// turn. A fallback carrying a probe turn is not that link's recovery - it is
/// announced for nobody, and the spent link keeps backing off instead of
/// restarting at the floor.
#[tokio::test]
async fn a_fallback_answering_leaves_the_spent_links_episode_open() {
    let h = harness_configured(
        vec![],
        |_| {},
        |config| config.exhaustion_probe_floor = Duration::from_millis(200),
    )
    .await;
    // The first link comes back at once, so a probe turn reaches it; the last
    // link - the one the suspension is about - stays spent for one probe more.
    let first_spent = || spent_until(Duration::from_millis(10));
    let last_spent =
        || ScriptedClient::error(ModelErrorClass::QuotaExhausted, "credit balance too low");
    for response in [
        first_spent(),
        // The probe turn the first link answers while the last is still out.
        ScriptedClient::tool_call("inspect_state", json!({})),
        first_spent(),
        first_spent(),
    ] {
        h.client.push_for("anthropic", response);
    }
    for response in [
        last_spent(),
        last_spent(),
        ScriptedClient::tool_call("complete_task", json!({"summary": "the window reset"})),
    ] {
        h.client.push_for("groq", response);
    }

    let (parent, child, _) = parent_and_suspending_child(
        &h,
        CreateAgentSpec {
            model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            fallbacks: vec![ModelRef::new("groq", "openai/gpt-oss-120b")],
            ..spec("outlast the last link's limit")
        },
    )
    .await;
    let status = h.wait_until(child, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );

    // One episode, on the spent link: the turn the first link answered left it
    // open, so the next suspension on it continued the count from 0 to 1
    // rather than opening a second episode back at the floor.
    let suspended = exhaustion_records(h.store.as_ref(), child, "suspended");
    assert_eq!(
        suspended.len(),
        1,
        "one episode, one diagnostic: {suspended:?}"
    );
    assert_eq!(suspended[0].1["model"], json!("openai/gpt-oss-120b"));
    let probes: Vec<serde_json::Value> =
        exhaustion_records(h.store.as_ref(), child, "probe_failed")
            .iter()
            .map(|(_, payload)| payload["probes"].clone())
            .collect();
    assert_eq!(probes, vec![json!(1)], "the probe count kept climbing");
    assert_eq!(child_suspensions(h.store.as_ref(), parent).len(), 1);

    // And recovery is announced once, for the link that was actually spent.
    let recovered = exhaustion_records(h.store.as_ref(), child, "recovered");
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].1["provider"], json!("groq"));
    assert_eq!(recovered[0].1["model"], json!("openai/gpt-oss-120b"));
    assert_eq!(recovered[0].1["probes"], json!(1));
    assert_eq!(child_resumptions(h.store.as_ref(), parent), vec![child]);
}

/// A canary a person pauses before it ever gets a slot must not take the
/// provider down with it: `paused` is orthogonal to the row's status, so the
/// claim has to be dropped on that too or everyone else waits on a pause.
#[tokio::test]
async fn a_paused_canary_does_not_park_the_provider_for_everyone_else() {
    let h = harness_configured(
        vec![
            ScriptedClient::tool_call("complete_task", json!({"summary": "held the only slot"})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "probed after all"})),
        ],
        |_| {},
        |config| {
            config.exhaustion_probe_floor = NO_PROBE_IN_THIS_TEST;
            // One slot, held below, so the released canary is Ready and stuck.
            config.max_concurrent_turns = 1;
        },
    )
    .await;
    let release = h.client.hold_response_at(1);
    h.runtime
        .create_agent(spec("hold the only slot"), Actor::user())
        .expect("blocker");
    // Both due at the same instant: the earlier id is released first.
    let due = now();
    let canary = extra_parked_agent(&h, "be released, then paused", due).await;
    let other = extra_parked_agent(&h, "probe once the claim is dropped", due).await;

    h.wait_until(canary, |s| matches!(s, AgentStatus::Ready))
        .await;
    assert!(
        blocked_on_exhaustion(&h.store.agent(other).expect("a").expect("e").status),
        "one canary at a time"
    );
    h.runtime
        .pause_agent(canary, false, Actor::user())
        .expect("pause");
    release.cancel();

    let status = h.wait_until(other, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );
    assert!(
        h.store.agent(canary).expect("a").expect("e").paused,
        "the paused canary stays paused and takes no turn"
    );
}
