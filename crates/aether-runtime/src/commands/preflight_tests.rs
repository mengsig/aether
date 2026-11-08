use super::{CreateAgentSpec, Preflight};
use crate::Runtime;
use aether_core::agent::{ApprovalPolicy, NetworkPolicy, Scope};
use aether_core::event::Actor;
use aether_core::model::ModelRef;
use aether_model::Models;
use aether_store::{AgentFilter, SqliteStore, Store, TaskFilter};
use aether_tools::ToolRegistry;
use std::sync::Arc;

#[tokio::test]
async fn stale_delegation_checks_create_neither_agents_nor_tasks() {
    let workspace = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let runtime = Runtime::start_with_engines(
        crate::RuntimeConfig {
            workspace_root: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
        store.clone(),
        Arc::new(Models::load(Some(config.path().to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .unwrap();
    let (parent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                model: Some(ModelRef::new("claude-code", "haiku")),
                scope: Some(Scope {
                    network: NetworkPolicy::Allowed,
                    workspace_root: Some(workspace.path().to_path_buf()),
                    ..Default::default()
                }),
                approval: Some(ApprovalPolicy::none()),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let spec = CreateAgentSpec {
        parent: Some(parent.id),
        config: Some(aether_core::agent::AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            ..Default::default()
        }),
        objective: Some("read docs".into()),
        ..Default::default()
    };
    let old_model = ModelRef::new("claude-code", "sonnet");
    for checked in [
        (&old_model, NetworkPolicy::Allowed),
        (&parent.model, NetworkPolicy::Denied),
    ] {
        let error = runtime
            .create_agent_with_preflight(
                spec.clone(),
                Actor::user(),
                Preflight {
                    checked: Some(checked),
                    run_budget: false,
                    inherit_limits: false,
                    before_hold: None,
                    isolated_workspace: None,
                },
                None,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed during delegation checks"),
            "{error}"
        );
        assert_eq!(store.agents(&AgentFilter::default()).unwrap().len(), 1);
        assert!(store.tasks(&TaskFilter::default()).unwrap().is_empty());
    }
    let (child, task) = runtime
        .create_agent_with_preflight(
            spec,
            Actor::user(),
            Preflight {
                checked: Some((&parent.model, parent.scope.network)),
                run_budget: false,
                inherit_limits: false,
                before_hold: None,
                isolated_workspace: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(child.parent, Some(parent.id));
    assert!(task.is_some());
    runtime.shutdown().await;
}
