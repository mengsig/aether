//! fixture scenarios and fixtures.
use super::*;

pub(super) struct Harness {
    pub(super) runtime: Arc<Runtime>,
    pub(super) client: Arc<ScriptedClient>,
    pub(super) store: Arc<dyn Store>,
    pub(super) _workspace: tempfile::TempDir,
    pub(super) _config_dir: tempfile::TempDir,
    pub(super) workspace_root: std::path::PathBuf,
    /// The `wait_until` hard ceiling for this harness: the gated 120s
    /// budget only when its script forks a real `bash`, else the old
    /// fast-fail 30s (most tests never spawn a subprocess).
    pub(super) wait_ceiling: Duration,
    /// Held for the harness's lifetime when its script forks a real `bash`;
    /// caps how many such tests run at once.
    pub(super) _bash_permit: Option<tokio::sync::SemaphorePermit<'static>>,
}

/// Caps concurrent real-`bash` kernel tests so this binary doesn't itself
/// fork a dozen login shells at once on a loaded host.
const REAL_BASH_CONCURRENCY: usize = 4;

/// Generous bound on acquiring `real_bash_gate()`: well past any legitimate
/// queue behind `REAL_BASH_CONCURRENCY` slots, so a future test that nests
/// two bash-gated harnesses on one thread fails loudly instead of wedging
/// the whole binary.
const REAL_BASH_GATE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(300);

fn real_bash_gate() -> &'static tokio::sync::Semaphore {
    static GATE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Semaphore::new(REAL_BASH_CONCURRENCY))
}

/// True if any scripted response asks the real `bash` tool to run — as
/// opposed to a seeded/synthetic event that merely names "bash" as a label.
pub(super) fn spawns_real_bash(responses: &[aether_model::testing::ScriptedResponse]) -> bool {
    responses.iter().any(|response| match response {
        Ok(events) => events.iter().any(|event| {
            matches!(
                event,
                StreamEvent::BlockStart {
                    block: BlockStart::ToolUse { name, .. },
                    ..
                } if name == "bash"
            )
        }),
        Err(_) => false,
    })
}

pub(super) async fn harness(responses: Vec<aether_model::testing::ScriptedResponse>) -> Harness {
    harness_with(responses, |_| {}).await
}

