//! `usage_breakdown`: the spend recorded by `invocation.completed`, grouped.
//! Rows are keyed and ordered by text, so the same log always answers alike.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aether_core::event::{Event, EventKind};
use aether_core::id::AgentId;
use aether_core::model::{ModelRef, Usage};
use aether_core::time::Timestamp;
use aether_protocol::read_models::{UsageBreakdown, UsageGroup, UsageRow, UsageScope};
use aether_store::{AgentFilter, EventFilter, Store};

/// Invocations read from the log at once.
const PAGE: usize = 2_000;
/// The key of a group the invocation does not record, such as a missing run.
const UNKNOWN: &str = "unknown";

/// What the request asked for, checked at the boundary.
#[derive(Debug, Clone)]
pub struct UsageRequest {
    pub scope: UsageScope,
    pub by: UsageGroup,
    /// Inclusive.
    pub since: Option<Timestamp>,
    /// Exclusive.
    pub until: Option<Timestamp>,
    pub limit: usize,
}

/// One completed invocation's share of the breakdown.
struct Completed<'a> {
    event: &'a Event,
    model: &'a ModelRef,
    usage: Usage,
}

/// Which agents a scope keeps, resolved once before the log is read.
struct Agents {
    roles: HashMap<AgentId, String>,
    /// `None` keeps every agent.
    kept: Option<BTreeSet<AgentId>>,
}

pub fn usage_breakdown(
    store: &dyn Store,
    request: &UsageRequest,
) -> aether_store::Result<UsageBreakdown> {
    assert!(request.limit > 0, "a breakdown holds at least one row");
    assert!(
        request.scope.role_prefix.as_deref() != Some(""),
        "an empty prefix is refused at the boundary"
    );
    let agents = scoped_agents(store, request)?;
    let filter = EventFilter {
        types: vec!["invocation.completed".into()],
        run: request.scope.run,
        agent: request.scope.agent,
        exclude_children: request.scope.agent.is_some(),
        ..EventFilter::default()
    };
    let mut groups: BTreeMap<String, UsageRow> = BTreeMap::new();
    let mut total = Usage::default();
    let mut after = 0u64;
    loop {
        let page = store.events(after, PAGE, &filter)?;
        let Some(last) = page.last() else { break };
        assert!(last.seq > after, "the log pages forwards");
        after = last.seq;
        for completed in page.iter().filter_map(completed) {
            if !in_scope(&completed, request, &agents) {
                continue;
            }
            total += completed.usage;
            let key = group_key(&completed, request.by, &agents);
            let row = groups.entry(key.clone()).or_insert_with(|| UsageRow {
                key,
                usage: Usage::default(),
                invocations: 0,
            });
            row.usage += completed.usage;
            row.invocations += 1;
        }
        if page.len() < PAGE {
            break;
        }
    }
    let truncated = groups.len() > request.limit;
    let rows: Vec<UsageRow> = groups.into_values().take(request.limit).collect();
    debug_assert!(rows.windows(2).all(|pair| pair[0].key < pair[1].key));
    Ok(UsageBreakdown {
        rows,
        total,
        truncated,
    })
}

/// Every agent's role, and the agents a subtree or role scope keeps.
fn scoped_agents(store: &dyn Store, request: &UsageRequest) -> aether_store::Result<Agents> {
    let scope = &request.scope;
    let needs_roles = scope.role_prefix.is_some() || request.by == UsageGroup::Role;
    let roles: HashMap<AgentId, String> = if needs_roles {
        let all = store.agents(&AgentFilter {
            run: scope.run,
            exclude_archived: false,
            ..AgentFilter::default()
        })?;
        all.iter()
            .map(|agent| (agent.id, agent.role.as_str().to_owned()))
            .collect()
    } else {
        HashMap::new()
    };
    let mut kept = match scope.subtree {
        Some(root) => Some(store.subtree_ids(root)?),
        None => None,
    };
    if let Some(prefix) = scope.role_prefix.as_deref() {
        let matching = roles
            .iter()
            .filter(|(_, role)| role.starts_with(prefix))
            .map(|(id, _)| *id);
        kept = Some(match kept {
            Some(under) => matching.filter(|id| under.contains(id)).collect(),
            None => matching.collect(),
        });
    }
    Ok(Agents { roles, kept })
}

fn completed(event: &Event) -> Option<Completed<'_>> {
    match &event.kind {
        EventKind::InvocationCompleted { model, usage, .. } => Some(Completed {
            event,
            model,
            usage: *usage,
        }),
        _ => None,
    }
}

fn in_scope(completed: &Completed<'_>, request: &UsageRequest, agents: &Agents) -> bool {
    let at = completed.event.at;
    if request.since.is_some_and(|since| at < since)
        || request.until.is_some_and(|until| at >= until)
    {
        return false;
    }
    match (&agents.kept, completed.event.agent_id) {
        (None, _) => true,
        (Some(kept), Some(agent)) => kept.contains(&agent),
        (Some(_), None) => false,
    }
}

fn group_key(completed: &Completed<'_>, by: UsageGroup, agents: &Agents) -> String {
    let event = completed.event;
    match by {
        UsageGroup::Model => completed.model.to_string(),
        UsageGroup::Provider => completed.model.provider.clone(),
        UsageGroup::Role => event
            .agent_id
            .and_then(|agent| agents.roles.get(&agent).cloned())
            .unwrap_or_else(|| UNKNOWN.into()),
        UsageGroup::Day => event.at.format("%Y-%m-%d").to_string(),
        UsageGroup::Run => event
            .run
            .map_or_else(|| UNKNOWN.into(), |run| run.to_string()),
    }
}

#[cfg(test)]
#[path = "usage_breakdown_tests.rs"]
mod tests;
