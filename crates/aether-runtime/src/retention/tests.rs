#![allow(clippy::unwrap_used)]

use super::*;
use crate::{RunningTurn, RuntimeConfig};
use aether_core::{
    agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope},
    id::AgentId,
    model::ModelRef,
    time::now,
    tool::ResourceKey,
};
use aether_model::{Models, testing::ScriptedClient};
use aether_store::{Commit, Mutation, SqliteStore};
use aether_tools::ToolRegistry;
use std::{collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

async fn fixture() -> (Arc<Runtime>, Agent, tempfile::TempDir) {
    fixture_with_days(7).await
}

async fn fixture_with_days(days: u16) -> (Arc<Runtime>, Agent, tempfile::TempDir) {
    fixture_with_worker(days, false).await
}

async fn fixture_with_worker(
    days: u16,
    keep_running: bool,
) -> (Arc<Runtime>, Agent, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!("[retention]\ncompleted_run_days = {days}\n"),
    )
    .unwrap();
    let runtime = Runtime::start_with_engines(
        RuntimeConfig::default(),
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        Arc::new(Models::load(Some(dir.path().to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(vec![]))),
        HashMap::new(),
    )
    .await
    .unwrap();
    if !keep_running {
        runtime.shutdown.cancel();
    }
    tokio::task::yield_now().await;
    let at = now() - chrono::Duration::days(14);
    let agent = Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "finished".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "test"),
        fallbacks: vec![],
        status: AgentStatus::Completed { summary: None },
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    };
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    (runtime, agent, dir)
}

#[tokio::test]
async fn background_sweep_reports_shutdown_without_deleting_candidates() {
    let (runtime, agent, _dir) = fixture().await;
    let report = runtime.sweep_run_history_inner(now(), true).unwrap();
    assert!(report.interrupted);
    assert_eq!(report.expired_runs, 0);
    assert!(runtime.store.agent(agent.id).unwrap().is_some());
}

