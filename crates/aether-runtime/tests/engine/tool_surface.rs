//! Leaf and parent orchestration tool exposure.
use super::*;
use aether_core::model::ToolSpec;
use std::collections::BTreeSet;

/// Every harness tool a root agent saw in v1.1.0.
const V1_1_0_ROOT_TOOLS: [&str; 21] = [
    "ask_user",
    "cancel_agent",
    "complete_task",
    "create_schedule",
    "delegate",
    "fail_task",
    "inspect_state",
    "integrate_workspace",
    "list_agents",
    "list_models",
    "list_schedules",
    "list_tasks",
    "provider_usage",
    "read_artifact",
    "read_task",
    "refresh_provider_usage",
    "remember",
    "request_checkpoint",
    "send_message",
    "update_schedule",
    "wait_for",
];

/// Coordination tools a leaf never sees.
const LEAF_REMOVED: [&str; 14] = [
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
    "provider_usage",
    "refresh_provider_usage",
    "swarm_playbook",
    "notify",
];

/// v1.1.0's harness tool lists as the engine bridge serialized them; a
/// delegated agent saw the same list as a root one.
const V1_1_0_ROOT_BYTES: usize = 15_709;
const V1_1_0_LEAF_BYTES: usize = 6_324;

/// Most bytes each surface may grow by over v1.1.0's.
const ROOT_ADDED_BYTES: usize = 200;
/// What the notification center's `notify` adds to a surface that is not a leaf.
const NOTIFY_ADDED_BYTES: usize = 700;
const DELEGATED_LEAF_ADDED_BYTES: usize = 300;
const DELEGATED_LEAD_ADDED_BYTES: usize = 470;

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

fn delegated_agent(h: &Harness, parent: AgentId, leaf: bool) -> aether_core::agent::Agent {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                parent: Some(parent),
                config: Some(AgentConfig {
                    leaf,
                    effort: Some(Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    assert_eq!(agent.parent, Some(parent));
    assert_eq!(agent.config.leaf, leaf);
    agent
}

fn names(specs: &[ToolSpec]) -> BTreeSet<&str> {
    specs.iter().map(|tool| tool.name.as_str()).collect()
}

/// How many bytes `specs` grow a v1.1.0 surface of `baseline` bytes by,
/// serialized the way the engine bridge lists them.
fn added_bridge_bytes(specs: &[ToolSpec], baseline: usize) -> usize {
    let listed: Vec<_> = specs
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&listed).unwrap().len();
    assert!(
        bytes > baseline,
        "{bytes} bytes: the swarm tools are missing"
    );
    bytes - baseline
}

#[tokio::test]
async fn non_leaf_engine_tools_expose_the_catalog_and_explicit_delegation_choices() {
    let h = harness(Reply::OneTurn).await;
    let agent = tool_surface_agent(&h);
    let root = h.runtime.engine_tool_specs(&agent);
    let mut before = aether_tools::builtin::harness_tools()
        .into_iter()
        .map(|tool| tool.descriptor().spec)
        .filter(|spec| spec.name != "list_peers")
        .collect::<Vec<_>>();
    before.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        serde_json::to_vec(&root).unwrap(),
        serde_json::to_vec(&before).unwrap()
    );
    assert_eq!(root.len(), 23);
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
    let removed = LEAF_REMOVED;
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
    let bytes = |specs: &[aether_core::model::ToolSpec]| serde_json::to_vec(specs).unwrap().len();
    let provider_bytes = bytes(&root) - bytes(&leaf);
    assert!(
        provider_bytes > 0,
        "a leaf must carry less tool text than a parent"
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
async fn a_root_agent_sees_the_v1_1_0_surface_plus_only_the_playbook_and_notify() {
    let h = harness(Reply::OneTurn).await;
    let root = h.runtime.engine_tool_specs(&tool_surface_agent(&h));
    let mut expected = BTreeSet::from(V1_1_0_ROOT_TOOLS);
    expected.extend(["swarm_playbook", "notify"]);
    assert_eq!(names(&root), expected);
    let added = added_bridge_bytes(&root, V1_1_0_ROOT_BYTES);
    println!("root surface: {added} descriptor bytes beyond v1.1.0");
    assert!(added <= ROOT_ADDED_BYTES + NOTIFY_ADDED_BYTES, "{added}");
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn delegated_agents_see_list_peers_and_only_a_lead_sees_the_playbook() {
    let h = harness(Reply::OneTurn).await;
    let root = tool_surface_agent(&h);
    let leaf = h
        .runtime
        .engine_tool_specs(&delegated_agent(&h, root.id, true));
    let lead = h
        .runtime
        .engine_tool_specs(&delegated_agent(&h, root.id, false));

    let mut leaf_expected: BTreeSet<&str> = V1_1_0_ROOT_TOOLS
        .into_iter()
        .filter(|name| !LEAF_REMOVED.contains(name))
        .collect();
    leaf_expected.insert("list_peers");
    assert_eq!(names(&leaf), leaf_expected);
    let mut lead_expected = BTreeSet::from(V1_1_0_ROOT_TOOLS);
    lead_expected.extend(["list_peers", "swarm_playbook", "notify"]);
    assert_eq!(names(&lead), lead_expected);

    let leaf_added = added_bridge_bytes(&leaf, V1_1_0_LEAF_BYTES);
    let lead_added = added_bridge_bytes(&lead, V1_1_0_ROOT_BYTES);
    println!(
        "delegated surfaces: leaf {leaf_added}, non-leaf {lead_added} descriptor bytes beyond v1.1.0"
    );
    assert!(leaf_added <= DELEGATED_LEAF_ADDED_BYTES, "{leaf_added}");
    assert!(
        lead_added <= DELEGATED_LEAD_ADDED_BYTES + NOTIFY_ADDED_BYTES,
        "{lead_added}"
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
