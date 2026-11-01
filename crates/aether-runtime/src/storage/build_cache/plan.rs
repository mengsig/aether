//! Which partitions a sweep evicts: a finished run's once its grace is over,
//! then, while the cache holds more than its budget, finished runs' and then
//! live runs' idle ones, oldest first. A partition in use always stays.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_core::id::AgentId;
use aether_core::time::Timestamp;

use super::{BuildCacheReport, PartitionEntry, resolve};

/// What the store says of one partition.
#[derive(Debug, Default)]
pub(super) struct Owners {
    /// Agents running, with work in flight or with a background process.
    pub(super) holders: Vec<AgentId>,
    /// Whether any agent of the run has not finished.
    pub(super) live: bool,
    pub(super) last_active: Option<Timestamp>,
}

/// What the store says of the cache: Aether's partitions and their runs,
/// and the directories clients named, which eviction must never reach.
#[derive(Debug, Default)]
pub(super) struct Claims {
    pub(super) owners: HashMap<PathBuf, Owners>,
    pub(super) clients: Vec<PathBuf>,
}

impl Claims {
    /// Whether `path` holds or lies inside a client's directory, comparing
    /// both as written and with links resolved.
    pub(super) fn touches_client(&self, path: &Path) -> bool {
        let real = resolve(path);
        self.clients.iter().any(|dir| {
            [path, real.as_path()]
                .into_iter()
                .any(|path| dir.starts_with(path) || path.starts_with(dir))
        })
    }
}

/// What a sweep weighs besides the store.
pub(super) struct Policy {
    pub(super) max_bytes: u64,
    pub(super) grace: Duration,
    pub(super) now: Timestamp,
    /// The pids inside each partition; `None` when processes cannot be read.
    pub(super) processes: Option<HashMap<PathBuf, Vec<u32>>>,
}

/// Where a partition stands in the eviction order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Kept,
    /// Its run finished longer than the grace ago: evicted whatever the size.
    Expired,
    /// Its run finished, or no agent of it is in the store.
    Finished,
    /// Its run is live, but nothing uses it now.
    Idle,
}

/// Mark the partitions to evict and return their indexes in
/// `report.entries`, finished runs' first. Every byte under the root counts
/// against the budget, Aether's or not; only Aether's are ever evicted.
pub(super) fn plan(report: &mut BuildCacheReport, claims: &Claims, policy: &Policy) -> Vec<usize> {
    let mut held = 0u64;
    let (mut expired, mut finished, mut idle) = (Vec::new(), Vec::new(), Vec::new());
    for (index, entry) in report.entries.iter_mut().enumerate() {
        held = held.saturating_add(entry.bytes);
        match classify(entry, claims, policy) {
            Class::Kept => {}
            Class::Expired => expired.push(index),
            Class::Finished => finished.push(index),
            Class::Idle => idle.push(index),
        }
    }
    let entries = &mut report.entries;
    let oldest_first = |a: &usize, b: &usize| {
        (entries[*a].last_used, &entries[*a].path).cmp(&(entries[*b].last_used, &entries[*b].path))
    };
    finished.sort_by(oldest_first);
    idle.sort_by(oldest_first);
    let mut chosen = expired;
    for &index in &chosen {
        held = held.saturating_sub(entries[index].bytes);
        entries[index].reason = "its run finished longer than the grace ago";
    }
    let over_budget = finished
        .into_iter()
        .map(|index| {
            (
                index,
                "a finished run's, oldest first, while over the byte budget",
            )
        })
        .chain(idle.into_iter().map(|index| {
            (
                index,
                "idle in a live run, oldest first, while over the byte budget",
            )
        }));
    for (index, reason) in over_budget {
        if held <= policy.max_bytes {
            break;
        }
        held = held.saturating_sub(entries[index].bytes);
        entries[index].reason = reason;
        chosen.push(index);
    }
    for &index in &chosen {
        let entry = &mut entries[index];
        debug_assert!(
            entry.run.is_some() && entry.leased_by.is_empty() && entry.used_by_pids.is_empty(),
            "{} is not evictable",
            entry.path.display()
        );
        entry.action = "would_evict";
    }
    report.bytes_kept = held;
    chosen
}

/// Settle what is known of one partition and where it stands. An unmarked
/// directory is Aether's only when an agent in the store builds into it:
/// a release before the marker made it.
fn classify(entry: &mut PartitionEntry, claims: &Claims, policy: &Policy) -> Class {
    let owner = claims.owners.get(&entry.path);
    if entry.run.is_none() {
        let (Some(run), Some(_)) = (entry.unmarked_run, owner) else {
            return Class::Kept;
        };
        entry.run = Some(run);
        entry.reason = "within the byte budget";
    }
    if let Some(owner) = owner {
        entry.leased_by.clone_from(&owner.holders);
        entry.last_used = entry.last_used.max(owner.last_active);
        entry.run_live = Some(owner.live);
    }
    if let Some(pids) = policy.processes.as_ref().and_then(|p| p.get(&entry.path)) {
        entry.used_by_pids.clone_from(pids);
    }
    if claims.touches_client(&entry.path) {
        entry.reason = "holds a client's build cache";
        return Class::Kept;
    }
    if !entry.leased_by.is_empty() {
        entry.reason = "in use by an agent of its run";
        return Class::Kept;
    }
    if !entry.used_by_pids.is_empty() {
        entry.reason = "in use by a running process";
        return Class::Kept;
    }
    match entry.run_live {
        Some(true) if policy.processes.is_none() => {
            entry.reason = "its run is live and processes cannot be checked on this host";
            Class::Kept
        }
        Some(true) => Class::Idle,
        Some(false) if expired(entry.last_used, policy) => Class::Expired,
        _ => Class::Finished,
    }
}

/// Whether `last_used` is longer than the grace ago; never used counts.
fn expired(last_used: Option<Timestamp>, policy: &Policy) -> bool {
    let Ok(grace) = chrono::Duration::from_std(policy.grace) else {
        return false;
    };
    last_used.is_none_or(|used| {
        used.checked_add_signed(grace)
            .is_some_and(|ends| ends <= policy.now)
    })
}
