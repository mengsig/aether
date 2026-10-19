//! operator rename scenarios and fixtures.
use super::*;

#[tokio::test]
async fn operator_rename_validates_and_preserves_agent_invariants() {
    let h = harness(Vec::new()).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..spec("rename target")
            },
            Actor::user(),
        )
        .unwrap();
    let before = h.store.events(0, 10000, &EventFilter::default()).unwrap();
    for name in [
        "".into(),
        "   ".into(),
        "x".repeat(257),
        "bad\nname".into(),
        "bad\0name".into(),
        "bad\u{2028}name".into(),
    ] {
        assert!(
            h.runtime
                .update_agent(
                    agent.id,
                    AgentUpdate {
                        name: Some(name),
                        ..Default::default()
                    },
                    Actor::user()
                )
                .is_err()
        );
        assert_eq!(h.store.agent(agent.id).unwrap().unwrap(), agent);
        assert_eq!(
            h.store.events(0, 10000, &EventFilter::default()).unwrap(),
            before
        );
    }
    let renamed = h
        .runtime
        .update_agent(
            agent.id,
            AgentUpdate {
                name: Some("  名前 café  ".into()),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    assert_eq!(renamed.name, "名前 café");
    let mut expected = agent.clone();
    expected.name = renamed.name.clone();
    expected.updated_at = renamed.updated_at;
    assert_eq!(renamed, expected);
    assert_eq!(h.store.agent(agent.id).unwrap().unwrap(), renamed);
    let events = h
        .store
        .events(0, 10000, &EventFilter::for_agent(agent.id))
        .unwrap();
    assert!(
        matches!(&events.last().unwrap().kind, EventKind::AgentConfigUpdated { name: Some(name), .. } if name == "名前 café")
    );
    h.runtime
        .cancel_agent(agent.id, None, Actor::user())
        .unwrap();
    let before = h.store.events(0, 10000, &EventFilter::default()).unwrap();
    let err = h
        .runtime
        .update_agent(
            agent.id,
            AgentUpdate {
                name: Some("cannot rename terminal".into()),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("only live or failed"), "{err}");
    assert_eq!(
        h.store.events(0, 10000, &EventFilter::default()).unwrap(),
        before
    );
}

#[tokio::test]
async fn creation_refuses_the_names_rename_refuses() {
    let h = harness(Vec::new()).await;
    let before = h.store.events(0, 10000, &EventFilter::default()).unwrap();
    for name in [
        "".to_owned(),
        "  ".into(),
        "x".repeat(257),
        "bad\nname".into(),
    ] {
        let refused = h.runtime.create_agent(
            CreateAgentSpec {
                start: false,
                name: Some(name),
                ..spec("named at creation")
            },
            Actor::user(),
        );
        let message = refused.expect_err("name is refused").to_string();
        assert!(message.contains("1 to 256 characters"), "{message}");
    }
    assert_eq!(
        h.store.events(0, 10000, &EventFilter::default()).unwrap(),
        before,
        "a refused name leaves no event"
    );
}