#[tokio::test]
async fn maintenance_expires_history_and_shutdown_stops_future_sweeps() {
    use std::time::Duration;
    let (runtime, agent, _dir) = fixture_with_worker(7, true).await;
    let worker = tokio::spawn(super::worker::run(
        runtime.clone(),
        Duration::from_millis(10),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.store.agent(agent.id).unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    // A stopped maintenance loop must not run again, even if old work appears.
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(runtime.store.agent(agent.id).unwrap().is_some());
}

#[tokio::test]
async fn sweep_keeps_recent_and_unfinished_runs_and_counts_each_run_once() {
    let (runtime, old, _dir) = fixture().await;
    let at = now();
    let mut sibling = old.clone();
    sibling.id = AgentId::new();
    let mut recent = old.clone();
    recent.id = AgentId::new();
    recent.run = RunId::new();
    recent.updated_at = at - chrono::Duration::days(7);
    let mut unfinished = old.clone();
    unfinished.id = AgentId::new();
    unfinished.run = RunId::new();
    let mut active = unfinished.clone();
    active.id = AgentId::new();
    active.status = AgentStatus::Created;
    for agent in [&sibling, &recent, &unfinished, &active] {
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
    }
    let report = runtime.sweep_run_history_at(at).unwrap();
    assert_eq!(report.expired_runs, 1);
    assert_eq!(report.expired_rows, 2);
    assert_eq!(report.protected.get("unfinished work"), Some(&1));
    for agent in [&recent, &unfinished, &active] {
        assert!(runtime.store.agent(agent.id).unwrap().is_some());
    }
    assert!(runtime.store.agent(sibling.id).unwrap().is_none());
    assert_eq!(runtime.sweep_run_history_at(at).unwrap().expired_runs, 0);
}

#[tokio::test]
async fn sweep_uses_configured_ten_day_retention() {
    let (runtime, mut agent, _dir) = fixture_with_days(10).await;
    let at = now();
    agent.updated_at = at - chrono::Duration::days(8);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    let preview = runtime.preview_retention_at(at).unwrap();
    assert_eq!(preview.history_before, at - chrono::Duration::days(10));
    assert_eq!(
        preview.runs[0].decision,
        RunRetentionDecision::Protected {
            reason: "recent state change".into()
        }
    );
    assert_eq!(runtime.sweep_run_history_at(at).unwrap().expired_runs, 0);
    assert!(runtime.store.agent(agent.id).unwrap().is_some());
    assert_eq!(
        runtime
            .sweep_run_history_at(at + chrono::Duration::days(3))
            .unwrap()
            .expired_runs,
        1
    );
}

#[tokio::test]
async fn completed_projection_is_protected_until_turn_winds_down() {
    let (runtime, agent, _dir) = fixture().await;
    let cutoff = now() - chrono::Duration::days(7);
    let cancel = CancellationToken::new();
    runtime.sched.lock().running.insert(
        agent.id,
        RunningTurn {
            task: None,
            interrupt: cancel.child_token(),
            cancel,
            cancel_request: None,
            stop_reason: None,
            addressed: false,
            provider: "mock".into(),
            engine: true,
            on_provider: false,
        },
    );
    assert_eq!(
        runtime.expire_run_history(agent.run, cutoff).unwrap(),
        RunExpiry::Protected("in-flight turn")
    );
    assert!(runtime.store.agent(agent.id).unwrap().is_some());
    runtime.sched.lock().running.remove(&agent.id);
    assert!(matches!(
        runtime.expire_run_history(agent.run, cutoff).unwrap(),
        RunExpiry::Expired { rows: 1, .. }
    ));
    assert!(runtime.store.agent(agent.id).unwrap().is_none());
}

#[tokio::test]
async fn quarantine_protects_history_until_resources_are_released() {
    let (runtime, agent, _dir) = fixture().await;
    let cutoff = now() - chrono::Duration::days(7);
    runtime
        .resources
        .quarantine(agent.id, vec![ResourceKey("file:/test".into())]);
    assert_eq!(
        runtime.expire_run_history(agent.run, cutoff).unwrap(),
        RunExpiry::Protected("held resource")
    );
    assert!(runtime.store.agent(agent.id).unwrap().is_some());
    runtime.resources.release_quarantine(agent.id);
    assert!(matches!(
        runtime.expire_run_history(agent.run, cutoff).unwrap(),
        RunExpiry::Expired { .. }
    ));
}

/// Judging a run scans whole tables; under the commit gate and `sched` that
/// stalled every commit and `status` for minutes each hourly sweep.
#[tokio::test]
async fn a_protected_run_is_judged_without_the_commit_gate_or_scheduler() {
    use std::time::Duration;
    let (runtime, mut unfinished, _dir) = fixture().await;
    unfinished.id = AgentId::new();
    unfinished.run = RunId::new();
    unfinished.status = AgentStatus::Created;
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(unfinished.clone())))
        .unwrap();
    let cutoff = now() - chrono::Duration::days(7);
    let sched = runtime.sched.lock();
    let hold = runtime.hold();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let judging = &runtime;
        scope.spawn(move || tx.send(judging.expire_run_history(unfinished.run, cutoff)));
        let verdict = rx.recv_timeout(Duration::from_secs(10));
        drop((hold, sched));
        assert_eq!(
            verdict
                .expect("judged while the gate and scheduler were held")
                .unwrap(),
            RunExpiry::Protected("unfinished work")
        );
    });
}

#[tokio::test]
async fn preview_and_sweep_agree_and_preview_preserves_every_run() {
    let (runtime, old, _dir) = fixture().await;
    let at = now();
    let mut recent = old.clone();
    recent.id = AgentId::new();
    recent.run = RunId::new();
    recent.updated_at = at - chrono::Duration::days(7);
    let mut active = old.clone();
    active.id = AgentId::new();
    active.run = RunId::new();
    active.status = AgentStatus::Created;
    for agent in [&recent, &active] {
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
    }
    let seq = runtime.store.last_seq().unwrap();
    let preview = runtime.preview_retention_at(at).unwrap();
    assert_eq!(preview.runs.len(), 3);
    for agent in [&old, &recent, &active] {
        assert_eq!(runtime.store.agent(agent.id).unwrap().as_ref(), Some(agent));
    }
    assert_eq!(runtime.store.last_seq().unwrap(), seq);
    assert_eq!(
        preview
            .runs
            .iter()
            .find(|p| p.run == recent.run)
            .unwrap()
            .decision,
        RunRetentionDecision::Protected {
            reason: "recent state change".into()
        }
    );
    assert_eq!(
        preview
            .runs
            .iter()
            .find(|p| p.run == active.run)
            .unwrap()
            .decision,
        RunRetentionDecision::Protected {
            reason: "unfinished work".into()
        }
    );
    let eligible: Vec<_> = preview
        .runs
        .iter()
        .filter(|p| p.decision == RunRetentionDecision::Eligible)
        .collect();
    assert_eq!(eligible.len(), 1);
    assert_eq!(eligible[0].run, old.run);
    assert!(preview.history_payload_bytes > 0);
    let sweep = runtime.sweep_run_history_at(at).unwrap();
    assert_eq!(sweep.expired_runs, eligible.len());
    assert_eq!(
        sweep.expired_rows as u64,
        eligible.iter().map(|p| p.rows).sum::<u64>()
    );
    assert!(runtime.store.agent(old.id).unwrap().is_none());
    for agent in [&recent, &active] {
        assert!(runtime.store.agent(agent.id).unwrap().is_some());
    }
}

