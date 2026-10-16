//! An engine launch gets the agent's build cache, created before the engine
//! starts: the client's named one, else the run's partition of Aether's.
use super::*;

/// What one launch was handed, and whether each cache dir existed by then.
#[derive(Debug, Clone)]
struct Launch {
    build_env: Vec<(String, std::path::PathBuf)>,
    existed: bool,
}

struct CacheProbe {
    inner: CheckpointEngine,
    launches: Arc<Mutex<Vec<Launch>>>,
}

#[async_trait::async_trait]
impl Engine for CacheProbe {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "cache-probe/1".into()
    }
    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.launches.lock().unwrap().push(Launch {
            existed: turn.build_env.iter().all(|(_, dir)| dir.is_dir()),
            build_env: turn.build_env.clone(),
        });
        self.inner.run(turn, cancel).await
    }
}

fn agent_with_cache(h: &Harness, build_cache: Option<std::path::PathBuf>) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    build_cache,
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    agent.id
}

#[tokio::test]
async fn a_launch_gets_its_cache_created_named_or_partitioned() {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(CacheProbe {
        inner: CheckpointEngine {
            turns: Arc::default(),
            valid: true,
        },
        launches: launches.clone(),
    });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("aether-cache");
    let h = harness_with_engine_config(engine, |config| {
        config.build_cache_root = Some(root.clone());
    })
    .await;
    let named = temp.path().join("items/one");
    let id = agent_with_cache(&h, Some(named.clone()));
    assert!(
        !named.exists(),
        "nothing is created before a process needs it"
    );
    engine_turn(&h, id, "build").await;
    std::fs::remove_dir_all(&named).unwrap();
    engine_turn(&h, id, "build again").await;

    let plain = agent_with_cache(&h, None);
    engine_turn(&h, plain, "default cache").await;
    let plain = h.store.agent(plain).unwrap().unwrap();
    assert!(plain.scope.repository.is_some(), "{:?}", plain.scope);

    let seen = launches.lock().unwrap().clone();
    let [first, second, last] = <[_; 3]>::try_from(seen).expect("three launches");
    let expected = vec![("CARGO_TARGET_DIR".to_owned(), named.join("cargo-target"))];
    for launch in [&first, &second] {
        assert_eq!(launch.build_env, expected, "{launch:?}");
        assert!(launch.existed, "created, or recreated, first: {launch:?}");
    }
    let [(name, dir)] = <[_; 1]>::try_from(last.build_env.clone()).expect("one variable");
    assert_eq!(name, "CARGO_TARGET_DIR");
    assert!(dir.starts_with(&root) && last.existed, "{last:?}");
    assert!(
        dir.ends_with(format!("{}/cargo-target", plain.run)),
        "{last:?}"
    );
    assert!(!dir.starts_with(&named));
}
