//! An agent's spawn schedule. Each occurrence is a `delegate` call its creator
//! makes with `wait=false`: the child is admitted by the code that admits one
//! delegated from the creator's own turn, so the schedule adds no authority.

use aether_core::agent::{Agent, AgentConfig, HostContext};
use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_core::schedule::{ScheduleOutcome, SpawnSpec};
use aether_store::Commit;

use super::execution::{Fired, skipped};
use crate::commands::{Preflight, PreparedAgent};
use crate::delegation_admission::{
    ChildReason, ChildRequest, EngineRefusal, Slots, engine_refusal, engine_unavailable,
    prepare_child, route_request,
};
use crate::{CommitHold, Result, Runtime, RuntimeError};

/// What an occurrence has ready before the hold.
pub(super) struct Staged {
    /// The creator row everything here was decided against.
    creator: Agent,
    /// The model the child runs, once routed; `None` for a template refused
    /// before routing, whose refusal `child` holds.
    model: Option<ModelRef>,
    /// `None` when the occurrence had to wait, so nothing was prepared.
    child: Result<Option<PreparedAgent>>,
}

/// A new schedule's template, admitted against its creator as it was read.
pub(super) struct Checked {
    creator: Agent,
    /// The model the child was routed to and its engine checked for.
    model: ModelRef,
    child: PreparedAgent,
}

impl Checked {
    /// Recheck current authority and the prepared child under the schedule's hold.
    pub(super) fn check_held(
        mut self,
        runtime: &Runtime,
        hold: &CommitHold<'_>,
        template: &SpawnSpec,
    ) -> Result<()> {
        let creator = live_creator(runtime, self.creator.id)?;
        check_delegate_exposure(runtime, &creator)?;
        if !same_authority(&self.creator, &creator) {
            return Err(RuntimeError::Invalid(
                "your model, scope or delegation policy changed while this schedule was \
                 being created; create it again"
                    .into(),
            ));
        }
        // Engine detection may have changed since the template was checked.
        refuse_engine_block(runtime, &self.model, template, &creator)?;
        runtime.check_prepared(hold, &mut self.child, Preflight::delegated(&creator))
    }
}

/// The single-model `delegate` call `template` stands for.
fn child_request(template: &SpawnSpec, objective: String) -> Result<ChildRequest> {
    let refuse = |why: &str| Err(RuntimeError::Invalid(why.into()));
    let config = template.config.clone().unwrap_or_default();
    let (Some(model), Some(effort)) = (template.model.clone(), config.effort) else {
        return refuse(
            "an agent's spawn template names its model and effort, as `delegate` does: \
             each run is your child",
        );
    };
    let host_context = match config.host_context {
        HostContext::Lean => Some(HostContext::Lean),
        HostContext::Full => None,
    };
    let chosen_by_delegate = AgentConfig {
        instructions: config.instructions.clone(),
        effort: config.effort,
        leaf: config.leaf,
        host_context: config.host_context,
        ..AgentConfig::default()
    };
    if config != chosen_by_delegate || !template.fallbacks.is_empty() || template.approval.is_some()
    {
        return refuse(
            "an agent's spawn template takes only what `delegate` takes: role, name, \
             objective, instructions, model, effort, leaf, host_context and scope. Each run \
             is your child and inherits the rest from you",
        );
    }
    Ok(ChildRequest {
        role: template.role.clone(),
        name: template.name.clone(),
        objective,
        instructions: config.instructions,
        model,
        fallbacks: Vec::new(),
        effort,
        scope: template.scope.clone(),
        leaf: config.leaf,
        host_context,
        reason: ChildReason::Unsupported,
    })
}

/// The `delegate` call `template` stands for, on the route its child runs
/// under `creator`'s policy. Every engine check and the child use this chain.
pub(crate) fn routed_request(
    runtime: &Runtime,
    template: &SpawnSpec,
    objective: String,
    creator: &Agent,
    slots: Slots,
) -> Result<ChildRequest> {
    let mut request = child_request(template, objective)?;
    route_request(runtime, creator, &mut request, slots)?;
    debug_assert!(request.fallbacks.is_empty(), "a template names one model");
    debug_assert!(
        !aether_model::secondary::has_original(&request.model)
            || template.model.as_ref() == Some(&request.model),
        "a secondary route stays only as the template asked"
    );
    Ok(request)
}

