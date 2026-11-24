use super::*;
use crate::commands::AgentUpdate;
use aether_core::agent::{AgentConfig, DeadlineUpdate};
use aether_core::model::{Effort, ModelRef};
use aether_model::Models;
use aether_store::SqliteStore;
use aether_tools::ToolRegistry;
use std::sync::{Arc, mpsc};
use std::time::Duration;

async fn runtime() -> (Arc<Runtime>, tempfile::TempDir, tempfile::TempDir) {
    let workspace = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let runtime = Runtime::start_with_engines(
        crate::RuntimeConfig {
            workspace_root: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        Arc::new(Models::load(Some(config.path().to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .unwrap();
    (runtime, workspace, config)
}

fn parent(runtime: &Runtime, deadline: aether_core::time::Timestamp) -> Agent {
    runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                config: Some(AgentConfig {
                    deadline: Some(deadline),
                    cost_budget: Some(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap()
        .0
}

fn child_spec(
    parent: AgentId,
    deadline: Option<aether_core::time::Timestamp>,
    budget: Option<f64>,
) -> CreateAgentSpec {
    CreateAgentSpec {
        parent: Some(parent),
        config: Some(AgentConfig {
            effort: Some(Effort::Low),
            deadline,
            cost_budget: budget,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn subtree_raise_crossing_creation_preflight_reaches_child() {
    let (runtime, _workspace, _config) = runtime().await;
    let old = now() + chrono::Duration::hours(1);
    let raised = old + chrono::Duration::hours(1);
    let parent = parent(&runtime, old);
    let (prepared, ready) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let creating = runtime.clone();
    std::thread::spawn(move || {
        let preflight = Preflight {
            inherit_limits: true,
            before_hold: Some((&prepared, &proceed)),
            ..Default::default()
        };
        let child = creating.create_agent_with_preflight(
            child_spec(parent.id, None, None),
            Actor::user(),
            preflight,
            None,
        );
        finished.send(child).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime
        .update_agent_keyed(
            parent.id,
            AgentUpdate {
                deadline: Some(DeadlineUpdate::Set(raised)),
                cost_budget: Some(2.0),
                ..Default::default()
            },
            true,
            Actor::user(),
            None,
        )
        .unwrap();
    release.send(()).unwrap();
    let (child, _) = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(child.config.deadline, Some(raised));
    assert_eq!(child.config.cost_budget, Some(2.0));
    runtime.shutdown().await;
}

#[tokio::test]
async fn explicit_narrower_delegation_limits_are_preserved() {
    let (runtime, _workspace, _config) = runtime().await;
    let deadline = now() + chrono::Duration::hours(2);
    let parent = parent(&runtime, deadline);
    let narrow = deadline - chrono::Duration::hours(1);
    let (child, _) = runtime
        .create_agent_with_preflight(
            child_spec(parent.id, Some(narrow), Some(0.5)),
            Actor::user(),
            Preflight {
                inherit_limits: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(child.config.deadline, Some(narrow));
    assert_eq!(child.config.cost_budget, Some(0.5));
    let (inherited, _) = runtime
        .create_agent_with_preflight(
            child_spec(parent.id, None, None),
            Actor::user(),
            Preflight {
                inherit_limits: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(inherited.config.deadline, Some(deadline));
    assert_eq!(inherited.config.cost_budget, Some(1.0));
    runtime.shutdown().await;
}

#[tokio::test]
async fn delegated_children_recheck_parent_authority_under_the_creation_hold() {
    use crate::delegation_admission::{ChildReason, ChildRequest, Slots, prepare_child};
    use aether_core::tool::ToolExposure;

    let (runtime, _workspace, _config) = runtime().await;
    let mut lead = parent(&runtime, now() + chrono::Duration::hours(1));
    for update in [
        AgentUpdate {
            model: Some(ModelRef::new("anthropic", "claude-haiku-5-5")),
            ..Default::default()
        },
        AgentUpdate {
            tools: Some(ToolExposure::Only {
                names: ["delegate".into()].into(),
            }),
            ..Default::default()
        },
    ] {
        let request = ChildRequest {
            role: "worker".into(),
            name: None,
            objective: "check the tree".into(),
            instructions: None,
            model: lead.model.clone(),
            fallbacks: Vec::new(),
            effort: Effort::Low,
            scope: None,
            leaf: true,
            host_context: None,
            reason: ChildReason::Omitted,
        };
        let staged = prepare_child(
            &runtime,
            &lead,
            request,
            &Actor::agent(lead.id),
            Slots::Counted,
        )
        .expect("the child fits the original authority");
        assert_eq!(staged.agent.config.tools, lead.config.tools);
        assert_eq!(staged.agent.model, lead.model);
        let current = runtime
            .update_agent(lead.id, update, Actor::user())
            .unwrap();
        let hold = runtime.hold();
        let refusal = runtime
            .admit_prepared(&hold, staged, Preflight::delegated(&lead), None)
            .expect_err("the child's authority was decided against an older parent");
        assert!(
            refusal.to_string().contains("authority changed"),
            "{refusal}"
        );
        lead = current;
    }
    assert_eq!(runtime.store.agents(&Default::default()).unwrap().len(), 1);
    assert!(runtime.store.tasks(&Default::default()).unwrap().is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_engine_fallback_must_still_gate_approvals_under_the_creation_hold() {
    use crate::delegation_admission::{ChildReason, ChildRequest, Slots, prepare_child};
    use aether_core::agent::ApprovalPolicy;
    use aether_engine::{CodexAppEngine, CodexEngine, EngineKind};

    let (runtime, _workspace, _config) = runtime().await;
    let (lead, _) = runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create a lead with required approvals");
    let fallback = ModelRef::new("codex", "gpt-6-luna");
    let app_server = Arc::new(CodexAppEngine::new("unused", "0.130.0"));
    runtime.engines.mark_usable(EngineKind::CODEX, app_server);
    assert!(runtime.engine_can_gate(&fallback).unwrap());
    let request = ChildRequest {
        role: "worker".into(),
        name: None,
        objective: "check the tree".into(),
        instructions: None,
        model: lead.model.clone(),
        fallbacks: vec![fallback.clone()],
        effort: Effort::Low,
        scope: None,
        leaf: true,
        host_context: None,
        reason: ChildReason::Omitted,
    };
    let staged = prepare_child(
        &runtime,
        &lead,
        request,
        &Actor::agent(lead.id),
        Slots::Counted,
    )
    .expect("stage the allowed engine");
    assert_eq!(staged.agent.fallbacks, [fallback]);
    let exec = Arc::new(CodexEngine::new("unused", "0.130.0"));
    runtime.engines.mark_usable(EngineKind::CODEX, exec);
    let refusal = runtime
        .admit_prepared(&runtime.hold(), staged, Preflight::delegated(&lead), None)
        .expect_err("the replacement engine cannot enforce approvals");
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("codex/gpt-6-luna")
            && refusal.contains("cannot enforce required approvals"),
        "{refusal}"
    );
    assert_eq!(runtime.store.agents(&Default::default()).unwrap().len(), 1);
    assert!(runtime.store.tasks(&Default::default()).unwrap().is_empty());
    runtime.shutdown().await;
}

/// `max_parallel` is rechecked under the creation hold: a child created
/// while a delegation waits between its count and its commit fills the slot.
#[tokio::test]
async fn a_concurrent_creation_fills_the_parallel_slot_a_delegation_counted() {
    let (runtime, _workspace, _config) = runtime().await;
    let policy = aether_core::delegation::DelegationPolicy {
        max_parallel: Some(1),
        exempt_roles: vec!["reviewer".into()],
        ..Default::default()
    };
    let (lead, _) = runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                config: Some(AgentConfig {
                    delegation_policy: Some(policy),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let (prepared, ready) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let creating = runtime.clone();
    std::thread::spawn(move || {
        let preflight = Preflight {
            parallel_limit: true,
            before_hold: Some((&prepared, &proceed)),
            ..Default::default()
        };
        let child = creating.create_agent_with_preflight(
            child_spec(lead.id, None, None),
            Actor::user(),
            preflight,
            None,
        );
        finished.send(child).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime
        .create_agent(child_spec(lead.id, None, None), Actor::user())
        .expect("a person's child is never refused for the limit");
    release.send(()).unwrap();
    let refused = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .expect_err("the slot was taken under the delegation's feet");
    assert!(
        refused.to_string().contains("at most 1 unfinished"),
        "{refused}"
    );
    let reviewer = CreateAgentSpec {
        role: "reviewer".into(),
        ..child_spec(lead.id, None, None)
    };
    let exempt = Preflight {
        parallel_limit: true,
        ..Default::default()
    };
    runtime
        .create_agent_with_preflight(reviewer, Actor::user(), exempt, None)
        .expect("an exempt role holds no slot");
    runtime.shutdown().await;
}
