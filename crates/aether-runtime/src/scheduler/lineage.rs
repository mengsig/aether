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

/// The limits a turn that gave its slot away for a native wait must fit
/// again before it resumes, beside the global and engine ones: its
/// provider's cap and every supervisor's subtree cap, counted as `pick` does.
pub(crate) struct ResumeLimits {
    provider: String,
    provider_cap: Option<usize>,
    /// `None` when the fleet could not be read: the turn then stays queued.
    lineage: Option<Lineage>,
}

impl ResumeLimits {
    /// Reads the store, so it is loaded outside the scheduler lock.
    pub(crate) fn load(runtime: &crate::Runtime, provider: &str) -> Self {
        debug_assert!(!provider.is_empty(), "a running turn has a provider");
        let provider_cap = runtime
            .models
            .registry()
            .provider(provider)
            .and_then(|p| p.max_concurrent)
            .map(|cap| cap as usize);
        let filter = aether_store::AgentFilter {
            live_only: true,
            ..Default::default()
        };
        let lineage = match runtime.store.agents(&filter) {
            Ok(live) => Some(Lineage::from_store(&live, runtime)),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the fleet to check subtree caps; still queued");
                None
            }
        };
        Self {
            provider: provider.to_owned(),
            provider_cap,
            lineage,
        }
    }

    /// Whether one of these limits is spent, so `agent` must keep queueing.
    pub(crate) fn held_back(&self, agent: AgentId, sched: &SchedulerState) -> bool {
        debug_assert!(
            sched.running.get(&agent).is_none_or(|r| !r.on_provider),
            "a resuming turn holds no slot, so it never counts against itself"
        );
        let on_provider = sched
            .on_provider()
            .filter(|(_, r)| r.provider == self.provider)
            .count();
        if self.provider_cap.is_some_and(|cap| on_provider >= cap) {
            return true;
        }
        self.lineage.as_ref().is_none_or(|lineage| {
            lineage.has_unreadable_ancestor(agent) || lineage.capped(agent, sched)
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