/// The creator while it can still have children, or else why the schedule
/// ends: it never outlives its creator. The outer error is the store's.
fn creator_or_ending(
    runtime: &Runtime,
    id: AgentId,
) -> Result<std::result::Result<Agent, RuntimeError>> {
    let ended = |state: &dyn std::fmt::Display| {
        RuntimeError::Invalid(format!(
            "its creator {id} {state}: an agent's spawn schedule ends with the agent"
        ))
    };
    let Some(creator) = runtime.store.agent(id)? else {
        return Ok(Err(ended(&"no longer exists")));
    };
    debug_assert_eq!(creator.id, id);
    if creator.status.is_terminal() {
        return Ok(Err(ended(&format_args!("is {}", creator.status.kind()))));
    }
    Ok(Ok(creator))
}

fn live_creator(runtime: &Runtime, id: AgentId) -> Result<Agent> {
    creator_or_ending(runtime, id)?
}

fn check_delegate_exposure(runtime: &Runtime, creator: &Agent) -> Result<()> {
    const DELEGATE: &str = "delegate";
    let exposure = &creator.config.tools;
    if let Some(reason) = runtime
        .tools
        .withheld_reason(DELEGATE, exposure, &creator.scope)
    {
        return Err(RuntimeError::Invalid(format!(
            "delegation refused: a spawn schedule requires access to `{DELEGATE}`: {reason}"
        )));
    }
    if runtime.tools.get(DELEGATE).is_none() {
        return Err(RuntimeError::Invalid(format!(
            "delegation refused: a spawn schedule requires the `{DELEGATE}` tool"
        )));
    }
    Ok(())
}

/// What stops `model`, the template's routed model, from running a child now,
/// by what the daemon last detected: a tick cannot wait on the probe `delegate` runs.
fn engine_block(
    runtime: &Runtime,
    model: &ModelRef,
    template: &SpawnSpec,
    creator: &Agent,
) -> Result<Option<EngineRefusal>> {
    // An unknown model is refused when the child is prepared.
    let Ok(resolved) = runtime.models.resolve(model) else {
        return Ok(None);
    };
    let Some(kind) = crate::turn_engine::engine_kind(resolved.provider.protocol) else {
        return Ok(None);
    };
    let network = template
        .scope
        .as_ref()
        .map_or(creator.scope.network, |scope| scope.network);
    let detected = match runtime.engines.now() {
        Ok(detected) => detected,
        Err(RuntimeError::Starting(why)) => {
            return Ok(Some(EngineRefusal::Unavailable { kind, why }));
        }
        Err(error) => return Err(error),
    };
    let provider = resolved.provider.id.as_str();
    // No probe follows as in `delegate`, so only what a turn would launch.
    if detected.launchable(&kind).is_none() {
        return Ok(Some(engine_unavailable(&detected, kind, provider)));
    }
    let approvals_required = !runtime.config.yolo && !creator.config.approval.is_yolo();
    Ok(engine_refusal(
        &detected,
        kind,
        provider,
        network,
        approvals_required,
    ))
}

fn refused_by_engine(model: &ModelRef, refusal: &EngineRefusal) -> RuntimeError {
    RuntimeError::Invalid(format!(
        "delegation refused: {model} cannot run this child: {refusal}"
    ))
}

/// Refuse a new schedule whose engine could not run its child now. Nothing
/// else may probe an idle engine, so a probe is asked for the caller's next try.
fn refuse_engine_block(
    runtime: &Runtime,
    model: &ModelRef,
    template: &SpawnSpec,
    creator: &Agent,
) -> Result<()> {
    let Some(refusal) = engine_block(runtime, model, template, creator)? else {
        return Ok(());
    };
    if let EngineRefusal::Unavailable { kind, .. } = &refusal {
        runtime.engines.want_recheck(*kind);
    }
    Err(refused_by_engine(model, &refusal))
}

/// The child `creator` would get from the routed `request` now, unwritten:
/// refused when the same `delegate` call would be, the engine's verdict aside.
fn prepare(
    runtime: &Runtime,
    request: ChildRequest,
    creator: &Agent,
    slots: Slots,
) -> Result<PreparedAgent> {
    let child = prepare_child(runtime, creator, request, &Actor::agent(creator.id), slots)?;
    debug_assert_eq!(child.agent.parent, Some(creator.id));
    debug_assert_eq!(child.agent.run, creator.run, "a child joins its run");
    Ok(child)
}

