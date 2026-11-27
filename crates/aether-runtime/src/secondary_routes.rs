//! Secondary routes (`antigravity/claude-opus-5-5`) give way to the vendor's
//! own provider, CLI engine or signed-in API, whenever it is usable: at
//! creation, and again before a turn for an agent created while that was unknown.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::model::ModelRef;
use aether_model::{Models, secondary};
use aether_store::{Commit, Mutation};

use crate::{Result, Runtime, turn_engine};

/// How long a read of the API sign-ins routes turns. `aether login` runs in
/// another process, so the daemon cannot hear of a change and reads again.
const SIGN_IN_FRESH_FOR: Duration = Duration::from_secs(2);

/// The API originals (`anthropic`, `openai`) with a usable sign-in, and when
/// that read began and ended.
#[derive(Debug, Default)]
pub(crate) struct SignedInOriginals {
    providers: HashSet<String>,
    /// `(began, ended)`; `None` before the first read.
    read: Option<(Instant, Instant)>,
}

impl Runtime {
    /// Read again which API providers serving a secondary route first-hand
    /// are signed in. Reading a sign-in is async, so routing uses this copy.
    pub async fn refresh_signed_in_originals(&self) {
        let began = Instant::now();
        let providers = signed_in_originals(&self.models).await;
        let ended = Instant::now();
        debug_assert!(ended >= began, "a read ends after it begins");
        let mut cached = self.signed_in_originals.write();
        // Two reads may overlap: the one begun later knows more.
        if cached.read.is_some_and(|(other, _)| other > began) {
            return;
        }
        *cached = SignedInOriginals {
            providers,
            read: Some((began, ended)),
        };
    }

    /// Whether the cached sign-ins were read recently enough to route a turn.
    /// Measured from the read's end, so a slow read cannot hold agents forever.
    fn sign_ins_are_fresh(&self) -> bool {
        self.signed_in_originals
            .read()
            .read
            .is_some_and(|(_, ended)| ended.elapsed() < SIGN_IN_FRESH_FOR)
    }

    /// Test hook: forget when the sign-ins were read, as if they were old.
    #[cfg(test)]
    pub(crate) fn age_sign_ins(&self) {
        self.signed_in_originals.write().read = None;
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
        let cached = self.signed_in_originals.read();
        let signed_in = &cached.providers;
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
    pub(crate) fn preferred_fallbacks(
        &self,
        primary: &ModelRef,
        fallbacks: Vec<ModelRef>,
    ) -> Vec<ModelRef> {
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

/// Whether routing could move one of `agent`'s links now.
fn could_move(agent: &Agent) -> bool {
    let primary = agent.engine_session.is_none() && secondary::has_original(&agent.model);
    primary || agent.fallbacks.iter().any(secondary::has_original)
}

/// Before a turn: move `agent`'s secondary links to their original providers
/// when one became usable after creation, as for an agent created while
/// detection ran. `None` holds the agent until the API sign-ins are read again.
pub(crate) fn route_before_turn(runtime: &Runtime, agent: Agent) -> Result<Option<Agent>> {
    // A delegated child keeps the chain its parent's policy admitted.
    if agent.parent.is_some() || !could_move(&agent) {
        return Ok(Some(agent));
    }
    // A native wait holds its engine turn open under a Waiting status.
    if matches!(agent.status, AgentStatus::Running)
        || runtime.engine_turns.lock().contains_key(&agent.id)
    {
        return Ok(Some(agent));
    }
    if !runtime.sign_ins_are_fresh() {
        runtime.sign_in_read_wanted.store(true, Ordering::Release);
        return Ok(None);
    }
    let (model, fallbacks) = rerouted(runtime, &agent);
    if model == agent.model && fallbacks == agent.fallbacks {
        return Ok(Some(agent));
    }
    commit_route(runtime, agent, model, fallbacks).map(Some)
}

/// Write `agent`'s new chain, unless the agent moved since it was read.
fn commit_route(
    runtime: &Runtime,
    agent: Agent,
    model: ModelRef,
    fallbacks: Vec<ModelRef>,
) -> Result<Agent> {
    debug_assert_eq!(
        fallbacks.len(),
        agent.fallbacks.len(),
        "routing keeps every link"
    );
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

/// Read the API sign-ins again, off the scheduler's task, when a pass held an
/// agent for them. The pass after the read routes it.
pub(crate) fn read_wanted_sign_ins(runtime: &Arc<Runtime>) -> Option<tokio::task::JoinHandle<()>> {
    if !runtime.sign_in_read_wanted.swap(false, Ordering::AcqRel) {
        return None;
    }
    let runtime = runtime.clone();
    Some(tokio::spawn(async move {
        runtime.refresh_signed_in_originals().await;
        runtime.notify.notify_one();
    }))
}

#[cfg(test)]
mod tests;