/// A harness whose retrieval keeps every memory in scope, for tests of
/// where memories reach rather than which are relevant to a message.
pub(super) async fn harness_uncut(
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> Harness {
    harness_configured(
        responses,
        |_| {},
        |config| {
            config.memory_retrieval.min_score = 0.0;
        },
    )
    .await
}

pub(super) async fn harness_with(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    seed: impl FnOnce(&dyn Store),
) -> Harness {
    harness_configured(responses, seed, |_| {}).await
}

pub(super) async fn harness_configured(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    seed: impl FnOnce(&dyn Store),
    configure: impl FnOnce(&mut RuntimeConfig),
) -> Harness {
    harness_with_tools(responses, seed, configure, |_| {}).await
}

pub(super) async fn harness_with_tools(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    seed: impl FnOnce(&dyn Store),
    configure: impl FnOnce(&mut RuntimeConfig),
    extra_tools: impl FnOnce(&mut ToolRegistry),
) -> Harness {
    harness_full(responses, seed, configure, None, None, None, extra_tools).await
}

/// Stands in for an installed CLI so `delegate` accepts an engine-backed
/// child. Turns still run through the scripted client (`client_override`
/// short-circuits engine dispatch), so this is never actually driven.
struct AbsentEngine(aether_engine::EngineKind);

#[async_trait::async_trait]
impl aether_engine::Engine for AbsentEngine {
    fn kind(&self) -> aether_engine::EngineKind {
        self.0
    }
    fn adapter(&self) -> String {
        "declared-but-never-run/1".into()
    }
    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_engine::EngineRun, aether_engine::EngineError> {
        panic!("a scripted kernel test must not reach a real engine")
    }
}

/// Every engine kind declared as installed, for tests about scheduling
/// engine-backed agents rather than about engines themselves.
pub(super) fn declared_engines()
-> std::collections::HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>> {
    [
        aether_engine::EngineKind::CLAUDE_CODE,
        aether_engine::EngineKind::CODEX,
    ]
    .into_iter()
    .map(|kind| {
        (
            kind,
            Arc::new(AbsentEngine(kind)) as Arc<dyn aether_engine::Engine>,
        )
    })
    .collect()
}

/// The full builder: `providers` is a `providers.toml` and `settings` a
/// `config.toml`, both written into the harness config directory before the
/// registry loads it; `store` reuses an existing one so a test can stand a
/// second daemon over the same state.
#[allow(clippy::too_many_arguments)]
pub(super) async fn harness_full(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    seed: impl FnOnce(&dyn Store),
    configure: impl FnOnce(&mut RuntimeConfig),
    providers: Option<&str>,
    settings: Option<&str>,
    store: Option<Arc<dyn Store>>,
    extra_tools: impl FnOnce(&mut ToolRegistry),
) -> Harness {
    harness_full_with_engines(
        responses,
        seed,
        configure,
        providers,
        settings,
        store,
        extra_tools,
        Default::default(),
    )
    .await
}

/// `harness_full` plus the engines the daemon should believe are installed.
/// Scripted turns never reach one (`client_override` short-circuits engine
/// dispatch); declaring them is what lets `delegate` accept an engine model.
#[allow(clippy::too_many_arguments)]
pub(super) async fn harness_full_with_engines(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    seed: impl FnOnce(&dyn Store),
    configure: impl FnOnce(&mut RuntimeConfig),
    providers: Option<&str>,
    settings: Option<&str>,
    store: Option<Arc<dyn Store>>,
    extra_tools: impl FnOnce(&mut ToolRegistry),
    engines: std::collections::HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>>,
) -> Harness {
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonical");
    let config_dir = tempfile::tempdir().expect("config dir");
    if let Some(toml) = providers {
        std::fs::write(config_dir.path().join("providers.toml"), toml).expect("providers.toml");
    }
    if let Some(toml) = settings {
        std::fs::write(config_dir.path().join("config.toml"), toml).expect("config.toml");
    }
    let store: Arc<dyn Store> =
        store.unwrap_or_else(|| Arc::new(SqliteStore::open_in_memory().expect("store")));
    seed(store.as_ref());
    let spawns_bash = spawns_real_bash(&responses);
    let bash_permit = if spawns_bash {
        Some(
            tokio::time::timeout(REAL_BASH_GATE_ACQUIRE_TIMEOUT, real_bash_gate().acquire())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "TIMEOUT: real_bash_gate() (capacity {REAL_BASH_CONCURRENCY}) was not \
                         acquired within {REAL_BASH_GATE_ACQUIRE_TIMEOUT:?} — likely a test \
                         nesting a second real-bash-gated Harness on the same thread, holding a \
                         permit the outer harness also needs\n{}",
                        system_load_snapshot()
                    )
                })
                .expect("real-bash gate semaphore is never closed"),
        )
    } else {
        None
    };
    let wait_ceiling = if spawns_bash {
        WAIT_UNTIL_BASH_HARD_CEILING
    } else {
        WAIT_UNTIL_FAST_HARD_CEILING
    };
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    let client = Arc::new(ScriptedClient::with_responses(responses));
    let mut config = RuntimeConfig {
        holder: "test".into(),
        max_concurrent_turns: 1,
        lease_ttl: Duration::from_secs(30),
        default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        workspace_root: Some(workspace_root.clone()),
        ..Default::default()
    };
    configure(&mut config);
    // Scripted kernel tests must not probe or depend on locally installed CLIs.
    let runtime = Runtime::start_with_engines(
        config,
        store.clone(),
        models,
        {
            let mut tools = ToolRegistry::with_builtins();
            extra_tools(&mut tools);
            Arc::new(tools)
        },
        Some(client.clone()),
        engines,
    )
    .await
    .expect("runtime");
    Harness {
        runtime,
        client,
        store,
        _workspace: workspace,
        _config_dir: config_dir,
        workspace_root,
        wait_ceiling,
        _bash_permit: bash_permit,
    }
}

impl Harness {
    /// Waits for `id`'s status to satisfy `pred`, within this harness's
    /// budget: its `wait_ceiling` (120s if it scripted a real
    /// `bash` call, else the old fast-fail 30s), stall window unchanged.
    pub(super) async fn wait_until(
        &self,
        id: AgentId,
        pred: impl Fn(&AgentStatus) -> bool,
    ) -> AgentStatus {
        wait_until_with_budget(
            self.store.as_ref(),
            id,
            pred,
            self.wait_ceiling,
            WAIT_UNTIL_STALL_WINDOW,
        )
        .await
    }

    /// Waits for a condition with no single agent to read progress from - a
    /// stream opening, a key being taken - on this harness's ceiling.
    pub(super) async fn wait_for(&self, what: &str, pred: impl Fn() -> bool) {
        wait_for_with_budget(
            self.wait_ceiling,
            self.wait_ceiling,
            || pred().then_some(()),
            || (),
            || what.to_owned(),
        )
        .await;
    }
}
