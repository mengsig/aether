use super::*;
use crate::commands::CreateAgentSpec;
use aether_core::agent::AgentConfig;
use aether_core::model::Effort;
use aether_engine::{Engine, EngineKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct ClaudeCode;

#[async_trait::async_trait]
impl Engine for ClaudeCode {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "claude-code/test".into()
    }

    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<aether_engine::EngineRun, aether_engine::EngineError> {
        Err(aether_engine::EngineError::Unsupported(
            "a test engine".into(),
        ))
    }
}

/// A runtime whose engine detection has not finished, with an Anthropic
/// API key when `anthropic_key` is set.
async fn runtime(anthropic_key: bool) -> (Arc<Runtime>, tempfile::TempDir) {
    runtime_signed_in(Arc::new(AtomicBool::new(anthropic_key))).await
}

/// As [`runtime`], with an Anthropic API key while `signed_in` holds: the
/// test signs in and out by setting it.
async fn runtime_signed_in(signed_in: Arc<AtomicBool>) -> (Arc<Runtime>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("config dir");
    let models = aether_model::Models::load(Some(dir.path().to_path_buf()))
        .expect("models")
        .with_env(move |name| {
            (signed_in.load(Ordering::SeqCst) && name == "ANTHROPIC_API_KEY")
                .then(|| "sk-test".to_owned())
        });
    let runtime = Runtime::start_with_engines_inner(
        crate::RuntimeConfig {
            holder: "unit-test".into(),
            // Approvals would need detection; a yolo daemon creates without it.
            yolo: true,
            workspace_root: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
        Arc::new(models),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        crate::engine_access::EngineCatalog::default(),
    )
    .await
    .expect("runtime");
    (runtime, dir)
}

/// Route `agent` as a scheduler pass would, reading the sign-ins again
/// first when the pass holds it for them. The runtime's own scheduler may
/// route it first, so this answers the stored agent.
async fn route(runtime: &Arc<Runtime>, agent: Agent) -> Agent {
    let id = agent.id;
    if route_before_turn(runtime, agent.clone())
        .expect("routed")
        .is_none()
    {
        runtime.refresh_signed_in_originals().await;
        route_before_turn(runtime, agent).expect("routed");
    }
    runtime.store.agent(id).expect("read").expect("stored")
}

/// The stored agent once the runtime's scheduler gave it `model`.
async fn scheduled_onto(
    runtime: &Runtime,
    id: aether_core::id::AgentId,
    model: &ModelRef,
) -> Agent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        runtime.notify.notify_one();
        let stored = runtime.store.agent(id).expect("read").expect("stored");
        if stored.model == *model || Instant::now() > deadline {
            return stored;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn agy(model: &str) -> ModelRef {
    ModelRef::new(secondary::SECONDARY_PROVIDER, model)
}

fn parked(model: ModelRef, fallbacks: Vec<ModelRef>) -> CreateAgentSpec {
    CreateAgentSpec {
        model: Some(model),
        fallbacks,
        config: Some(AgentConfig {
            effort: Some(Effort::Low),
            ..Default::default()
        }),
        start: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn an_agent_created_during_detection_moves_to_its_original_before_a_turn() {
    let (runtime, _dir) = runtime(false).await;
    let spec = parked(agy("claude-opus-5-5"), vec![agy("claude-sonnet-5-5")]);
    let (agent, _) = runtime.create_agent(spec, Actor::user()).expect("created");
    assert_eq!(agent.model, agy("claude-opus-5-5"), "nothing is known yet");
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    route(&runtime, agent.clone()).await;
    let stored = runtime.store.agent(agent.id).unwrap().expect("stored");
    assert_eq!(
        stored.model,
        ModelRef::new("claude-code", "claude-opus-5-5")
    );
    assert_eq!(
        stored.fallbacks,
        vec![ModelRef::new("claude-code", "claude-sonnet-5-5")]
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_primary_with_an_engine_session_keeps_its_route() {
    let (runtime, _dir) = runtime(false).await;
    let spec = parked(agy("claude-opus-5-5"), vec![agy("claude-sonnet-5-5")]);
    let (mut agent, _) = runtime.create_agent(spec, Actor::user()).expect("created");
    agent.engine_session = Some("agy-session".into());
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .expect("session stored");
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    let routed = route(&runtime, agent).await;
    assert_eq!(
        routed.model,
        agy("claude-opus-5-5"),
        "no handoff mid-conversation"
    );
    assert_eq!(
        routed.fallbacks,
        vec![ModelRef::new("claude-code", "claude-sonnet-5-5")]
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_signed_in_api_original_takes_primary_and_fallback_routes() {
    let (runtime, _dir) = runtime(true).await;
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    assert_eq!(
        runtime.prefer_original(agy("claude-opus-5-5")),
        agy("claude-opus-5-5"),
        "a sign-in counts once it has been read"
    );
    runtime.refresh_signed_in_originals().await;
    let (model, fallbacks) =
        runtime.preferred_chain(agy("claude-opus-5-5"), vec![agy("claude-sonnet-5-5")]);
    assert_eq!(model, ModelRef::new("anthropic", "claude-opus-5-5"));
    assert_eq!(
        fallbacks,
        vec![ModelRef::new("anthropic", "claude-sonnet-5-5")]
    );
    // Under a subscription primary, an API fallback would move the bill.
    let (model, fallbacks) =
        runtime.preferred_chain(agy("gemini-4-neon"), vec![agy("claude-sonnet-5-5")]);
    assert_eq!(model, agy("gemini-4-neon"));
    assert_eq!(fallbacks, vec![agy("claude-sonnet-5-5")]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn the_original_engine_outranks_a_signed_in_api() {
    let (runtime, _dir) = runtime(true).await;
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    assert_eq!(
        runtime.prefer_original(agy("claude-opus-5-5")),
        ModelRef::new("claude-code", "claude-opus-5-5")
    );
    assert_eq!(
        runtime.prefer_original(agy("gemini-4-neon")),
        agy("gemini-4-neon"),
        "no original serves Gemini"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_login_after_parking_moves_the_agent_at_its_first_turn() {
    let signed_in = Arc::new(AtomicBool::new(false));
    let (runtime, _dir) = runtime_signed_in(signed_in.clone()).await;
    // Codex serves no Claude model, so only the API could take the route.
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let spec = parked(agy("claude-opus-5-5"), Vec::new());
    let (agent, _) = runtime.create_agent(spec, Actor::user()).expect("created");
    assert_eq!(
        agent.model,
        agy("claude-opus-5-5"),
        "signed out at creation"
    );
    signed_in.store(true, Ordering::SeqCst);
    // Not told of the login, the scheduler reads the sign-ins again itself.
    runtime.age_sign_ins();
    let anthropic = ModelRef::new("anthropic", "claude-opus-5-5");
    let routed = scheduled_onto(&runtime, agent.id, &anthropic).await;
    assert_eq!(routed.model, anthropic);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_logout_after_parking_keeps_the_secondary_route() {
    let signed_in = Arc::new(AtomicBool::new(true));
    let (runtime, _dir) = runtime_signed_in(signed_in.clone()).await;
    runtime.refresh_signed_in_originals().await;
    let spec = parked(agy("claude-opus-5-5"), Vec::new());
    let (agent, _) = runtime.create_agent(spec, Actor::user()).expect("created");
    assert_eq!(agent.model, agy("claude-opus-5-5"), "nothing is known yet");
    signed_in.store(false, Ordering::SeqCst);
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    runtime.age_sign_ins();
    let routed = route(&runtime, agent).await;
    assert_eq!(
        routed.model,
        agy("claude-opus-5-5"),
        "a stale sign-in must not route to an API signed out of"
    );
    runtime.shutdown().await;
}

/// A lead whose delegation policy admits only `workers`, if any.
fn lead(runtime: &Runtime, workers: &[ModelRef]) -> Agent {
    use aether_core::delegation::{DelegationPolicy, WorkerChoice};
    let policy = (!workers.is_empty()).then(|| DelegationPolicy {
        workers: workers
            .iter()
            .map(|model| WorkerChoice {
                model: Some(model.clone()),
                tier: None,
                effort: None,
            })
            .collect(),
        ..Default::default()
    });
    let mut spec = parked(agy("claude-opus-5-5"), Vec::new());
    spec.config = Some(AgentConfig {
        delegation_policy: policy,
        ..Default::default()
    });
    runtime.create_agent(spec, Actor::user()).expect("lead").0
}

fn child_request(
    model: ModelRef,
    fallbacks: Vec<ModelRef>,
) -> crate::delegation_admission::ChildRequest {
    crate::delegation_admission::ChildRequest {
        role: "worker".into(),
        name: None,
        objective: "Do the work".into(),
        instructions: None,
        model,
        fallbacks,
        effort: Effort::Low,
        scope: None,
        leaf: true,
        host_context: None,
        reason: crate::delegation_admission::ChildReason::Omitted,
    }
}

/// `request` as `delegate` routes it under `lead`'s policy.
fn routed_request(
    runtime: &Runtime,
    lead: &Agent,
    mut request: crate::delegation_admission::ChildRequest,
) -> (ModelRef, Vec<ModelRef>) {
    use crate::delegation_admission::{Slots, policy_refusal, route_request};
    route_request(runtime, lead, &mut request, Slots::Counted).expect("routed");
    let refusal = policy_refusal(runtime, lead, &request, Slots::Counted).expect("checked");
    assert_eq!(refusal, None, "the routed chain is the one admitted");
    (request.model, request.fallbacks)
}

#[tokio::test]
async fn delegation_routes_only_where_the_lead_policy_admits() {
    let (runtime, _dir) = runtime(false).await;
    let only_agy = lead(&runtime, &[agy("claude-sonnet-5-5")]);
    let names_original = lead(
        &runtime,
        &[ModelRef::new("claude-code", "claude-sonnet-5-5")],
    );
    let unbounded = lead(&runtime, &[]);
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let asked = || child_request(agy("claude-sonnet-5-5"), Vec::new());
    assert_eq!(
        routed_request(&runtime, &only_agy, asked()).0,
        agy("claude-sonnet-5-5"),
        "a policy naming only the agy route keeps it"
    );
    assert_eq!(
        routed_request(&runtime, &names_original, asked()).0,
        ModelRef::new("claude-code", "claude-sonnet-5-5")
    );
    assert_eq!(
        routed_request(&runtime, &unbounded, asked()).0,
        ModelRef::new("claude-code", "claude-sonnet-5-5")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn delegation_checks_the_fallback_a_subscription_primary_keeps() {
    let (runtime, _dir) = runtime(true).await;
    let unbounded = lead(&runtime, &[]);
    runtime
        .engines
        .finish_with(EngineKind::CODEX, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let request = child_request(agy("gemini-4-neon"), vec![agy("claude-sonnet-5-5")]);
    let (model, fallbacks) = routed_request(&runtime, &unbounded, request);
    assert_eq!(model, agy("gemini-4-neon"));
    assert_eq!(
        fallbacks,
        vec![agy("claude-sonnet-5-5")],
        "the API fallback would cross the bill, so preflight sees the agy link"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_delegated_child_is_not_routed_again_before_its_turn() {
    let (runtime, _dir) = runtime(false).await;
    let only_agy = lead(&runtime, &[agy("claude-sonnet-5-5")]);
    let mut spec = parked(agy("claude-sonnet-5-5"), Vec::new());
    spec.parent = Some(only_agy.id);
    let (child, _) = runtime.create_agent(spec, Actor::user()).expect("child");
    runtime
        .engines
        .finish_with(EngineKind::CLAUDE_CODE, Arc::new(ClaudeCode));
    runtime.refresh_signed_in_originals().await;
    let routed = route(&runtime, child).await;
    assert_eq!(
        routed.model,
        agy("claude-sonnet-5-5"),
        "only the delegation that admitted it may route a child"
    );
    runtime.shutdown().await;
}

mod schedules;
