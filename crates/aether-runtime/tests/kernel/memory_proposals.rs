use super::*;
use aether_core::memory::OperatorMemorySpec;

fn proposal() -> OperatorMemorySpec {
    serde_json::from_value(serde_json::json!({
        "text":"Always verify the build with the repository's release script",
        "kind":"lesson", "scope":{"scope":"global"}, "proposed":true,
    }))
    .unwrap()
}

#[tokio::test]
async fn proposal_recall_exclusion_reaches_agent_context_until_acceptance() {
    use super::memory_scope::memories_shown_after;
    let h = harness(vec![
        ScriptedClient::text("Waiting for review"),
        ScriptedClient::text("Lesson accepted"),
    ])
    .await;
    let pending = h.runtime.create_memory(proposal(), Actor::user()).unwrap();
    let (agent, _) = h
        .runtime
        .create_agent(long_lived("assistant", None), Actor::user())
        .unwrap();
    let before = memories_shown_after(&h, agent.id, "What build lessons apply?", 1).await;
    assert!(!before.contains(&pending.id));
    let record = h.store.memory(pending.id).unwrap().unwrap();
    assert_eq!(record.retrieval_count, 0);
    let invocation = h
        .store
        .invocations_for_agent(agent.id)
        .unwrap()
        .pop()
        .unwrap();
    let snapshot = h.store.snapshot(invocation.snapshot_id).unwrap().unwrap();
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains(&pending.text)
    );
    h.runtime
        .accept_memory(
            pending.id,
            Some("Verified by repeated builds".into()),
            Actor::user(),
        )
        .unwrap();
    let after = memories_shown_after(&h, agent.id, "What build lessons apply?", 1).await;
    assert!(after.contains(&pending.id));
    assert!(h.store.memory(pending.id).unwrap().unwrap().retrieval_count > 0);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn proposal_authz_refuses_agents_and_the_mcp_tool_surface_cannot_review() {
    let h = harness(Vec::new()).await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("proposal authority"), Actor::user())
        .unwrap();
    let pending = h.runtime.create_memory(proposal(), Actor::user()).unwrap();
    let by = Actor::Agent { id: agent.id };
    let seq = h.store.last_seq().unwrap();
    assert!(h.runtime.create_memory(proposal(), by.clone()).is_err());
    assert!(
        h.runtime
            .accept_memory(pending.id, None, by.clone())
            .unwrap_err()
            .to_string()
            .contains("operator")
    );
    assert!(
        h.runtime
            .reject_memory(pending.id, "Agent decision".into(), by)
            .unwrap_err()
            .to_string()
            .contains("operator")
    );
    assert_eq!(h.store.last_seq().unwrap(), seq);
    assert_eq!(h.store.memory(pending.id).unwrap(), Some(pending));
    let tools = h.runtime.engine_tool_specs(&agent);
    assert!(tools.iter().any(|tool| tool.name == "remember"));
    for name in [
        "accept_memory",
        "reject_memory",
        "create_memory",
        "restore_memory",
    ] {
        assert!(!tools.iter().any(|tool| tool.name == name), "{name}");
    }
    let remember = tools.iter().find(|tool| tool.name == "remember").unwrap();
    assert!(
        remember.input_schema["properties"]
            .get("proposed")
            .is_none()
    );
    assert_eq!(remember.input_schema["additionalProperties"], false);
    h.runtime.shutdown().await;
}
