//! Every engine launch of an agent, fresh, resumed or compacting, gets that
//! agent's own temp dir, created before the engine starts.
use super::*;

/// What one launch was handed, and whether its temp dir existed by then.
#[derive(Debug, Clone)]
struct Launch {
    summary_only: bool,
    resumed: bool,
    temp_dir: Option<std::path::PathBuf>,
    existed: bool,
}

struct TempProbe {
    inner: CheckpointEngine,
    launches: Arc<Mutex<Vec<Launch>>>,
}

#[async_trait::async_trait]
impl Engine for TempProbe {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "temp-probe/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launches.lock().unwrap().push(Launch {
            summary_only: turn.summary_only,
            resumed: turn.session.is_some(),
            existed: turn.temp_dir.as_ref().is_some_and(|dir| dir.is_dir()),
            temp_dir: turn.temp_dir.clone(),
        });
        self.inner.run(turn, cancel).await
    }
}

fn child_of(h: &Harness, parent: AgentId) -> AgentId {
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    effort: Some(Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    child.id
}

#[tokio::test]
async fn every_launch_gets_the_agents_own_temp_dir() {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(TempProbe {
        inner: CheckpointEngine {
            turns: Arc::default(),
            valid: true,
        },
        launches: launches.clone(),
    });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("t");
    let h = harness_with_engine_config(engine, |config| {
        aether_core::agent_temp::prepare_root(&root, &config.instance_id).unwrap();
        config.agent_temp_root = Some(root.clone());
    })
    .await;
    let id = chat_agent(&h);
    let leaf = aether_core::agent_temp::leaf_path(&root, id);
    assert!(
        !leaf.exists(),
        "nothing is created before a process needs it"
    );

    engine_turn(&h, id, "first").await;
    std::fs::write(leaf.join("kept"), b"scratch").unwrap();
    engine_turn(&h, id, "second").await;
    assert!(leaf.join("kept").exists(), "a later turn reuses the dir");
    std::fs::remove_dir_all(&leaf).unwrap();
    engine_turn(&h, id, "third").await;
    let before = h.store.agent(id).unwrap().unwrap().stats.turns;
    h.runtime.request_compaction(id, Actor::user()).unwrap();
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns > before && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    let seen = launches.lock().unwrap().clone();
    assert!(
        seen.iter().any(|l| !l.resumed && !l.summary_only),
        "{seen:?}"
    );
    assert!(seen.iter().any(|l| l.resumed), "{seen:?}");
    assert!(seen.iter().any(|l| l.summary_only), "{seen:?}");
    for launch in &seen {
        assert_eq!(launch.temp_dir.as_ref(), Some(&leaf), "{launch:?}");
        assert!(
            launch.existed,
            "created, or recreated, before the launch: {launch:?}"
        );
    }

    let child = child_of(&h, id);
    engine_turn(&h, child, "child").await;
    let child_leaf = aether_core::agent_temp::leaf_path(&root, child);
    assert_ne!(child_leaf, leaf);
    let last = launches.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.temp_dir, Some(child_leaf.clone()));
    assert!(child_leaf.is_dir() && leaf.is_dir());
}

#[tokio::test]
async fn a_temp_dir_that_is_not_the_agents_own_stops_the_turn() {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(TempProbe {
        inner: CheckpointEngine {
            turns: Arc::default(),
            valid: true,
        },
        launches: launches.clone(),
    });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("t");
    let h = harness_with_engine_config(engine, |config| {
        aether_core::agent_temp::prepare_root(&root, &config.instance_id).unwrap();
        config.agent_temp_root = Some(root.clone());
    })
    .await;
    let id = chat_agent(&h);
    let leaf = aether_core::agent_temp::leaf_path(&root, id);
    std::os::unix::fs::symlink(temp.path(), &leaf).unwrap();
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .unwrap();
    let mut reported = None;
    for _ in 0..750 {
        reported = h
            .store
            .events(0, 10_000, &EventFilter::for_agent(id))
            .unwrap()
            .into_iter()
            .map(|e| serde_json::to_string(&e.kind).unwrap())
            .find(|e| e.contains("agent temp dir") && e.contains("not a real directory"));
        if reported.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        reported.is_some(),
        "the refusal is recorded where an operator sees it"
    );
    assert!(
        launches.lock().unwrap().is_empty(),
        "no engine starts without its temp dir"
    );
    assert!(
        std::fs::symlink_metadata(&leaf)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