#[tokio::test]
async fn preview_includes_runtime_guards_without_mutating_scheduler_or_resources() {
    for reason in ["in-flight turn", "native engine turn", "held resource"] {
        let (runtime, agent, _dir) = fixture().await;
        match reason {
            "in-flight turn" => {
                let cancel = CancellationToken::new();
                runtime.sched.lock().running.insert(
                    agent.id,
                    RunningTurn {
                        task: None,
                        interrupt: cancel.child_token(),
                        cancel,
                        cancel_request: None,
                        stop_reason: None,
                        addressed: false,
                        provider: "mock".into(),
                        engine: true,
                        on_provider: false,
                    },
                );
            }
            "native engine turn" => {
                let (tx, _rx) = tokio::sync::mpsc::channel(1);
                runtime.engine_turns.lock().insert(
                    agent.id,
                    crate::turn_engine::EngineTurnHandle {
                        token: "test".into(),
                        tx,
                    },
                );
            }
            _ => runtime
                .resources
                .quarantine(agent.id, vec![ResourceKey("file:/test".into())]),
        }
        let preview = runtime.preview_retention().unwrap();
        assert_eq!(
            preview.runs[0].decision,
            RunRetentionDecision::Protected {
                reason: reason.into()
            }
        );
        assert_eq!(preview.history_payload_bytes, 0);
        assert_eq!(
            runtime
                .expire_run_history(agent.run, preview.history_before)
                .unwrap(),
            RunExpiry::Protected(reason)
        );
        assert!(runtime.store.agent(agent.id).unwrap().is_some());
        runtime.sched.lock().running.remove(&agent.id);
        runtime.engine_turns.lock().remove(&agent.id);
        runtime.resources.release_quarantine(agent.id);
    }
}

#[tokio::test]
async fn preview_artifact_bytes_count_shared_blobs_once_and_match_eviction() {
    use aether_core::{
        artifact::{Artifact, ArtifactKind},
        event::Actor,
        id::ArtifactId,
    };
    let (runtime, agent, _dir) = fixture().await;
    let id = ArtifactId::new();
    let path = runtime
        .store
        .write_artifact_bytes(id, b"shared payload")
        .unwrap();
    let artifact = Artifact {
        id,
        kind: ArtifactKind::File,
        name: "old".into(),
        path: Some(path),
        summary: String::new(),
        size_bytes: 14,
        content_type: None,
        digest: Some("shared".into()),
        created_by: Actor::agent(agent.id),
        agent_id: Some(agent.id),
        task_id: None,
        created_at: now() - chrono::Duration::days(60),
        evicted_at: None,
    };
    let mut twin = artifact.clone();
    twin.id = ArtifactId::new();
    for a in [&artifact, &twin] {
        runtime
            .apply(Commit::new().mutate(Mutation::PutArtifact(a.clone())))
            .unwrap();
    }
    let seq = runtime.store.last_seq().unwrap();
    let preview = runtime.preview_retention().unwrap();
    assert_eq!(preview.eligible_artifacts.len(), 2);
    assert_eq!(preview.artifact_payload_bytes, 14);
    assert_eq!(
        preview.runs[0].decision,
        RunRetentionDecision::Protected {
            reason: "artifact".into()
        }
    );
    assert_eq!(
        runtime.store.read_artifact_bytes(id).unwrap().unwrap(),
        b"shared payload"
    );
    assert_eq!(runtime.store.last_seq().unwrap(), seq);
    assert_eq!(
        runtime.sweep_artifacts().unwrap(),
        preview.eligible_artifacts.len()
    );
    assert!(runtime.store.read_artifact_bytes(id).unwrap().is_none());
}

