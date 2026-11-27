//! Secondary routes (`antigravity/claude-opus-5-5`) give way to the vendor's
//! own provider, CLI engine or signed-in API, whenever it is usable: at
//! creation, and again before a turn for an agent created while that was unknown.

use std::collections::HashSet;

use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::model::ModelRef;
use aether_model::{Models, secondary};
use aether_store::{Commit, Mutation};

use crate::{Result, Runtime, turn_engine};

impl Runtime {
    /// Read again which API providers serving a secondary route first-hand
    /// are signed in. Reading a sign-in is async, so routing uses this copy.
    pub async fn refresh_signed_in_originals(&self) {
        let signed_in = signed_in_originals(&self.models).await;
        *self.signed_in_originals.write() = signed_in;
    }

    /// `model`, or its first-hand route when it is a secondary one and the
    /// vendor's own engine, or its signed-in API provider, is usable.
    /// Detection still running keeps `model`; a turn routes it again.
    pub(crate) fn prefer_original(&self, model: ModelRef) -> ModelRef {
        if !secondary::has_original(&model) {
            return model;
        }
        let Ok(detected) = self.engines.now() else {
            return model;
        };
        let registry = self.models.registry();
        let signed_in = self.signed_in_originals.read();
        let usable = |provider: &str| {
            let Some(def) = registry.provider(provider) else {
                return false;
            };
            match turn_engine::engine_kind(def.protocol) {
                Some(kind) => detected.launchable(&kind).is_some(),
                None => signed_in.contains(provider),
            }
        };
        match secondary::original_route(registry, &model, usable) {
            Some(original) => {
                tracing::debug!(%model, %original, "the original provider can serve a secondary route");
                original
            }
            None => model,
        }
    }

    /// `model` and `fallbacks` with each secondary link on its original
    /// provider where usable.
    pub(crate) fn preferred_chain(
        &self,
        model: ModelRef,
        fallbacks: Vec<ModelRef>,
    ) -> (ModelRef, Vec<ModelRef>) {
        let model = self.prefer_original(model);
        let fallbacks = self.preferred_fallbacks(&model, fallbacks);
        (model, fallbacks)
    }

    /// `fallbacks` behind `primary`, each secondary link on its original
    /// provider where usable, unless that would move a subscription
    /// `primary` onto another bill: such a link keeps its secondary route.
    fn preferred_fallbacks(&self, primary: &ModelRef, fallbacks: Vec<ModelRef>) -> Vec<ModelRef> {
        let links = fallbacks.len();
        let routed: Vec<ModelRef> = fallbacks
            .into_iter()
            .map(|link| {
                let routed = self.prefer_original(link.clone());
                let crosses_bill = routed != link
                    && self
                        .check_billing_class(primary, std::slice::from_ref(&routed))
                        .is_err();
                if crosses_bill { link } else { routed }
            })
            .collect();
        debug_assert_eq!(routed.len(), links, "routing never drops a link");
        routed
    }
}

/// The API originals (`anthropic`, `openai`) with a sign-in Aether can use.
async fn signed_in_originals(models: &Models) -> HashSet<String> {
    let registry = models.registry();
    let mut signed_in = HashSet::new();
    for provider in secondary::ORIGINAL_PROVIDERS {
        let is_api = registry
            .provider(provider)
            .is_some_and(|def| def.enabled && turn_engine::engine_kind(def.protocol).is_none());
        if is_api && models.auth_status(provider).await.configured {
            signed_in.insert(provider.to_owned());
        }
    }
    debug_assert!(
        signed_in.iter().all(|p| p != secondary::SECONDARY_PROVIDER),
        "Antigravity is never its own original"
    );
    signed_in
}

/// Whether `agent` names a secondary route that an original could take over.
fn has_secondary_link(agent: &Agent) -> bool {
    std::iter::once(&agent.model)
        .chain(&agent.fallbacks)
        .any(secondary::has_original)
}

/// Before a turn: move `agent`'s secondary links to their original providers
/// when one became usable after creation, as for an agent created while
/// detection ran. The primary moves only before the agent's first engine session.
pub(crate) fn route_before_turn(runtime: &Runtime, agent: Agent) -> Result<Agent> {
    if !has_secondary_link(&agent) || matches!(agent.status, AgentStatus::Running) {
        return Ok(agent);
    }
    // A native wait holds its engine turn open under a Waiting status.
    if runtime.engine_turns.lock().contains_key(&agent.id) {
        return Ok(agent);
    }
    let (model, fallbacks) = rerouted(runtime, &agent);
    if model == agent.model && fallbacks == agent.fallbacks {
        return Ok(agent);
    }
    let hold = runtime.hold();
    let Some(mut current) = runtime.store.agent(agent.id)? else {
        return Ok(agent);
    };
    // Anything that moved meanwhile is decided again on the next pass.
    if current.status != agent.status
        || current.model != agent.model
        || current.fallbacks != agent.fallbacks
        || current.engine_session != agent.engine_session
    {
        return Ok(agent);
    }
    debug_assert!(current.is_live(), "the scheduler routes live agents only");
    let model_changed = model != current.model;
    tracing::info!(agent = %current.id, from = %current.model, to = %model, "moving secondary routes to their original providers");
    current.model = model;
    current.fallbacks = fallbacks;
    current.updated_at = aether_core::time::now();
    let commit = Commit::new()
        .event(
            EventDraft::new(
                Actor::Scheduler,
                EventKind::AgentConfigUpdated {
                    name: None,
                    model: model_changed.then(|| current.model.clone()),
                    config: current.config.clone(),
                    scope: None,
                },
            )
            .agent(current.id)
            .run(current.run),
        )
        .mutate(Mutation::UpsertAgent(current.clone()));
    runtime.apply_held(hold, commit)?;
    Ok(current)
}

/// The chain `agent` should run now. A primary that already holds an engine
/// session keeps it: moving would hand its conversation off mid-flight.
fn rerouted(runtime: &Runtime, agent: &Agent) -> (ModelRef, Vec<ModelRef>) {
    let model = match agent.engine_session {
        Some(_) => agent.model.clone(),
        None => runtime.prefer_original(agent.model.clone()),
    };
    let fallbacks = runtime.preferred_fallbacks(&model, agent.fallbacks.clone());
    (model, fallbacks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CreateAgentSpec;
    use aether_core::agent::AgentConfig;
    use aether_core::model::Effort;
    use aether_engine::{Engine, EngineKind};
    use std::sync::Arc;

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
        let dir = tempfile::tempdir().expect("config dir");
        let models = aether_model::Models::load(Some(dir.path().to_path_buf()))
            .expect("models")
            .with_env(move |name| {
                (anthropic_key && name == "ANTHROPIC_API_KEY").then(|| "sk-test".to_owned())
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
        route_before_turn(&runtime, agent.clone()).expect("routed");
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
        let routed = route_before_turn(&runtime, agent).expect("routed");
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
}
