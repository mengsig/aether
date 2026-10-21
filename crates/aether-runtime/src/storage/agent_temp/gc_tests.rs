use crate::Runtime;
use aether_core::{
    agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope},
    id::{AgentId, RunId},
    model::ModelRef,
};
use aether_store::{Commit, Mutation};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

struct Fixture {
    runtime: Arc<Runtime>,
    root: PathBuf,
    _temp: tempfile::TempDir,
    _config: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root =
            aether_core::agent_temp::prepare_root(&temp.path().join("t"), "gc-test").unwrap();
        let (runtime, config) = crate::test_runtime(|config| {
            config.instance_id = "gc-test".into();
            config.agent_temp_root = Some(root.clone());
        })
        .await;
        runtime.shutdown.cancel();
        tokio::task::yield_now().await;
        Self {
            runtime,
            root,
            _temp: temp,
            _config: config,
        }
    }
    fn owner(&self, status: AgentStatus) -> Agent {
        Agent {
            id: AgentId::new(),
            parent: None,
            role: Role::new("worker"),
            name: "log owner".into(),
            scope: Scope::default(),
            model: ModelRef::new("mock", "test"),
            fallbacks: vec![],
            status,
            paused: false,
            paused_via: None,
            current_task: None,
            archived: false,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(SystemTime::now().into()),
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats::default(),
            run: RunId::new(),
            created_at: SystemTime::now().into(),
            updated_at: SystemTime::now().into(),
        }
    }
    fn leaf(&self, agent: &Agent) -> PathBuf {
        self.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
        let path = aether_core::agent_temp::leaf_path(&self.root, agent.id);
        aether_core::agent_temp::ensure_agent_leaf(&path, agent.id, "gc-test").unwrap();
        path
    }
    fn gc(&self, dry_run: bool) -> serde_json::Value {
        self.runtime.gc_agent_temp(&self.root, dry_run).unwrap()
    }
}

#[tokio::test]
async fn live_and_failed_resumable_owners_are_kept_but_final_owners_are_removed() {
    let f = Fixture::new().await;
    let live = f.leaf(&f.owner(AgentStatus::Ready));
    let failed = f.leaf(&f.owner(AgentStatus::Failed {
        error: "retry me".into(),
    }));
    let terminal = f.leaf(&f.owner(AgentStatus::Completed { summary: None }));
    let cancelled = f.leaf(&f.owner(AgentStatus::Cancelled {
        reason: Some("done".into()),
    }));
    let mut restarted = f.owner(AgentStatus::Failed {
        error: "replaced".into(),
    });
    restarted.restarted_as = Some(AgentId::new());
    let replaced = f.leaf(&restarted);
    fs::create_dir(terminal.join("nested")).unwrap();
    fs::write(terminal.join("nested/scratch"), b"disposable").unwrap();
    let preview = f.gc(true);
    assert_eq!(
        preview["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["action"] == "would_remove")
            .count(),
        3
    );
    assert!(terminal.exists() && replaced.exists());
    f.gc(false);
    assert!(live.is_dir() && failed.is_dir());
    assert!(!terminal.exists() && !cancelled.exists() && !replaced.exists());
}

#[tokio::test]
async fn new_engine_turn_admission_before_the_gc_gate_keeps_the_leaf() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    let leaf = f.leaf(&owner);
    let sched = f.runtime.sched.lock();
    let runtime = f.runtime.clone();
    let root = f.root.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let gc = std::thread::spawn(move || {
        done_tx.send(runtime.gc_agent_temp(&root, false)).unwrap();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !f.runtime.commit_gate_held() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        f.runtime.commit_gate_held(),
        "GC must reach admission gate within two seconds"
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    f.runtime.engine_turns.lock().insert(
        owner.id,
        crate::turn_engine::EngineTurnHandle {
            token: "test".into(),
            tx,
        },
    );
    drop(sched);
    let report = done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("GC must finish in seconds")
        .unwrap();
    gc.join().unwrap();
    assert_eq!(report["entries"][0]["action"], "kept");
    assert!(leaf.is_dir());
    f.runtime.engine_turns.lock().remove(&owner.id);
    f.gc(false);
    assert!(!leaf.exists());
}
