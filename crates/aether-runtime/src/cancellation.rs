//! Durable cancellation intent. A request remains pending until its entire
//! stored subtree is terminal; a failed walk is retried before new dispatch.

use std::collections::{BTreeMap, HashSet};

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_store::{Commit, Mutation, StoreError};
use serde_json::json;

use crate::{CancelRequest, CommitHold, Result, Runtime, RuntimeError};

const JOURNAL: &str = "runtime.pending_cancellations";
type Pending = BTreeMap<AgentId, CancelRequest>;

fn load(runtime: &Runtime) -> Result<Pending> {
    match runtime.store.meta(JOURNAL)? {
        Some(body) => serde_json::from_str(&body).map_err(|error| {
            StoreError::Corrupt(format!(
                "invalid cancellation journal: {error}; run `aether doctor` and `aether diagnose`"
            ))
            .into()
        }),
        None => Ok(Pending::new()),
    }
}

fn mutation(pending: &Pending) -> Result<Mutation> {
    let value = serde_json::to_string(pending)
        .map_err(|error| RuntimeError::Internal(format!("encode cancellation journal: {error}")))?;
    Ok(Mutation::SetMeta {
        key: JOURNAL.into(),
        value,
    })
}

pub(crate) fn request(
    runtime: &Runtime,
    id: AgentId,
    reason: Option<String>,
    by: Actor,
) -> Result<()> {
    let hold = runtime.hold();
    let commit = prepare(runtime, &hold, id, reason, by)?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// Build intent under the caller's write hold, allowing a budget/deadline
/// transition and its subtree cancellation to become durable together.
pub(crate) fn prepare(
    runtime: &Runtime,
    _hold: &CommitHold<'_>,
    id: AgentId,
    reason: Option<String>,
    by: Actor,
) -> Result<Commit> {
    let agent = runtime
        .store
        .agent(id)?
        .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
    let mut pending = load(runtime)?;
    if pending.contains_key(&id) {
        return Ok(Commit::new());
    }
    pending.insert(
        id,
        CancelRequest {
            by: by.clone(),
            reason: reason.clone(),
        },
    );
    Ok(Commit::new().mutate(mutation(&pending)?).event(
        EventDraft::new(
            by,
            EventKind::Custom {
                name: "cancellation.requested".into(),
                payload: json!({"reason": reason}),
            },
        )
        .agent(id)
        .run(agent.run),
    ))
}

/// Called under the same commit hold as child creation. No child can commit
/// beneath a pending cancellation after the final walk decides it is empty.
pub(crate) fn check_parent(
    runtime: &Runtime,
    _hold: &CommitHold<'_>,
    parent: AgentId,
) -> Result<()> {
    let pending = load(runtime)?;
    let mut cursor = Some(parent);
    let mut seen = HashSet::new();
    while let Some(id) = cursor {
        if !seen.insert(id) {
            return Err(StoreError::Corrupt(format!("cyclic ancestry at {id}")).into());
        }
        if pending.contains_key(&id) {
            return Err(RuntimeError::Invalid(format!(
                "agent {id}'s subtree is being cancelled"
            )));
        }
        cursor = runtime
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?
            .parent;
    }
    Ok(())
}

pub(crate) fn pending_agents(runtime: &Runtime) -> Result<HashSet<AgentId>> {
    let pending = load(runtime)?;
    if pending.is_empty() {
        return Ok(HashSet::new());
    }
    let mut affected = HashSet::new();
    for id in pending.keys() {
        affected.insert(*id);
        affected.extend(runtime.store.subtree_ids(*id)?);
    }
    Ok(affected)
}

pub(crate) fn drain(runtime: &Runtime) -> Result<()> {
    let mut first_error = None;
    for id in load(runtime)?.keys().copied().collect::<Vec<_>>() {
        match process(runtime, id) {
            Ok(()) => runtime.clear_problem(&format!("cancellation.{id}")),
            Err(error) => {
                runtime.report_problem(format!("cancellation.{id}"), format!(
                    "Cancellation of {id} is still pending: {error}. Its subtree cannot run. Run `aether doctor` and `aether diagnose`."
                ));
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub(crate) fn process(runtime: &Runtime, id: AgentId) -> Result<()> {
    let Some(request) = load(runtime)?.remove(&id) else {
        return Ok(());
    };
    runtime.cancel_agent_now(id, request.reason, request.by.clone())?;

    // Creation checks the journal under this hold too, so the empty-tree
    // observation and clearing the intent form one admission boundary.
    let hold = runtime.hold();
    for member in runtime.store.subtree_ids(id)? {
        let agent = runtime.store.agent(member)?.ok_or_else(|| {
            StoreError::Corrupt(format!(
                "Cancellation member {member} is missing; run `aether doctor`"
            ))
        })?;
        if !agent.status.is_terminal() {
            return Ok(());
        }
    }
    let agent = runtime
        .store
        .agent(id)?
        .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
    let mut pending = load(runtime)?;
    if pending.remove(&id).is_some() {
        runtime.apply_held(
            hold,
            Commit::new().mutate(mutation(&pending)?).event(
                EventDraft::new(
                    request.by,
                    EventKind::Custom {
                        name: "cancellation.completed".into(),
                        payload: json!({}),
                    },
                )
                .agent(id)
                .run(agent.run),
            ),
        )?;
    }
    Ok(())
}
