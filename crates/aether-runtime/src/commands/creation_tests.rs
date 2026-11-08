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
