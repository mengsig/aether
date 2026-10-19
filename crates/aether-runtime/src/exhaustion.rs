//! Exhaustion diagnostics are committed with the terminal transition, never
//! forwarded by an ancestor that may already be unable to run.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use aether_core::agent::{Agent, BlockReason, ExhaustionClass};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::inbox::InboxKind;
use aether_core::model::ModelRef;
use aether_core::time::now;
use aether_store::{AgentFilter, Commit, EventFilter};
use serde_json::{Value, json};

use crate::{CommitHold, Result, Runtime};

/// Live descendants named in one diagnostic; the total is always reported.
/// The diagnostic is copied into every ancestor's inbox and model context.
const MAX_LISTED_DESCENDANTS: usize = 32;

/// Custom events read back to find the agent's open exhaustion episode. A turn
/// writes a handful; a window this wide reaches past several of them. Missing
/// an older episode only restarts the schedule at the floor.
const EPISODE_LOOKBACK: usize = 200;

/// Completed invocations read back to see whether the spent link itself has
/// answered since its suspension. A turn writes a handful. Past this window
/// the `exhaustion.recovered` that a real recovery writes in the same commit
/// is what closes the episode, so the fence does not rest on this number.
const ANSWER_LOOKBACK: usize = 64;

/// Failure texts the turn loop writes for a model or engine link. Anything
/// else - an agent's own `fail_task` text, a refused turn - is never classified.
const MODEL_FAILURE_PREFIXES: [&str; 6] = [
    "model error",
    "model unavailable",
    "the model failed",
    "every link of the model chain failed",
    "claude-code failed:",
    "codex failed:",
];

/// Local runtime budgets, named by the scheduler's own stop reasons. Only
/// the runtime ends an agent with one: its own text in that shape proves nothing.
fn budget_category(error: &str, by: &Actor) -> Option<&'static str> {
    if !matches!(by, Actor::Scheduler | Actor::System) {
        return None;
    }
    if error.starts_with("token budget of ") {
        Some("token_budget")
    } else if error.starts_with("cost budget of $") {
        Some("cost_budget")
    } else {
        None
    }
}

struct ExhaustedCause {
    category: &'static str,
    detail: String,
    model: ModelRef,
}

/// Exhaustion needs provenance: a runtime budget stop, or a model/engine
/// failure whose own turn recorded a typed exhaustion class.
fn exhaustion_cause(
    runtime: &Runtime,
    agent: &Agent,
    error: &str,
    by: &Actor,
) -> Result<Option<ExhaustedCause>> {
    if let Some(category) = budget_category(error, by) {
        return Ok(Some(ExhaustedCause {
            category,
            detail: error.to_owned(),
            model: agent.model.clone(),
        }));
    }
    if !MODEL_FAILURE_PREFIXES
        .iter()
        .any(|prefix| error.starts_with(prefix))
    {
        return Ok(None);
    }
    recorded_cause(runtime, agent)
}

