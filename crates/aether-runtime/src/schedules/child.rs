//! An agent's spawn schedule. Each occurrence is a `delegate` call its creator
//! makes with `wait=false`: the child is admitted by the code that admits one
//! delegated from the creator's own turn, so the schedule adds no authority.

use aether_core::agent::{Agent, AgentConfig, HostContext};
use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::schedule::{ScheduleOutcome, SpawnSpec};
use aether_store::Commit;

use super::execution::{Fired, skipped};
use crate::commands::{Preflight, PreparedAgent};
use crate::delegation_admission::{ChildReason, ChildRequest, Slots, prepare_child};
use crate::{CommitHold, Result, Runtime, RuntimeError};

/// What an occurrence has ready before the hold.
pub(super) struct Staged {
    /// The creator row everything here was decided against.
    creator: Agent,
    /// `None` when the occurrence had to wait, so nothing was prepared.
    child: Result<Option<PreparedAgent>>,
}

/// The `delegate` call `template` stands for, with `objective` as its task.
/// Refuses what `delegate` has no argument for.
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
        effort,
        scope: template.scope.clone(),
        leaf: config.leaf,
        host_context,
        reason: ChildReason::Unsupported,
    })
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

/// The child `creator` would get from `template` now, unwritten: refused
/// exactly when the same `delegate` call would be.
fn prepare(
    runtime: &Runtime,
    template: &SpawnSpec,
    objective: String,
    creator: &Agent,
    slots: Slots,
) -> Result<PreparedAgent> {
    let request = child_request(template, objective)?;
    prepare_child(runtime, creator, request, &Actor::agent(creator.id), slots)
}

/// A new schedule's template, checked against its creator as it is now.
pub(super) fn check_template(
    runtime: &Runtime,
    template: &SpawnSpec,
    creator: AgentId,
) -> Result<PreparedAgent> {
    let creator = live_creator(runtime, creator)?;
    let objective = template.objective.clone();
    prepare(runtime, template, objective, &creator, Slots::Counted)
}

/// Resuming needs a creator that can still have children.
pub(super) fn check_resumable(runtime: &Runtime, creator: AgentId) -> Result<()> {
    live_creator(runtime, creator).map(|_| ())
}

/// Why this occurrence waits for the next one. None of these ends the
/// schedule: each clears without anyone changing it.
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
/// the creator had ended, which only the hold may conclude.
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
    // A full `max_parallel` is one of the waits, never this child's refusal.
    let child = match waits(runtime, &creator, &template.role, last_child)? {
        Some(_) => Ok(None),
        None => prepare(runtime, template, objective, &creator, Slots::LeftToCaller).map(Some),
    };
    Ok(Some(Staged { creator, child }))
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

/// Fire one occurrence under the hold its commit is applied under. `None`:
/// `staged` no longer matches the store, so the next pass stages it again.
pub(super) fn fire(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    creator: AgentId,
    role: &str,
    last_child: &mut Option<AgentId>,
    staged: Result<Option<Staged>>,
) -> Result<Option<Fired>> {
    let creator = live_creator(runtime, creator)?;
    let Some(staged) = staged? else {
        return Ok(None);
    };
    if !same_authority(&staged.creator, &creator) {
        return Ok(None);
    }
    if let Some(reason) = waits(runtime, &creator, role, *last_child)? {
        return Ok(Some(skipped(reason)));
    }
    let Some(prepared) = staged.child? else {
        return Ok(None);
    };
    if runtime.workspace_gc_reserves(&prepared.agent.scope) {
        return Ok(Some(skipped("workspace maintenance in progress")));
    }
    let (child, task, created) =
        runtime.admit_prepared(hold, prepared, Preflight::delegated(), None)?;
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
    *last_child = Some(child.id);
    let outcome = ScheduleOutcome::Spawned {
        agent: child.id,
        task: task.id,
        run: child.run,
    };
    // A child's repository is its parent's: there is nothing to seed.
    Ok(Some((outcome, None)))
}