/// A new schedule's template, checked against its creator as it is now.
pub(super) fn check_template(
    runtime: &Runtime,
    template: &SpawnSpec,
    creator: AgentId,
) -> Result<Checked> {
    let creator = live_creator(runtime, creator)?;
    check_delegate_exposure(runtime, &creator)?;
    let objective = template.objective.clone();
    let request = routed_request(runtime, template, objective, &creator, Slots::Counted)?;
    let model = request.model.clone();
    refuse_engine_block(runtime, &model, template, &creator)?;
    let child = prepare(runtime, request, &creator, Slots::Counted)?;
    Ok(Checked {
        creator,
        model,
        child,
    })
}

/// Firing and resuming both need a creator that can still have children.
pub(super) fn check_creator_live(runtime: &Runtime, creator: AgentId) -> Result<()> {
    live_creator(runtime, creator).map(|_| ())
}

/// Why this occurrence waits for the next one, its engine aside. None of
/// these ends the schedule: each clears without anyone changing it.
fn waits(
    runtime: &Runtime,
    creator: &Agent,
    role: &str,
    last_child: Option<AgentId>,
) -> Result<Option<String>> {
    if creator.paused {
        return Ok(Some("creator is paused".into()));
    }
    if let Some(reason) = previous_child_live(runtime, last_child)? {
        return Ok(Some(reason));
    }
    runtime.parallel_refusal(creator, role.trim())
}

/// What the engine lets an occurrence do, as it stands under the hold.
enum EngineVerdict {
    Launch,
    /// A probe asked for answered, and the engine still cannot be launched.
    Skip(String),
    /// It cannot be launched and no probe has answered yet: one is asked for.
    AwaitProbe,
}

/// The engine's verdict under the hold. An occurrence is skipped only on a
/// probe's answer, as `delegate` refuses only after its own probe.
fn engine_verdict(
    runtime: &Runtime,
    model: &ModelRef,
    template: &SpawnSpec,
    creator: &Agent,
) -> Result<EngineVerdict> {
    match engine_block(runtime, model, template, creator)? {
        None => Ok(EngineVerdict::Launch),
        Some(EngineRefusal::Unavailable { kind, why }) => {
            if runtime.engines.recheck_answered(kind) {
                return Ok(EngineVerdict::Skip(why));
            }
            runtime.engines.want_recheck(kind);
            Ok(EngineVerdict::AwaitProbe)
        }
        Some(refusal @ EngineRefusal::Policy(_)) => Err(refused_by_engine(model, &refusal)),
    }
}

/// The overlap rule: the previous child, a restart of it, or anything either
/// delegated is still live.
fn previous_child_live(runtime: &Runtime, last_child: Option<AgentId>) -> Result<Option<String>> {
    let mut cursor = last_child;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err(RuntimeError::Internal(format!(
                "cycle in the restarts of scheduled child {id}"
            )));
        }
        let subtree = runtime.store.subtree_agents(id)?;
        if let Some(live) = subtree.iter().find(|agent| agent.is_live()) {
            let state = if live.paused {
                "paused".to_owned()
            } else {
                live.status.kind().to_string()
            };
            return Ok(Some(format!(
                "previous child {id} is still live: {} is {state}",
                live.id
            )));
        }
        cursor = subtree
            .iter()
            .find(|agent| agent.id == id)
            .and_then(|agent| agent.restarted_as);
    }
    Ok(None)
}

/// Built outside the hold, since preparing an agent may run `git`. `None`:
/// the creator had ended, which only the hold may conclude, or the route
/// waits for the API sign-ins to be read again. Either way it stays due.
pub(super) fn stage(
    runtime: &Runtime,
    template: &SpawnSpec,
    objective: String,
    creator: AgentId,
    last_child: Option<AgentId>,
) -> Result<Option<Staged>> {
    let Ok(creator) = creator_or_ending(runtime, creator)? else {
        return Ok(None);
    };
    let secondary = template
        .model
        .as_ref()
        .is_some_and(aether_model::secondary::has_original);
    if secondary && runtime.hold_for_sign_ins() {
        return Ok(None);
    }
    // A full `max_parallel` is one of the waits, never this child's refusal.
    let request = routed_request(runtime, template, objective, &creator, Slots::LeftToCaller);
    let model = request.as_ref().ok().map(|request| request.model.clone());
    // The engine's verdict belongs to the hold: nothing is staged against it.
    let ready = waits(runtime, &creator, &template.role, last_child)?.is_none()
        && match &model {
            Some(model) => engine_block(runtime, model, template, &creator)?.is_none(),
            None => true,
        };
    let child = match ready {
        true => request
            .and_then(|request| prepare(runtime, request, &creator, Slots::LeftToCaller))
            .map(Some),
        false => Ok(None),
    };
    Ok(Some(Staged {
        creator,
        model,
        child,
    }))
}