#[tokio::test]
async fn preview_simulates_cross_run_dependencies_in_sweep_order_without_source_writes() {
    use aether_core::{
        event::{Actor, EventDraft, EventKind},
        id::EventId,
        task::{Task, TaskResult, TaskStatus},
    };
    use aether_store::{EventFilter, ImportedEvent};

    // Forward references unlock the later run; reverse references are not revisited.
    // A runtime-protected earlier run must keep protecting the later run.
    for (forward, held) in [(true, false), (false, false), (true, true)] {
        let (runtime, old, _dir) = fixture().await;
        let mut other = old.clone();
        other.id = AgentId::new();
        other.run = RunId::new();
        let mut agents = [old, other];
        agents.sort_by_key(|agent| agent.run);
        let mut tasks = agents.each_ref().map(|agent| {
            let mut task = Task::new("finished", Actor::System, agent.run, agent.updated_at);
            task.owner = Some(agent.id);
            task.status = TaskStatus::Completed {
                result: TaskResult::summary("done"),
            };
            task
        });
        let (source, target) = if forward { (0, 1) } else { (1, 0) };
        tasks[source].dependencies.push(tasks[target].id);
        let events = agents.each_ref().map(|agent| ImportedEvent {
            id: EventId::new(),
            at: agent.updated_at,
            draft: EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: "history".into(),
                    payload: serde_json::json!({"text": "retained until expiry"}),
                },
            )
            .agent(agent.id)
            .run(agent.run),
        });
        runtime
            .store
            .import(
                events.to_vec(),
                agents
                    .iter()
                    .cloned()
                    .map(Mutation::UpsertAgent)
                    .chain(tasks.iter().cloned().map(Mutation::UpsertTask))
                    .collect(),
            )
            .unwrap();
        if held {
            runtime
                .resources
                .quarantine(agents[0].id, vec![ResourceKey("file:/test".into())]);
        }
        let at = now();
        let before = at - chrono::Duration::days(7);
        assert_eq!(
            runtime
                .store
                .preview_run_history(agents[target].run, before)
                .unwrap()
                .unwrap()
                .decision,
            RunRetentionDecision::Protected {
                reason: "external task reference".into()
            }
        );
        let seq = runtime.store.last_seq().unwrap();
        let commits = runtime.store.commits();
        let history = runtime
            .store
            .events(0, 100, &EventFilter::default())
            .unwrap();
        let cleanup = runtime.store.pending_artifact_cleanup().unwrap();
        let preview = runtime.preview_retention_at(at).unwrap();
        assert_eq!(runtime.store.last_seq().unwrap(), seq);
        assert_eq!(runtime.store.commits(), commits);
        assert_eq!(runtime.store.pending_artifact_cleanup().unwrap(), cleanup);
        assert_eq!(
            runtime
                .store
                .events(0, 100, &EventFilter::default())
                .unwrap(),
            history
        );
        for (agent, task) in agents.iter().zip(&tasks) {
            assert_eq!(runtime.store.agent(agent.id).unwrap().as_ref(), Some(agent));
            assert_eq!(runtime.store.task(task.id).unwrap().as_ref(), Some(task));
        }
        let eligible: Vec<_> = preview
            .runs
            .iter()
            .filter(|run| run.decision == RunRetentionDecision::Eligible)
            .collect();
        assert_eq!(
            eligible.len(),
            if held {
                0
            } else if forward {
                2
            } else {
                1
            }
        );
        for run in &eligible {
            let index = agents
                .iter()
                .position(|agent| agent.run == run.run)
                .unwrap();
            let expected_bytes = serde_json::to_vec(&agents[index]).unwrap().len()
                + serde_json::to_vec(&tasks[index]).unwrap().len()
                + history
                    .iter()
                    .filter(|event| event.run == Some(run.run))
                    .map(|event| serde_json::to_vec(event).unwrap().len())
                    .sum::<usize>();
            assert_eq!(run.history_payload_bytes, expected_bytes as u64);
        }
        let sweep = runtime.sweep_run_history_at(at).unwrap();
        assert_eq!(sweep.expired_runs, eligible.len());
        assert_eq!(
            sweep.expired_events as u64,
            eligible.iter().map(|run| run.events).sum::<u64>()
        );
        assert_eq!(
            sweep.expired_rows as u64,
            eligible.iter().map(|run| run.rows).sum::<u64>()
        );
        assert_eq!(
            preview.history_payload_bytes,
            eligible
                .iter()
                .map(|run| run.history_payload_bytes)
                .sum::<u64>()
        );
        for agent in &agents {
            let deleted = eligible.iter().any(|run| run.run == agent.run);
            assert_eq!(runtime.store.agent(agent.id).unwrap().is_none(), deleted);
        }
        runtime.resources.release_quarantine(agents[0].id);
    }
}

#[tokio::test]
async fn sweep_skips_quietly_while_another_sweep_holds_the_lock() {
    let (runtime, _agent, _dir) = fixture().await;
    let held = runtime.storage_gc_lock.lock();
    assert!(matches!(
        runtime.storage_gc(false),
        Err(crate::RuntimeError::SweepBusy)
    ));
    worker::sweep(&runtime, true);
    assert!(runtime.problems().unwrap().is_empty());
    drop(held);
}
