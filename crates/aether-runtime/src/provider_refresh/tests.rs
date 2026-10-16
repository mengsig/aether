use super::*;
use aether_engine::{Engine, EngineError, EngineKind, EngineRun, EngineTurn};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, mpsc};

#[derive(Clone, Copy)]
enum Behavior {
    Usage,
    Failure,
    TurnFailure,
    Malformed,
    Unsupported,
    Wait,
}

struct ProbeEngine {
    behavior: Behavior,
    reads: AtomicUsize,
    turns: AtomicUsize,
    started: Notify,
    cancelled: Notify,
    cleanup: Notify,
}

impl ProbeEngine {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            reads: AtomicUsize::new(0),
            turns: AtomicUsize::new(0),
            started: Notify::new(),
            cancelled: Notify::new(),
            cleanup: Notify::new(),
        })
    }
}

fn payload() -> Value {
    json!({"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":2000000000}})
}

#[async_trait::async_trait]
impl Engine for ProbeEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }
    fn adapter(&self) -> String {
        "usage-test".into()
    }
    async fn read_usage(
        &self,
        _: &[String],
        cancel: CancellationToken,
    ) -> std::result::Result<Option<Value>, EngineError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        match self.behavior {
            Behavior::Usage => Ok(Some(payload())),
            Behavior::Failure => Err(EngineError::Exited("fixture failure".into())),
            Behavior::Unsupported | Behavior::TurnFailure => Ok(None),
            Behavior::Malformed => Ok(Some(json!({"bad":"payload"}))),
            Behavior::Wait => {
                cancel.cancelled().await;
                self.cancelled.notify_one();
                self.cleanup.notified().await;
                Err(EngineError::Exited("cancelled".into()))
            }
        }
    }
    async fn run(
        &self,
        turn: EngineTurn,
        _: CancellationToken,
    ) -> std::result::Result<EngineRun, EngineError> {
        assert!(turn.summary_only && turn.lean_host_context);
        assert!(turn.mcp.is_none() && turn.extra_dirs.is_empty());
        assert!(turn.session.is_none() && turn.cwd.is_dir());
        self.turns.fetch_add(1, Ordering::SeqCst);
        let (tx, events) = mpsc::channel(3);
        tx.send(aether_engine::EngineEvent::RateLimit { payload: payload() })
            .await
            .unwrap();
        let outcome = aether_engine::EngineOutcome {
            is_error: matches!(self.behavior, Behavior::TurnFailure),
            error: matches!(self.behavior, Behavior::TurnFailure).then(|| "turn failed".into()),
            ..Default::default()
        };
        tx.send(aether_engine::EngineEvent::Done(outcome))
            .await
            .unwrap();
        let (input, mut incoming) = mpsc::channel::<aether_engine::EngineInput>(1);
        tokio::spawn(async move {
            assert!(
                incoming.recv().await.is_none(),
                "probe cannot send another turn"
            );
            drop(tx);
        });
        Ok(EngineRun {
            events,
            input: Some(input),
            decisions: None,
            stop: None,
        })
    }
}

async fn runtime(engine: Arc<dyn Engine>, opt_in: bool) -> (Arc<Runtime>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[providers]\nusage_stale_after=900\nusage_refresh_timeout=1\n",
    )
    .unwrap();
    if opt_in {
        std::fs::write(
            dir.path().join("providers.toml"),
            "[providers.codex]\nusage_probe='turn'\n",
        )
        .unwrap();
    }
    let runtime = Runtime::start_with_engines(
        crate::RuntimeConfig::default(),
        Arc::new(aether_store::SqliteStore::open_in_memory().unwrap()),
        Arc::new(aether_model::Models::load(Some(dir.path().to_owned())).unwrap()),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        std::collections::HashMap::from([(EngineKind::CODEX, engine)]),
    )
    .await
    .unwrap();
    (runtime, dir)
}

async fn refresh(runtime: &Arc<Runtime>) -> Value {
    let data = runtime.refresh_provider_usage(Some("codex")).await.unwrap();
    data["providers"][0].clone()
}