/// What a child's admission was decided on: a change to any of it since the
/// occurrence was staged means the decision is stale.
fn same_authority(staged: &Agent, now: &Agent) -> bool {
    debug_assert_eq!(staged.id, now.id);
    staged.scope == now.scope
        && staged.config == now.config
        && staged.model == now.model
        && staged.fallbacks == now.fallbacks
}

impl Staged {
    /// The child to admit against `creator`, read under the hold. `None`:
    /// nothing was prepared, or its refusal was decided on a row since moved.
    fn current_child(self, creator: &Agent) -> Result<Option<PreparedAgent>> {
        debug_assert_eq!(self.creator.id, creator.id);
        match self.child {
            Ok(child) => Ok(child),
            Err(error @ RuntimeError::Store(_)) => Err(error),
            Err(_) if self.creator != *creator => Ok(None),
            Err(refusal) => Err(refusal),
        }
    }
}

/// Fire one occurrence under the hold its commit is applied under. `None`:
/// `staged` no longer matches the store, so the next pass stages it again.
pub(super) fn fire(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    creator: AgentId,
    template: &SpawnSpec,
    last_child: &mut Option<AgentId>,
    staged: Result<Option<Staged>>,
) -> Result<Option<Fired>> {
    let creator = live_creator(runtime, creator)?;
    // Ahead of every wait: a creator being cancelled gets no more children.
    crate::cancellation::check_parent(runtime, hold, creator.id)?;
    check_delegate_exposure(runtime, &creator)?;
    let Some(staged) = staged? else {
        return Ok(None);
    };
    if !same_authority(&staged.creator, &creator) {
        return Ok(None);
    }
    if let Some(reason) = waits(runtime, &creator, &template.role, *last_child)? {
        return Ok(Some(skipped(reason)));
    }
    // A template refused before routing has no engine to ask; its refusal follows.
    let verdict = match &staged.model {
        Some(model) => engine_verdict(runtime, model, template, &creator)?,
        None => EngineVerdict::Launch,
    };
    match verdict {
        EngineVerdict::Launch => {}
        EngineVerdict::Skip(reason) => return Ok(Some(skipped(reason))),
        // Still due: a pass after the probe's answer decides it.
        EngineVerdict::AwaitProbe => return Ok(None),
    }
    let Some(prepared) = staged.current_child(&creator)? else {
        return Ok(None);
    };
    if runtime.workspace_gc_reserves(&prepared.agent.scope) {
        return Ok(Some(skipped("workspace maintenance in progress")));
    }
    let outcome = admit(runtime, hold, commit, prepared, &creator)?;
    if let ScheduleOutcome::Spawned { agent, .. } = &outcome {
        *last_child = Some(*agent);
    }
    // A child's repository is its parent's: there is nothing to seed.
    Ok(Some((outcome, None)))
}

/// Add the child's creation to `commit`, as `delegate` admits one under its hold.
fn admit(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    prepared: PreparedAgent,
    creator: &Agent,
) -> Result<ScheduleOutcome> {
    let (child, task, created) =
        runtime.admit_prepared(hold, prepared, Preflight::delegated(creator), None)?;
    let task =
        task.ok_or_else(|| RuntimeError::Internal("a scheduled child must carry its task".into()))?;
    debug_assert_eq!(
        child.parent,
        Some(creator.id),
        "fired as its creator's child"
    );
    debug_assert_eq!(child.run, creator.run, "a child joins its creator's run");
    commit.events.extend(created.events);
    commit.mutations.extend(created.mutations);
    Ok(ScheduleOutcome::Spawned {
        agent: child.id,
        task: task.id,
        run: child.run,
    })
}