/// Walk the unanswered tail of the agent's latest turn, newest first. A
/// completed invocation fences earlier failures, an older turn never counts,
/// and only a typed class is trusted - message text is never reinterpreted.
fn recorded_cause(runtime: &Runtime, agent: &Agent) -> Result<Option<ExhaustedCause>> {
    let started = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["agent.started".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    let Some(EventKind::AgentStarted { turn }) = started.first().map(|event| &event.kind) else {
        return Ok(None);
    };
    let turn = *turn;
    let recent = runtime.store.latest_events(
        agent.chain().link_count() + 1,
        &EventFilter {
            types: vec!["invocation.failed".into(), "invocation.completed".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    for event in recent {
        let EventKind::InvocationFailed {
            invocation,
            class,
            error,
            ..
        } = event.kind
        else {
            break;
        };
        let Some(row) = runtime.store.invocation(invocation)? else {
            break;
        };
        if row.turn != turn {
            break;
        }
        if let Some(class) = class.filter(|class| recoverable_by_fallback(*class)) {
            return Ok(Some(ExhaustedCause {
                category: class.as_str(),
                detail: error,
                model: row.model,
            }));
        }
    }
    Ok(None)
}

/// Parent and task-creator ancestry, terminal ancestors included. A broken
/// link (missing row, cycle) ends its path and is reported, never fatal:
/// a diagnostic must not make the failure itself uncommittable.
fn responsible_ancestors(
    runtime: &Runtime,
    agent: &Agent,
    creator: Option<AgentId>,
    anomalies: &mut BTreeSet<String>,
) -> Result<BTreeMap<AgentId, Agent>> {
    let mut recipients = BTreeMap::new();
    for start in [agent.parent, creator.filter(|id| *id != agent.id)] {
        let mut path = BTreeSet::from([agent.id]);
        let mut cursor = start;
        while let Some(id) = cursor {
            if !path.insert(id) {
                anomalies.insert(format!("ancestry cycle at {id}"));
                break;
            }
            let Some(ancestor) = runtime.store.agent(id)? else {
                anomalies.insert(format!("ancestor {id} not found"));
                break;
            };
            cursor = ancestor.parent;
            recipients.insert(id, ancestor);
        }
    }
    Ok(recipients)
}

/// Live agents below `root` at any depth. Terminal intermediates are walked
/// too: a completed steward may still have running grandchildren.
fn live_descendants(
    runtime: &Runtime,
    root: AgentId,
    anomalies: &mut BTreeSet<String>,
) -> Result<Vec<Agent>> {
    let mut live = Vec::new();
    let mut pending = vec![root];
    let mut seen = BTreeSet::from([root]);
    while let Some(parent) = pending.pop() {
        for child in runtime.store.agents(&AgentFilter {
            parent: Some(parent),
            ..Default::default()
        })? {
            if !seen.insert(child.id) {
                anomalies.insert(format!("descendant cycle at {}", child.id));
                continue;
            }
            pending.push(child.id);
            if !child.status.is_terminal() {
                live.push(child);
            }
        }
    }
    Ok(live)
}

/// The still-open task this work belongs to, and the ancestors responsible for
/// it. One walk: a suspension and its recovery reach exactly the same inboxes.
fn responsible_for(
    runtime: &Runtime,
    agent: &Agent,
    anomalies: &mut BTreeSet<String>,
) -> Result<(Option<aether_core::task::Task>, BTreeMap<AgentId, Agent>)> {
    // A long-lived agent keeps its settled task as `current_task`; this
    // failure is not that task's, and its creator is no longer responsible.
    let open_task = agent
        .current_task
        .map(|id| runtime.store.task(id))
        .transpose()?
        .flatten()
        .filter(|task| !task.status.is_terminal());
    let creator = open_task.as_ref().and_then(|task| match task.creator {
        Actor::Agent { id } => Some(id),
        _ => None,
    });
    let recipients = responsible_ancestors(runtime, agent, creator, anomalies)?;
    Ok((open_task, recipients))
}

/// Who failed, on what, and which ancestors are responsible for the work: the
/// half of the diagnostic the terminal path and the suspend path share.
fn diagnostic(
    runtime: &Runtime,
    agent: &Agent,
    cause: &ExhaustedCause,
) -> Result<(Value, BTreeMap<AgentId, Agent>)> {
    let mut anomalies = BTreeSet::new();
    let (open_task, recipients) = responsible_for(runtime, agent, &mut anomalies)?;
    let live = live_descendants(runtime, agent.id, &mut anomalies)?;
    let listed: Vec<Value> = live
        .iter()
        .take(MAX_LISTED_DESCENDANTS)
        .map(|child| {
            json!({"agent": child.id, "task": child.current_task, "status": child.status.kind()})
        })
        .collect();
    let mut payload = json!({
        "source_agent": agent.id,
        "source_task": open_task.as_ref().map(|task| task.id),
        "provider": cause.model.provider,
        "model": cause.model.model,
        "category": cause.category,
        "original_error": aether_model::http::safe_provider_text(&cause.detail, 4000),
        "live_descendants_at_failure": listed,
        "live_descendant_count": live.len(),
        "notified_ancestors": recipients.keys().collect::<Vec<_>>(),
    });
    if !anomalies.is_empty() {
        tracing::warn!(agent = %agent.id, ?anomalies, "exhaustion delivery walked a broken hierarchy");
        payload["hierarchy_anomalies"] = json!(anomalies);
    }
    Ok((payload, recipients))
}

/// A budget stop as a sentence for the task error and the ancestors' inboxes;
/// the structured payload stays in the `exhaustion.failed` event.
fn budget_message(reason: &str, cancellation_requested: bool, payload: &Value) -> String {
    let live = payload["live_descendant_count"].as_u64().unwrap_or(0);
    assert!(
        payload.is_object(),
        "the diagnostic payload must be an object"
    );
    let subtree = if cancellation_requested {
        "its live subtree was cancelled with it"
    } else {
        "no new subtree cancellation was requested"
    };
    format!(
        "Stopped by its budget ({reason}); {subtree}, {live} descendant(s) were live at the stop. \
         Details are in the exhaustion.failed event."
    )
}

/// Return an enriched error and deliver it once per responsible ancestor.
/// The caller holds the terminal-row fence and commits everything atomically;
/// no process or filesystem is touched here.
pub(crate) fn prepare(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    agent: &Agent,
    error: &str,
    by: &Actor,
) -> Result<Option<String>> {
    let Some(cause) = exhaustion_cause(runtime, agent, error, by)? else {
        return Ok(None);
    };
    let (mut payload, recipients) = diagnostic(runtime, agent, &cause)?;
    let cancellation_requested = commit.events.iter().any(|event| {
        event.agent_id == Some(agent.id)
            && matches!(&event.kind, EventKind::Custom { name, .. } if name == "cancellation.requested")
    });
    payload["terminal_error"] = json!(aether_model::http::safe_provider_text(error, 4000));
    payload["subtree_cancellation_requested_in_transition"] = json!(cancellation_requested);
    payload["descendant_policy"] = json!(if cancellation_requested {
        "durable subtree cancellation requested"
    } else {
        "no new subtree cancellation requested here; inspect existing cancellation intent and \
         live descendants before continuing, resuming or cancelling"
    });
    payload["preservation"] = json!(
        "Runtime task/transcript/checkpoint records retained. A native \
            session left for a cross-engine fallback is named in its engine.session and context.handoff events. \
            Filesystem/worktree existence and native session resumability are not verified. \
            Restart or reassign after resolving the limit; inspect live descendants before resuming."
    );
    let enriched = if cause.category.ends_with("_budget") {
        budget_message(error, cancellation_requested, &payload)
    } else {
        format!("exhaustion: {payload}")
    };
    commit.push_event(
        EventDraft::new(
            by.clone(),
            EventKind::Custom {
                name: "exhaustion.failed".into(),
                payload,
            },
        )
        .agent(agent.id)
        .parent(agent.parent)
        .run(agent.run),
    );
    for ancestor in recipients.values() {
        runtime.push_inbox(
            hold,
            commit,
            ancestor,
            InboxKind::ChildFailed {
                agent: agent.id,
                error: enriched.clone(),
            },
            by.clone(),
        )?;
    }
    Ok(Some(enriched))
}

/// Native transient failures reach every responsible ancestor even if a parent cannot run.
pub(crate) fn prepare_native_failure(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    agent: &Agent,
    error: &str,
    by: &Actor,
) -> Result<bool> {
    if !["codex failed:", "claude-code failed:"]
        .iter()
        .any(|prefix| error.starts_with(prefix))
    {
        return Ok(false);
    }
    let recent = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["invocation.failed".into(), "invocation.completed".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    let Some(EventKind::InvocationFailed {
        invocation,
        class: Some(class),
        retryable: false,
        ..
    }) = recent.first().map(|event| &event.kind)
    else {
        return Ok(false);
    };
    if !native_transient(*class) {
        return Ok(false);
    }
    let Some(row) = runtime.store.invocation(*invocation)? else {
        return Ok(false);
    };
    if row.error.as_deref() != Some(error) {
        return Ok(false);
    }
    let started = runtime.store.latest_events(
        1,
        &EventFilter {
            types: vec!["agent.started".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    if !matches!(started.first().map(|event| &event.kind), Some(EventKind::AgentStarted { turn }) if *turn == row.turn)
    {
        return Ok(false);
    }
    let mut anomalies = BTreeSet::new();
    let (_, recipients) = responsible_for(runtime, agent, &mut anomalies)?;
    if !anomalies.is_empty() {
        tracing::warn!(agent = %agent.id, ?anomalies, "native failure delivery walked a broken hierarchy");
    }
    for ancestor in recipients.values() {
        runtime.push_inbox(
            hold,
            commit,
            ancestor,
            InboxKind::ChildFailed {
                agent: agent.id,
                error: error.to_owned(),
            },
            by.clone(),
        )?;
    }
    Ok(true)
}

/// A spent final link, as the turn saw it. Carried to `finish`, which is where
/// the suspension is committed under the row fence.
#[derive(Debug, Clone)]
pub(crate) struct Exhausted {
    pub model: ModelRef,
    pub class: ExhaustionClass,
    /// Sanitized provider text, already enriched with code and retry-after.
    pub detail: String,
    /// Only a provider-reported duration; reset prose is never parsed.
    pub retry_after: Option<Duration>,
}

/// Report a spent provider without changing a task already carried by another link.
pub(crate) fn record_incident(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    agent: &Agent,
    model: &ModelRef,
    error: &aether_core::model::ModelError,
    by: &Actor,
) -> Result<()> {
    if !recoverable_by_fallback(error.class) || incident_open(runtime, agent, model)? {
        return Ok(());
    }
    let (mut payload, recipients) = diagnostic(
        runtime,
        agent,
        &ExhaustedCause {
            category: error.class.as_str(),
            detail: error.to_string(),
            model: model.clone(),
        },
    )?;
    let verdict = if error.class == aether_core::model::ModelErrorClass::ContextLimit {
        "context limit exceeded; task outcome unchanged"
    } else {
        "provider exhausted; task outcome unchanged"
    };
    payload["verdict"] = json!(verdict);
    commit.push_event(custom(agent, by, "exhaustion.reported", payload.clone()));
    for ancestor in recipients.values() {
        runtime.push_inbox(
            hold,
            commit,
            ancestor,
            InboxKind::ExternalTrigger {
                source: "exhaustion".into(),
                payload: payload.clone(),
                operator: false,
            },
            by.clone(),
        )?;
    }
    Ok(())
}

fn incident_open(runtime: &Runtime, agent: &Agent, link: &ModelRef) -> Result<bool> {
    Ok(
        matches!(runtime.store.latest_exhaustion_incident(agent.id, link)?.map(|event| event.kind),
        Some(EventKind::Custom { name, .. }) if name == "exhaustion.reported"),
    )
}

pub(crate) fn recover_incident(
    runtime: &Runtime,
    commit: &mut Commit,
    agent: &Agent,
    link: &ModelRef,
    by: &Actor,
) -> Result<()> {
    if incident_open(runtime, agent, link)? {
        commit.push_event(custom(
            agent,
            by,
            "exhaustion.provider_recovered",
            json!({
                "provider": link.provider, "model": link.model,
            }),
        ));
    }
    Ok(())
}

/// Whether an `exhaustion.*` payload is about `link`. Every writer here puts
/// the spent or answered provider and model at the payload root.
fn names_link(payload: &Value, link: &ModelRef) -> bool {
    payload["provider"].as_str() == Some(link.provider.as_str())
        && payload["model"].as_str() == Some(link.model.as_str())
}

/// Whether `link` itself answered after `seq`. A fallback answering is another
/// link's news, so it leaves this one's episode alone.
fn link_answered_since(
    runtime: &Runtime,
    agent: &Agent,
    seq: u64,
    link: &ModelRef,
) -> Result<bool> {
    let completed = runtime.store.latest_events(
        ANSWER_LOOKBACK,
        &EventFilter {
            types: vec!["invocation.completed".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    Ok(completed
        .iter()
        .take_while(|event| event.seq > seq)
        .any(|event| {
            matches!(&event.kind, EventKind::InvocationCompleted { model, .. } if model == link)
        }))
}

/// The exhaustion episode this agent is still in **on `link`**, if any. An
/// episode is keyed by the spent link, not by the agent's turn: open means the
/// newest suspension-lifecycle event naming `link` suspended it and `link` itself has
/// not answered since. A fallback carrying the turn therefore leaves the spent
/// link's episode - its `since` and its probe count - standing (§5).
fn open_episode(runtime: &Runtime, agent: &Agent, link: &ModelRef) -> Result<Option<BlockReason>> {
    let customs = runtime.store.latest_events(
        EPISODE_LOOKBACK,
        &EventFilter {
            types: vec!["custom".into()],
            ..EventFilter::own_events(agent.id)
        },
    )?;
    let newest = customs.iter().find_map(|event| match &event.kind {
        EventKind::Custom { name, payload }
            if matches!(
                name.as_str(),
                "exhaustion.suspended"
                    | "exhaustion.probe_failed"
                    | "exhaustion.recovered"
                    | "exhaustion.failed"
            ) && names_link(payload, link) =>
        {
            Some((name.as_str(), event.seq, payload))
        }
        _ => None,
    });
    // Only this link's newest suspension event decides: anything but a
    // suspension - a `recovered`, a terminal `exhaustion.failed` - closes the
    // episode it belongs to.
    let Some((_, seq, payload)) = newest
        .filter(|(name, ..)| *name == "exhaustion.suspended" || *name == "exhaustion.probe_failed")
    else {
        return Ok(None);
    };
    if link_answered_since(runtime, agent, seq, link)? {
        return Ok(None);
    }
    Ok(serde_json::from_value(payload["reason"].clone()).ok())
}

/// Where a suspension parks the agent, and whether its ancestors have already
/// been told about this episode.
pub(crate) struct Suspension {
    pub reason: BlockReason,
    new_episode: bool,
}

/// Plan where a spent link parks the agent: a first suspension on that link
/// starts an episode at the floor, a failed probe continues the one it belongs
/// to - including across turns a fallback carried in between.
pub(crate) fn plan(runtime: &Runtime, agent: &Agent, cause: &Exhausted) -> Result<Suspension> {
    let at = now();
    let previous = open_episode(runtime, agent, &cause.model)?;
    let reason = BlockReason::Exhausted {
        model: cause.model.clone(),
        class: cause.class,
        since: match &previous {
            Some(BlockReason::Exhausted { since, .. }) => *since,
            _ => at,
        },
        reported_reset: cause.retry_after.and_then(|after| {
            let bounded = reported_interval(after, runtime.config.exhaustion_probe_floor);
            chrono::Duration::from_std(bounded)
                .ok()
                .map(|bounded| at + bounded)
        }),
        next_probe: next_probe(
            at,
            previous.as_ref(),
            cause.retry_after,
            runtime.config.exhaustion_probe_floor,
        ),
        probes: probes_after(previous.as_ref()),
        detail: aether_model::http::safe_provider_text(&cause.detail, 400),
    };
    Ok(Suspension {
        reason,
        new_episode: previous.is_none(),
    })
}

/// Record the planned suspension: the schedule in an event either way, and the
/// diagnostic to every responsible ancestor once per episode - not once per
/// probe, which would be noise they cannot act on. The caller holds the
/// `Running` row fence and commits everything atomically.
pub(crate) fn record(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    agent: &Agent,
    cause: &Exhausted,
    suspension: &Suspension,
    by: &Actor,
) -> Result<()> {
    let BlockReason::Exhausted {
        since,
        reported_reset,
        next_probe,
        probes,
        detail,
        ..
    } = &suspension.reason
    else {
        return Err(crate::RuntimeError::Internal(
            "a suspension must park the agent on an exhausted block".into(),
        ));
    };
    // `reason` carries the episode: the row loses it the moment the scheduler
    // releases the agent to probe, and the next suspension reads it back here.
    let mut payload = json!({
        "reason": suspension.reason,
        "provider": cause.model.provider,
        "model": cause.model.model,
        "class": cause.class.as_str(),
        "since": since,
        "reported_reset": reported_reset,
        "next_probe": next_probe,
        "probes": probes,
        "detail": detail,
    });
    if !suspension.new_episode {
        commit.push_event(custom(agent, by, "exhaustion.probe_failed", payload));
        return Ok(());
    }
    let (diagnostic, recipients) = diagnostic(
        runtime,
        agent,
        &ExhaustedCause {
            category: cause.class.as_str(),
            detail: cause.detail.clone(),
            model: cause.model.clone(),
        },
    )?;
    let Value::Object(fields) = diagnostic else {
        return Err(crate::RuntimeError::Internal(
            "the exhaustion diagnostic must be an object".into(),
        ));
    };
    for (key, value) in fields {
        payload[key] = value;
    }
    // The links walked before the one that suspended, named rather than counted:
    // which provider was tried is what a person reads this payload for.
    let chain = agent.chain();
    let tried: Vec<&ModelRef> = chain
        .links()
        .take_while(|link| **link != cause.model)
        .collect();
    payload["fallbacks_tried"] = json!(tried);
    payload["verdict"] = json!(
        "Suspended, not failed: the task stays active, descendants keep running, and the agent \
         probes this provider by itself until it answers. Resume it sooner from your client, \
         or cancel it; no work is lost meanwhile."
    );
    let enriched = format!("exhaustion: {payload}");
    commit.push_event(custom(agent, by, "exhaustion.suspended", payload));
    for ancestor in recipients.values() {
        runtime.push_inbox(
            hold,
            commit,
            ancestor,
            InboxKind::ChildSuspended {
                agent: agent.id,
                detail: enriched.clone(),
            },
            by.clone(),
        )?;
    }
    Ok(())
}

/// Close the agent's open exhaustion episode because a provider answered, and
/// tell the ancestors the suspension told. Appended to the commit carrying the
/// invocation that answered, so the event log is the fence (§5): once
/// `exhaustion.recovered` is that link's newest `exhaustion.*` event the
/// episode is no longer open, and a second completed invocation on it - this
/// turn or a later one, across a restart - finds nothing to close.
///
/// `answered` is the link that replied, and only its own episode closes here:
/// a fallback carrying the turn is not the spent link's recovery, so that
/// episode stays open with its probe count intact (§5) and nothing is
/// written. The event therefore always names the link whose quota was spent.
///
/// Returns the provider whose other suspended agents may now be released.
pub(crate) fn record_recovery(
    runtime: &Runtime,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    agent: &Agent,
    answered: &ModelRef,
    by: &Actor,
) -> Result<Option<String>> {
    recover_incident(runtime, commit, agent, answered, by)?;
    let Some(BlockReason::Exhausted { since, probes, .. }) =
        open_episode(runtime, agent, answered)?
    else {
        return Ok(None);
    };
    let mut anomalies = BTreeSet::new();
    let (_, recipients) = responsible_for(runtime, agent, &mut anomalies)?;
    if !anomalies.is_empty() {
        tracing::warn!(agent = %agent.id, ?anomalies, "exhaustion recovery walked a broken hierarchy");
    }
    let payload = json!({
        "provider": answered.provider,
        "model": answered.model,
        "since": since,
        "probes": probes,
        "notified_ancestors": recipients.keys().collect::<Vec<_>>(),
    });
    commit.push_event(custom(agent, by, "exhaustion.recovered", payload));
    for ancestor in recipients.values() {
        runtime.push_inbox(
            hold,
            commit,
            ancestor,
            InboxKind::ChildResumed { agent: agent.id },
            by.clone(),
        )?;
    }
    Ok(Some(answered.provider.clone()))
}

fn custom(agent: &Agent, by: &Actor, name: &str, payload: Value) -> EventDraft {
    EventDraft::new(
        by.clone(),
        EventKind::Custom {
            name: name.to_owned(),
            payload,
        },
    )
    .agent(agent.id)
    .parent(agent.parent)
    .run(agent.run)
}

mod classification;
mod probing;
pub(crate) use classification::{engine_error_class, native_transient, recoverable_by_fallback};
pub(crate) use probing::next_probe;
use probing::{probes_after, reported_interval};
#[cfg(test)]
mod tests;
