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