async fn notified(notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(2), notify.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn refresh_records_windows_and_reset_then_caches_even_when_stale() {
    let engine = ProbeEngine::new(Behavior::Usage);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let first = refresh(&runtime).await;
    assert_eq!(first["status"], "refreshed");
    assert_eq!(first["headroom"]["windows"][0]["remaining_percent"], 58);
    assert!(first["headroom"]["windows"][0]["resets_at"].is_string());
    runtime
        .sched
        .lock()
        .quotas
        .get_mut("codex")
        .unwrap()
        .observed_at = now() - chrono::Duration::seconds(901);
    assert_eq!(refresh(&runtime).await["status"], "cached");
    assert_eq!(engine.reads.load(Ordering::SeqCst), 1);
    let events = runtime.store.events(0, 100, &Default::default()).unwrap();
    assert!(events.iter().any(|e| e.type_name() == "provider.quota"));
    runtime.shutdown().await;
}

#[tokio::test]
async fn unsupported_engine_never_turns_without_explicit_opt_in() {
    for opt_in in [false, true] {
        let engine = ProbeEngine::new(Behavior::Unsupported);
        let (runtime, _dir) = runtime(engine.clone(), opt_in).await;
        let row = refresh(&runtime).await;
        assert_eq!(
            row["status"],
            if opt_in { "refreshed" } else { "unsupported" }
        );
        assert_eq!(engine.turns.load(Ordering::SeqCst), usize::from(opt_in));
        if !opt_in {
            assert_eq!(row["headroom"]["stale"], true);
        }
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn errors_are_per_provider_without_changing_backoff_or_quota() {
    let engine = ProbeEngine::new(Behavior::Failure);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let row = refresh(&runtime).await;
    assert_eq!(row["status"], "error");
    assert!(row["error"].as_str().unwrap().contains("fixture failure"));
    assert!(runtime.sched.lock().backoff.is_empty());
    assert!(runtime.sched.lock().quotas.is_empty());
    assert_eq!(refresh(&runtime).await["status"], "cached");
    assert_eq!(engine.reads.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

#[tokio::test]
async fn backoff_and_exhaustion_claims_prevent_refresh_without_consuming_interval() {
    let engine = ProbeEngine::new(Behavior::Usage);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    runtime.sched.lock().backoff.insert(
        "codex".into(),
        crate::Backoff {
            until: Instant::now() + Duration::from_secs(60),
            until_at: now() + chrono::Duration::seconds(60),
            reason: "existing".into(),
        },
    );
    assert_eq!(refresh(&runtime).await["status"], "backoff");
    assert_eq!(runtime.sched.lock().backoff["codex"].reason, "existing");
    assert_eq!(engine.reads.load(Ordering::SeqCst), 0);
    runtime.sched.lock().backoff.clear();
    runtime
        .sched
        .lock()
        .probing
        .insert("codex".into(), aether_core::id::AgentId::new());
    assert_eq!(refresh(&runtime).await["status"], "in_flight");
    runtime.sched.lock().probing.clear();
    assert_eq!(refresh(&runtime).await["status"], "refreshed");
    runtime.shutdown().await;
}

#[tokio::test]
async fn concurrent_calls_and_caller_cancellation_hold_claim_until_cleanup() {
    let engine = ProbeEngine::new(Behavior::Wait);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let pending = tokio::spawn({
        let runtime = runtime.clone();
        async move { refresh(&runtime).await }
    });
    notified(&engine.started).await;
    assert_eq!(refresh(&runtime).await["status"], "in_flight");
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    notified(&engine.cancelled).await;
    assert_eq!(refresh(&runtime).await["status"], "in_flight");
    assert_eq!(engine.reads.load(Ordering::SeqCst), 1);
    engine.cleanup.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.sched.lock().usage_refresh["codex"].in_flight {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(refresh(&runtime).await["status"], "cached");
    assert!(runtime.sched.lock().backoff.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn timeout_cancels_and_waits_for_cleanup_without_setting_backoff() {
    let engine = ProbeEngine::new(Behavior::Wait);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    engine.cleanup.notify_one();
    let row = tokio::time::timeout(Duration::from_secs(3), refresh(&runtime))
        .await
        .unwrap();
    assert_eq!(row["status"], "error");
    assert!(row["error"].as_str().unwrap().contains("timed out"));
    assert!(!runtime.sched.lock().usage_refresh["codex"].in_flight);
    assert!(runtime.sched.lock().backoff.is_empty());
    assert_eq!(engine.reads.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

#[tokio::test]
async fn codex_app_server_refresh_records_a_new_reset_observation() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let program = fixture.path().join("fake-codex");
    std::fs::write(&program, r##"#!/bin/sh
set -eu
IFS= read -r init
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r initialized
IFS= read -r request
printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":2000000000}}}}'
while IFS= read -r message; do :; done
"##).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let engine = Arc::new(aether_engine::CodexAppEngine::new(
        program.to_string_lossy(),
        "test",
    ));
    let (runtime, _dir) = runtime(engine, false).await;
    let row = tokio::time::timeout(Duration::from_secs(3), refresh(&runtime))
        .await
        .unwrap();
    assert_eq!(row["status"], "refreshed", "{row}");
    let usage = runtime.sched.lock().quotas["codex"].clone();
    assert_eq!(
        usage.windows[0].quota.resets_at.unwrap().timestamp(),
        2000000000
    );
    assert!(usage.observed_at <= now());
    assert_eq!(refresh(&runtime).await["status"], "cached");
    runtime.shutdown().await;
}

#[tokio::test]
async fn concurrent_backoff_is_preserved_when_a_refresh_fails() {
    let engine = ProbeEngine::new(Behavior::Wait);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let pending = tokio::spawn({
        let runtime = runtime.clone();
        async move { refresh(&runtime).await }
    });
    notified(&engine.started).await;
    let until = Instant::now() + Duration::from_secs(60);
    runtime.sched.lock().backoff.insert(
        "codex".into(),
        crate::Backoff {
            until,
            until_at: now() + chrono::Duration::seconds(60),
            reason: "another turn exhausted".into(),
        },
    );
    engine.cleanup.notify_one();
    let row = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row["status"], "error");
    assert_eq!(runtime.sched.lock().backoff["codex"].until, until);
    assert_eq!(
        runtime.sched.lock().backoff["codex"].reason,
        "another turn exhausted"
    );
    assert!(runtime.sched.lock().quotas.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn unknown_provider_is_rejected_and_disabled_provider_is_never_probed() {
    let engine = ProbeEngine::new(Behavior::Usage);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    assert!(
        runtime
            .refresh_provider_usage(Some("missing"))
            .await
            .is_err()
    );
    assert!(
        runtime
            .refresh_provider_usage(Some("openai-codex"))
            .await
            .is_err()
    );
    assert_eq!(engine.reads.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn uncooperative_cleanup_quarantines_refreshes_instead_of_overlapping() {
    let engine = ProbeEngine::new(Behavior::Wait);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let row = tokio::time::timeout(Duration::from_secs(8), refresh(&runtime))
        .await
        .unwrap();
    assert_eq!(row["status"], "error");
    assert!(row["error"].as_str().unwrap().contains("quarantined"));
    assert!(!runtime.sched.lock().usage_refresh["codex"].in_flight);
    assert!(runtime.sched.lock().usage_refresh["codex"].quarantined);
    let repeated = refresh(&runtime).await;
    assert_eq!(repeated["status"], "quarantined");
    assert!(repeated["error"].as_str().unwrap().contains("restart"));
    assert_eq!(engine.reads.load(Ordering::SeqCst), 1);
    assert!(runtime.sched.lock().backoff.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn fresh_passive_observations_do_not_claim_or_probe() {
    let engine = ProbeEngine::new(Behavior::Failure);
    let (runtime, _dir) = runtime(engine.clone(), false).await;
    let usage = ProviderUsage::from_engine_payload(&payload(), now()).unwrap();
    runtime.sched.lock().quotas.insert("codex".into(), usage);
    let row = refresh(&runtime).await;
    assert_eq!(row["status"], "cached");
    assert_eq!(row["headroom"]["stale"], false);
    assert_eq!(engine.reads.load(Ordering::SeqCst), 0);
    assert!(runtime.sched.lock().usage_refresh.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn malformed_reads_and_failed_turns_do_not_publish_quota() {
    for behavior in [Behavior::Malformed, Behavior::TurnFailure] {
        let engine = ProbeEngine::new(behavior);
        let (runtime, _dir) = runtime(engine.clone(), true).await;
        let row = refresh(&runtime).await;
        assert_eq!(row["status"], "error");
        assert!(row["error"].is_string());
        assert!(runtime.sched.lock().quotas.is_empty());
        assert!(runtime.sched.lock().backoff.is_empty());
        assert_eq!(
            engine.turns.load(Ordering::SeqCst),
            usize::from(matches!(behavior, Behavior::TurnFailure))
        );
        runtime.shutdown().await;
    }
}
