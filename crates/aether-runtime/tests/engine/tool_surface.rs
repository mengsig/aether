//! Leaf and parent orchestration tool exposure.
use super::*;
fn tool_surface_agent(h: &Harness) -> aether_core::agent::Agent {
    h.runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap()
        .0
}

#[tokio::test]
async fn non_leaf_engine_tools_expose_the_catalog_and_explicit_delegation_choices() {
    let h = harness(Reply::OneTurn).await;
    let agent = tool_surface_agent(&h);
    let root = h.runtime.engine_tool_specs(&agent);
    let mut before = aether_tools::builtin::harness_tools()
        .into_iter()
        .map(|tool| tool.descriptor().spec)
        .collect::<Vec<_>>();
    before.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        serde_json::to_vec(&root).unwrap(),
        serde_json::to_vec(&before).unwrap()
    );
    assert_eq!(root.len(), 20);
    let delegate = root.iter().find(|tool| tool.name == "delegate").unwrap();
    let required = delegate.input_schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("model")));
    assert!(required.contains(&serde_json::json!("effort")));
    assert_eq!(delegate.input_schema["additionalProperties"], false);
    let integrate = root
        .iter()
        .find(|tool| tool.name == "integrate_workspace")
        .unwrap();
    assert_eq!(
        integrate.input_schema["required"],
        serde_json::json!(["child_agent_id"])
    );
    assert_eq!(integrate.input_schema["additionalProperties"], false);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn leaf_engine_tools_remove_only_coordination_tools() {
    let h = harness(Reply::OneTurn).await;
    let mut agent = tool_surface_agent(&h);
    let root = h.runtime.engine_tool_specs(&agent);
    agent.config.leaf = true;
    let leaf = h.runtime.engine_tool_specs(&agent);
    let removed = [
        "delegate",
        "integrate_workspace",
        "cancel_agent",
        "create_schedule",
        "update_schedule",
        "list_schedules",
        "list_agents",
        "list_tasks",
        "read_task",
        "list_models",
    ];
    for name in removed {
        assert!(root.iter().any(|tool| tool.name == name), "{name}");
        assert!(!leaf.iter().any(|tool| tool.name == name), "{name}");
    }
    for name in [
        "complete_task",
        "fail_task",
        "send_message",
        "wait_for",
        "inspect_state",
        "read_artifact",
        "remember",
        "request_checkpoint",
        "ask_user",
    ] {
        assert!(leaf.iter().any(|tool| tool.name == name), "{name}");
    }
    // `permission` is appended unconditionally by aether-engine's bridge, not this registry.
    let expected = root
        .iter()
        .filter(|tool| !removed.contains(&tool.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_vec(&leaf).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    let est = aether_context::estimate::TokenEstimator;
    println!(
        "engine tools: root={}, leaf={}, estimated token delta={}",
        root.len(),
        leaf.len(),
        est.tools(&root) - est.tools(&leaf)
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn leaf_engine_tools_still_obey_tool_exposure() {
    let h = harness(Reply::OneTurn).await;
    let mut agent = tool_surface_agent(&h);
    agent.config.leaf = true;
    agent.config.tools = aether_core::tool::ToolExposure::Only {
        names: ["delegate".into(), "read_artifact".into()].into(),
    };
    let narrowed = h.runtime.engine_tool_specs(&agent);
    assert_eq!(narrowed.len(), 1);
    assert_eq!(narrowed[0].name, "read_artifact");
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn minimal_solo_exposure_shows_only_task_terminal_tools() {
    let h = harness(Reply::OneTurn).await;
    let mut agent = tool_surface_agent(&h);
    agent.config.tools = aether_core::tool::ToolExposure::Only {
        names: ["complete_task".into(), "fail_task".into()].into(),
    };
    let names = h
        .runtime
        .engine_tool_specs(&agent)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["complete_task", "fail_task"]);
    h.runtime.shutdown().await;
}
