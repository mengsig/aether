use crate::SchedulerState;
use aether_core::{agent::Agent, id::AgentId};
use std::collections::{HashMap, HashSet};

/// Parent links and per-agent caps of the live fleet, for subtree limits.
pub(super) struct Lineage {
    parents: HashMap<AgentId, AgentId>,
    caps: HashMap<AgentId, usize>,
    unreadable: HashSet<AgentId>,
}

impl Lineage {
    /// Load only missing ancestors, not the entire historical fleet. Finished
    /// intermediates remain part of the tree and cannot release ancestor caps.
    pub(super) fn from_store(live: &[Agent], runtime: &crate::Runtime) -> Self {
        let mut lineage = Self::of(live);
        let mut seen: HashSet<_> = live.iter().map(|agent| agent.id).collect();
        let mut pending: Vec<_> = lineage.parents.values().copied().collect();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let agent = match runtime.store.agent(id) {
                Ok(Some(agent)) => agent,
                failed => {
                    runtime.report_problem(format!("ancestor.{id}"), format!(
                        "Ancestor {id} cannot be read ({failed:?}); its descendants cannot run. Run `aether doctor` and `aether diagnose`."
                    ));
                    lineage.unreadable.insert(id);
                    continue;
                }
            };
            runtime.clear_problem(&format!("ancestor.{id}"));
            if let Some(parent) = agent.parent {
                lineage.parents.insert(id, parent);
                pending.push(parent);
            }
            if let Some(cap) = agent.config.concurrency {
                lineage.caps.insert(id, cap);
            }
        }
        lineage
    }

    pub(super) fn of(live: &[Agent]) -> Self {
        let mut parents = HashMap::new();
        let mut caps = HashMap::new();
        for agent in live {
            if let Some(parent) = agent.parent {
                parents.insert(agent.id, parent);
            }
            if let Some(cap) = agent.config.concurrency {
                caps.insert(agent.id, cap);
            }
        }
        Self {
            parents,
            caps,
            unreadable: HashSet::new(),
        }
    }

    /// Ancestors of `id`, nearest first.
    fn ancestors(&self, id: AgentId) -> Vec<AgentId> {
        let mut out = Vec::new();
        let mut cursor = id;
        while let Some(parent) = self.parents.get(&cursor) {
            if out.contains(parent) {
                break; // never trust a cycle in stored links
            }
            out.push(*parent);
            cursor = *parent;
        }
        out
    }

    pub(super) fn has_unreadable_ancestor(&self, id: AgentId) -> bool {
        self.ancestors(id)
            .iter()
            .any(|id| self.unreadable.contains(id))
    }

    /// Whether some ancestor's cap is already met by its running descendants.
    pub(super) fn capped(&self, id: AgentId, sched: &SchedulerState) -> bool {
        let ancestors = self.ancestors(id);
        ancestors.iter().any(|ancestor| {
            let Some(cap) = self.caps.get(ancestor) else {
                return false;
            };
            let under = sched
                .on_provider()
                .filter(|(r, _)| self.ancestors(**r).contains(ancestor))
                .count();
            under >= *cap
        })
    }
}

/// How many of `id`'s descendants, at any depth, are consuming a concurrency
/// slot right now - exactly what `Lineage::capped` counts against a cap, so a
/// tool reply or a client projection never disagrees with what the scheduler
/// itself enforces. `live` is the caller's own fleet snapshot, so this makes
/// no store read of its own.
pub(crate) fn on_provider_descendants(
    id: AgentId,
    live: &[Agent],
    sched: &SchedulerState,
) -> usize {
    let lineage = Lineage::of(live);
    sched
        .on_provider()
        .filter(|(r, _)| lineage.ancestors(**r).contains(&id))
        .count()
}

/// The cap that actually holds a delegated child back from starting
/// immediately, if any - the same predicate `dispatch` admits it against
/// (`Lineage::capped`'s ancestor walk, then the daemon's global limit), so a
/// delegate advisory never says "it starts now" when the scheduler will in
/// fact queue it.
pub(crate) enum CapHolder {
    /// The spent cap: `agent` is the ancestor (nearest first) whose own
    /// cap is met, or `None` for the daemon's global limit.
    Spent {
        agent: Option<AgentId>,
        running: usize,
        cap: usize,
    },
    /// The daemon's cap on simultaneous engine turns is spent, and the child
    /// would run on an engine.
    EngineCap { running: usize, cap: usize },
    /// Nothing in the child's ancestry holds it back right now.
    Clear,
}

/// `parent` is the delegating agent - the child itself need not exist yet in
/// `live`, so a freshly created child is never double-counted against its
/// own cap.
///
/// `engine_cap` is the daemon's engine cap when the child would run on an
/// engine, and `None` otherwise - an API-model child is never held by it.
pub(crate) fn cap_holder(
    parent: AgentId,
    live: &[Agent],
    sched: &SchedulerState,
    global_cap: usize,
    engine_cap: Option<usize>,
) -> CapHolder {
    let lineage = Lineage::of(live);
    let mut chain = vec![parent];
    chain.extend(lineage.ancestors(parent));
    for ancestor in chain {
        let Some(&cap) = lineage.caps.get(&ancestor) else {
            continue;
        };
        let running = on_provider_descendants(ancestor, live, sched);
        if running >= cap {
            return CapHolder::Spent {
                agent: Some(ancestor),
                running,
                cap,
            };
        }
    }
    if let Some(cap) = engine_cap {
        let running = sched.on_engine();
        if running >= cap {
            return CapHolder::EngineCap { running, cap };
        }
    }
    let running = sched.on_provider().count();
    if running >= global_cap {
        return CapHolder::Spent {
            agent: None,
            running,
            cap: global_cap,
        };
    }
    CapHolder::Clear
}
