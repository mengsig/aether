//! execution policy scenarios and fixtures.
use super::*;

/// Every config knob persists at creation; mutable knobs can then change on a
/// live agent and emit an event. `leaf`, `host_context` and `build_cache` are
/// intentionally creation-only.
#[tokio::test]
async fn every_config_knob_persists_at_creation_and_mutable_knobs_update_live() {
    use aether_core::agent::{ExecutionBackend, HostContext, NetworkPolicy};
    use aether_core::model::Effort;
    use aether_core::task::Priority;
    use aether_core::tool::ToolExposure;
    use aether_runtime::AgentUpdate;

    let h = harness(vec![]).await;
    let at_creation = AgentConfig {
        instructions: Some("be terse".into()),
        effort: Some(Effort::High),
        max_turns: Some(9),
        max_output_tokens: Some(1024),
        token_budget: Some(100_000),
        cost_budget: Some(2.50),
        deadline: Some(now() + chrono::Duration::hours(1)),
        tools: ToolExposure::Only {
            names: ["read_file".to_owned()].into_iter().collect(),
        },
        approval: ApprovalPolicy::none(),
        priority: Priority::High,
        long_lived: true,
        leaf: true,
        concurrency: Some(3),
        host_context: HostContext::Lean,
        build_cache: Some("/srv/cache/items/one".into()),
    };
    let scope = Scope {
        workspace_root: Some(h.workspace_root.clone()),
        network: NetworkPolicy::Allowed,
        backend: ExecutionBackend::Container {
            image: "rust:1".into(),
        },
        ..Default::default()
    };
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: None,
                config: Some(at_creation.clone()),
                scope: Some(scope.clone()),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    let stored = h.store.agent(agent.id).expect("agent").expect("exists");
    assert_eq!(stored.config, at_creation, "every knob landed at creation");
    assert_eq!(stored.scope.network, NetworkPolicy::Allowed);
    assert_eq!(stored.scope.backend, scope.backend);

    // Now change every one of them on the live agent.
    let later = now() + chrono::Duration::hours(4);
    let updated = h
        .runtime
        .update_agent(
            agent.id,
            AgentUpdate {
                name: None,
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                // Priced, because the patch sets a cost budget: a budget no
                // link can report is refused.
                fallbacks: Some(vec![ModelRef::new("anthropic", "claude-haiku-5-5")]),
                effort: Some(Effort::Low),
                instructions: Some("explain your reasoning".into()),
                max_turns: Some(42),
                token_budget: Some(7),
                cost_budget: Some(0.25),
                deadline: Some(DeadlineUpdate::Set(later)),
                tools: Some(ToolExposure::All),
                approval: Some(ApprovalPolicy::default()),
                priority: Some(Priority::Low),
                concurrency: Some(1),
                long_lived: Some(false),
                network: Some(NetworkPolicy::Denied),
                backend: Some(ExecutionBackend::Host),
            },
            Actor::user(),
        )
        .expect("update");
    let c = &updated.config;
    assert_eq!(c.instructions.as_deref(), Some("explain your reasoning"));
    assert_eq!(c.effort, Some(Effort::Low));
    assert_eq!(c.max_turns, Some(42));
    assert_eq!(c.token_budget, Some(7));
    assert_eq!(c.cost_budget, Some(0.25));
    assert_eq!(c.deadline, Some(later));
    assert_eq!(c.tools, ToolExposure::All);
    assert_eq!(c.approval, ApprovalPolicy::default());
    assert_eq!(c.priority, Priority::Low);
    assert!(!c.long_lived);
    assert!(
        c.leaf,
        "live updates must preserve the creation-only leaf setting"
    );
    assert_eq!(c.concurrency, Some(1));
    assert_eq!(c.host_context, HostContext::Lean);
    assert_eq!(
        updated.model,
        ModelRef::new("anthropic", "claude-sonnet-5-5")
    );
    assert_eq!(
        updated.fallbacks,
        vec![ModelRef::new("anthropic", "claude-haiku-5-5")]
    );
    assert_eq!(updated.scope.network, NetworkPolicy::Denied);
    assert_eq!(updated.scope.backend, ExecutionBackend::Host);

    // Each change is an event, and it carries what changed.
    let event = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .rev()
        .find_map(|e| match e.kind {
            EventKind::AgentConfigUpdated {
                model,
                config,
                scope,
                ..
            } => Some((model, config, scope)),
            _ => None,
        })
        .expect("agent.config_updated");
    assert_eq!(event.1, updated.config);
    assert_eq!(event.0, Some(updated.model.clone()));
    assert_eq!(
        event.2.expect("scope in the event").backend,
        ExecutionBackend::Host
    );

    // A scope knob can be tightened but never widened past the parent.
    let (scope_parent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "scope-parent".into(),
                scope: Some(updated.scope.clone()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create non-leaf parent for scope test");
    let mut child_spec = CreateAgentSpec {
        role: "worker".into(),
        parent: Some(scope_parent.id),
        config: Some(aether_core::agent::AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            ..Default::default()
        }),
        start: true,
        ..Default::default()
    };
    child_spec.scope = Some(Scope {
        backend: ExecutionBackend::Container {
            image: "rust:1".into(),
        },
        ..updated.scope.clone()
    });
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("a child may narrow");
    let err = h
        .runtime
        .update_agent(
            child.id,
            AgentUpdate {
                network: Some(NetworkPolicy::Allowed),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("widening past the parent");
    assert!(
        err.to_string().contains("must stay within the parent"),
        "{err}"
    );
}

/// Handed to a process the agent's backend does not contain.
struct OffBackend;

#[async_trait::async_trait]
impl aether_tools::Tool for OffBackend {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: "off_backend".into(),
                description: "runs somewhere else entirely".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn escapes_backend(&self) -> bool {
        true
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        Ok(aether_tools::ToolExecution::Outcome(
            aether_core::tool::ToolOutcome::ok("ran on the daemon's host"),
        ))
    }
}

/// A container-backed agent never silently runs its tools on the host: a tool
/// that leaves the container is not offered to it, and naming it anyway says
/// which of the agent's own limits withheld it rather than reading as a typo.
#[tokio::test]
async fn a_container_backed_agent_is_refused_a_tool_that_leaves_its_container() {
    use aether_core::agent::ExecutionBackend;

    let h = harness_with_tools(
        vec![
            ScriptedClient::tool_call("off_backend", json!({})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "gave up"})),
        ],
        |_| {},
        |_| {},
        |tools| tools.register(Arc::new(OffBackend)),
    )
    .await;
    let mut s = spec("reach outside the container");
    s.scope = Some(Scope {
        workspace_root: Some(h.workspace_root.clone()),
        backend: ExecutionBackend::Container {
            image: "rust:1".into(),
        },
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let failure = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::ToolFailed { error, .. } => Some(error),
            _ => None,
        })
        .expect("the call was refused");
    assert_eq!(
        failure,
        "`off_backend` is not available to this agent: it runs outside this agent's \
         container backend, on the daemon's host",
        "the whole message a person reads, not one word of it"
    );
}

/// VISION §3.1: `deadline` is a hard stop. It ends the agent cleanly with the
/// reason, and takes its live subtree with it.
#[tokio::test]
async fn a_deadline_stops_the_agent_and_its_children() {
    let h = harness(vec![]).await;
    let mut s = spec("take your time");
    s.objective = None;
    s.config.as_mut().expect("config").long_lived = true;
    let (parent, _) = h
        .runtime
        .create_agent(s.clone(), Actor::user())
        .expect("parent");
    let mut child_spec = s;
    child_spec.parent = Some(parent.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("child");

    h.runtime
        .update_agent(
            parent.id,
            aether_runtime::AgentUpdate {
                deadline: Some(DeadlineUpdate::Set(now() - chrono::Duration::seconds(1))),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("set a deadline in the past");

    let status = h.wait_until(parent.id, |s| s.is_terminal()).await;
    match status {
        AgentStatus::Failed { error } => assert!(error.contains("deadline of"), "{error}"),
        other => panic!("expected Failed with the deadline, got {other:?}"),
    }
    assert!(
        types(h.store.as_ref(), parent.id).contains(&"deadline.passed".to_owned()),
        "the stop is an event"
    );
    let child_status = h.wait_until(child.id, |s| s.is_terminal()).await;
    assert_eq!(child_status.kind(), AgentStatusKind::Cancelled);
}

/// VISION §16: `turn_engine` never reads `scope.backend` - an external engine
/// runs its tools on the daemon's host. Container scope on an engine-backed
/// agent would read as confinement and provide none, so it is refused at both
/// the surfaces that can set it.
#[tokio::test]
async fn a_container_backend_is_refused_on_an_engine_backed_agent() {
    use aether_core::agent::ExecutionBackend;

    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        Some(
            "[providers.cc]\nprotocol = \"claude-code\"\n\
             base_url = \"http://127.0.0.1:1\"\n\
             [[providers.cc.models]]\nid = \"sonnet\"\n",
        ),
        None,
        None,
        |_| {},
    )
    .await;
    let contained = Scope {
        workspace_root: Some(h.workspace_root.clone()),
        backend: ExecutionBackend::Container {
            image: "rust:1".into(),
        },
        ..Default::default()
    };
    let mut s = spec_on("build it", "cc", "sonnet");
    s.scope = Some(contained.clone());
    let err = h
        .runtime
        .create_agent(s, Actor::user())
        .expect_err("an engine cannot be confined to a container");
    assert!(
        err.to_string().contains("runs its tools on the host"),
        "{err}"
    );

    // Nor by tightening a live engine-backed agent onto it.
    let (agent, _) = h
        .runtime
        .create_agent(spec_on("build it", "cc", "sonnet"), Actor::user())
        .expect("host-backed engine agent");
    let err = h
        .runtime
        .update_agent(
            agent.id,
            aether_runtime::AgentUpdate {
                backend: Some(ExecutionBackend::Container {
                    image: "rust:1".into(),
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("the same refusal on update");
    assert!(
        err.to_string().contains("runs its tools on the host"),
        "{err}"
    );

    // Nor by moving a container-scoped agent onto an engine.
    let mut api = spec("build it");
    api.scope = Some(contained);
    let (api_agent, _) = h
        .runtime
        .create_agent(api, Actor::user())
        .expect("an API model may be container-scoped");
    let err = h
        .runtime
        .update_agent(
            api_agent.id,
            aether_runtime::AgentUpdate {
                model: Some(ModelRef::new("cc", "sonnet")),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("the model move is refused too");
    assert!(
        err.to_string().contains("runs its tools on the host"),
        "{err}"
    );
}
