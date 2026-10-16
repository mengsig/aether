//! What the build cache holds, and keeping it to its byte budget: Aether's
//! own partitions that no agent leases are evicted, least recently used first.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aether_core::id::{AgentId, RunId};
use aether_core::time::Timestamp;
use aether_store::AgentFilter;
use serde::Serialize;

use super::{Runtime, invalid};
use crate::build_cache::PARTITION_MARKER;

const POLICY: &str =
    "unleased partitions over the byte budget are evicted, least recently used first";

/// A cache is build output, far more entries than any other category, so its
/// walk gets its own, larger bound.
const MAX_CACHE_ENTRIES: usize = 4_000_000;

/// Directories open at once while walking; deeper ones are left uncounted.
const MAX_DEPTH: usize = 128;

/// The default budget, in percent of the cache's filesystem.
const DEFAULT_BUDGET_PERCENT: u64 = 15;

/// Evicted partitions are renamed in here under the commit hold and deleted
/// after it; what a failed deletion leaves is retried on the next sweep.
const TRASH: &str = ".evicting";

/// Marks the trash as Aether's own; an unmarked `.evicting` is never used.
const TRASH_MARKER: &str = ".aether-trash";

#[derive(Debug, Serialize)]
pub(super) struct BuildCacheReport {
    root: PathBuf,
    policy: &'static str,
    /// Allocated disk blocks, as `du` counts them.
    bytes: u64,
    /// What Aether's partitions may hold; unset when there is nothing to evict.
    max_bytes: Option<u64>,
    bytes_removed: u64,
    repositories: usize,
    /// One per repository and run.
    partitions: usize,
    /// Entries that could not be read, so `bytes` misses them.
    unreadable: usize,
    truncated: bool,
    entries: Vec<PartitionEntry>,
    /// Evictions that failed; their bytes stay on disk.
    errors: Vec<String>,
    #[serde(skip)]
    budget: usize,
}

#[derive(Debug, Serialize)]
struct PartitionEntry {
    path: PathBuf,
    /// Unset for a directory Aether does not create, which is never evicted.
    run: Option<RunId>,
    bytes: u64,
    /// The latest of its run's agent activity and a write at its top level.
    last_used: Option<Timestamp>,
    /// Live agents of the run, or ones with work in flight.
    leased_by: Vec<AgentId>,
    action: &'static str,
    reason: &'static str,
}

/// What the store says of one partition.
#[derive(Debug, Default)]
struct Owners {
    holders: Vec<AgentId>,
    last_active: Option<Timestamp>,
}

/// What the store says of the cache: Aether's partitions in use, and the
/// directories clients named, which eviction must never reach.
#[derive(Debug, Default)]
struct Claims {
    owners: HashMap<PathBuf, Owners>,
    clients: Vec<PathBuf>,
}

impl Claims {
    /// Whether `path` holds or lies inside a client's directory, comparing
    /// both as written and with links resolved.
    fn touches_client(&self, path: &Path) -> bool {
        let real = resolve(path);
        self.clients.iter().any(|dir| {
            [path, real.as_path()]
                .into_iter()
                .any(|path| dir.starts_with(path) || path.starts_with(dir))
        })
    }
}

impl Runtime {
    /// Report the cache under `root` and, unless `dry_run`, evict it to its
    /// budget. Leases are read and partitions moved aside under the commit
    /// hold, so no agent can start using one between the check and the move.
    pub(super) fn gc_build_cache(
        &self,
        root: &Path,
        dry_run: bool,
    ) -> crate::Result<BuildCacheReport> {
        debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
        let trash = root.join(TRASH);
        let mut report = scan(root).map_err(invalid)?;
        let partitions = report.entries.iter().any(|entry| entry.run.is_some());
        let leftovers = !dry_run && fs::symlink_metadata(&trash).is_ok();
        if !partitions && !leftovers {
            return Ok(report);
        }
        if partitions {
            report.max_bytes = Some(match self.models.config().build_cache.max_bytes {
                Some(bytes) => bytes,
                None => fs4::total_space(root).map_err(invalid)? / 100 * DEFAULT_BUDGET_PERCENT,
            });
        }
        let hold = (!dry_run).then(|| self.hold());
        let sched = self.sched.lock();
        let engine_turns = self.engine_turns.lock();
        let in_flight = self.in_flight_agents(&sched, &engine_turns)?;
        let claims = self.build_cache_claims(root, &in_flight)?;
        let chosen = match report.max_bytes {
            Some(max_bytes) => plan(&mut report, &claims, max_bytes),
            None => Vec::new(),
        };
        if dry_run {
            return Ok(report);
        }
        let left = trash_leftovers(&trash, &claims, &mut report.errors);
        let moved = move_to_trash(&mut report, &chosen, &trash);
        drop((engine_turns, sched, hold));
        for (index, path) in moved {
            match remove_partition(&path) {
                Ok(()) => report.bytes_removed += report.entries[index].bytes,
                Err(error) => report
                    .errors
                    .push(format!("removing {}: {error}", path.display())),
            }
        }
        for path in left {
            if let Err(error) = remove_partition(&path) {
                report
                    .errors
                    .push(format!("removing {}: {error}", path.display()));
            }
        }
        Ok(report)
    }

    /// Evict the cache now because the disk guard found the disk low, rather
    /// than at the next sweep. A sweep already running evicts it itself.
    pub(crate) fn evict_build_cache(&self) {
        let Some(root) = &self.config.build_cache_root else {
            return;
        };
        let Some(_sweep) = self.storage_gc_lock.try_lock() else {
            return;
        };
        match self.gc_build_cache(root, false) {
            Ok(report) => tracing::info!(
                bytes_removed = report.bytes_removed,
                errors = report.errors.len(),
                "disk is low; build cache evicted to its budget"
            ),
            Err(error) => tracing::warn!(%error, "disk is low; build cache eviction failed"),
        }
    }

    /// Each Aether partition under `root` an agent in the store builds into,
    /// with the agents that lease it and when its run was last active, and
    /// every directory a client named for its agents.
    fn build_cache_claims(
        &self,
        root: &Path,
        in_flight: &HashSet<AgentId>,
    ) -> crate::Result<Claims> {
        let mut claims = Claims::default();
        for agent in self.store.agents(&AgentFilter::default())? {
            if let Some(dir) = &agent.config.build_cache {
                claims.clients.push(resolve(dir));
                claims.clients.push(dir.clone());
            }
            let Some(partition) = crate::build_cache::aether_partition(root, &agent) else {
                continue;
            };
            let owner = claims.owners.entry(partition).or_default();
            if !agent.status.is_terminal() || in_flight.contains(&agent.id) {
                owner.holders.push(agent.id);
            }
            let active = agent
                .updated_at
                .max(agent.status_since.unwrap_or(agent.updated_at));
            owner.last_active = owner.last_active.max(Some(active));
        }
        Ok(claims)
    }
}

/// Mark the evictable partitions to evict, least recently used first, until
/// the rest of Aether's fit `max_bytes`. Evictable means Aether's, unleased
/// and holding no client's directory. Returns indexes in `report.entries`.
fn plan(report: &mut BuildCacheReport, claims: &Claims, max_bytes: u64) -> Vec<usize> {
    let mut held = 0u64;
    let mut evictable = Vec::new();
    for (index, entry) in report.entries.iter_mut().enumerate() {
        if entry.run.is_none() {
            continue;
        }
        held = held.saturating_add(entry.bytes);
        if let Some(owner) = claims.owners.get(&entry.path) {
            entry.leased_by.clone_from(&owner.holders);
            entry.last_used = entry.last_used.max(owner.last_active);
        }
        if claims.touches_client(&entry.path) {
            entry.reason = "holds a client's build cache";
        } else if !entry.leased_by.is_empty() {
            entry.reason = "leased by an agent of its run";
        } else {
            evictable.push(index);
        }
    }
    let entries = &mut report.entries;
    evictable.sort_by(|&a, &b| {
        (entries[a].last_used, &entries[a].path).cmp(&(entries[b].last_used, &entries[b].path))
    });
    let mut chosen = Vec::new();
    for index in evictable {
        if held <= max_bytes {
            break;
        }
        let entry = &mut entries[index];
        debug_assert!(
            entry.run.is_some() && entry.leased_by.is_empty(),
            "{} is not evictable",
            entry.path.display()
        );
        held = held.saturating_sub(entry.bytes);
        entry.action = "would_evict";
        entry.reason = "least recently used while over the byte budget";
        chosen.push(index);
    }
    chosen
}

/// `path` with every link in the part that exists resolved, and the rest
/// as written, so that two names for one directory compare equal.
fn resolve(path: &Path) -> PathBuf {
    debug_assert!(path.is_absolute(), "{} is not absolute", path.display());
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            return missing
                .iter()
                .rev()
                .fold(real, |real, part| real.join(part));
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return path.to_path_buf();
        };
        missing.push(name);
        existing = parent;
    }
}

/// Rename each chosen partition into `trash`, which is cheap enough for the
/// commit hold. Returns where each moved one went.
fn move_to_trash(
    report: &mut BuildCacheReport,
    chosen: &[usize],
    trash: &Path,
) -> Vec<(usize, PathBuf)> {
    if chosen.is_empty() {
        return Vec::new();
    }
    if let Err(error) = open_trash(trash, true) {
        report
            .errors
            .push(format!("trash {}: {error}", trash.display()));
        for &index in chosen {
            report.entries[index].action = "kept";
            report.entries[index].reason = "eviction failed";
        }
        return Vec::new();
    }
    let mut moved = Vec::new();
    for &index in chosen {
        let entry = &mut report.entries[index];
        debug_assert!(
            entry.run.is_some(),
            "{} is not Aether's",
            entry.path.display()
        );
        if !is_partition(&entry.path) {
            entry.action = "kept";
            entry.reason = "changed since the scan";
            report.errors.push(format!(
                "{} is no longer a marked directory; left in place",
                entry.path.display()
            ));
            continue;
        }
        let target = trash.join(ulid::Ulid::generate().to_string());
        match fs::rename(&entry.path, &target) {
            Ok(()) => {
                entry.action = "evicted";
                moved.push((index, target));
            }
            Err(error) => {
                entry.action = "kept";
                entry.reason = "eviction failed";
                report
                    .errors
                    .push(format!("moving {}: {error}", entry.path.display()));
            }
        }
    }
    moved
}

/// Whether `trash` is Aether's: a real directory holding its marker. With
/// `create`, a missing one is made and marked; anything else is refused.
fn open_trash(trash: &Path, create: bool) -> io::Result<bool> {
    match fs::symlink_metadata(trash) {
        Ok(meta) if meta.is_dir() && is_marked(trash, TRASH_MARKER) => Ok(true),
        Ok(_) => Err(io::Error::other(
            "exists but is not a directory Aether made",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            fs::DirBuilder::new().mode(0o700).create(trash)?;
            fs::File::create_new(trash.join(TRASH_MARKER))?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether `path` is itself a marked directory, not a link to one.
fn is_partition(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) && is_marked(path, PARTITION_MARKER)
}

/// Whether `dir` holds `marker` as a regular file, not a link.
fn is_marked(dir: &Path, marker: &str) -> bool {
    fs::symlink_metadata(dir.join(marker)).is_ok_and(|meta| meta.is_file())
}

/// The partitions earlier evictions moved aside but could not remove: only
/// marked ones in Aether's own trash that hold no client's directory.
fn trash_leftovers(trash: &Path, claims: &Claims, errors: &mut Vec<String>) -> Vec<PathBuf> {
    match open_trash(trash, false) {
        Ok(true) => {}
        Ok(false) => return Vec::new(),
        Err(error) => {
            errors.push(format!("trash {}: {error}", trash.display()));
            return Vec::new();
        }
    }
    let entries = match fs::read_dir(trash) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("reading {}: {error}", trash.display()));
            return Vec::new();
        }
    };
    let mut left = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("reading {}: {error}", trash.display()));
                continue;
            }
        };
        let path = entry.path();
        if entry.file_name() == TRASH_MARKER {
            continue;
        }
        let real_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !real_dir || !is_marked(&path, PARTITION_MARKER) || claims.touches_client(&path) {
            errors.push(format!(
                "{} is not an evicted partition; left in place",
                path.display()
            ));
            continue;
        }
        left.push(path);
    }
    left
}

/// Delete a marked partition, its marker last, so a deletion that fails
/// part way still leaves it marked for the next sweep to finish. Whatever
/// the rename moved is checked again: a link swapped in is never followed.
fn remove_partition(partition: &Path) -> io::Result<()> {
    if !is_partition(partition) {
        return Err(io::Error::other("not a marked directory; left in place"));
    }
    for entry in fs::read_dir(partition)? {
        let entry = entry?;
        if entry.file_name() == PARTITION_MARKER {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    fs::remove_file(partition.join(PARTITION_MARKER))?;
    fs::remove_dir(partition)
}

/// Bytes, repositories and partitions under `root`. Below the configured
/// root, links are never followed, and files a running build deletes
/// mid-walk are skipped. Every entry read, at any level, spends the budget.
fn scan(root: &Path) -> io::Result<BuildCacheReport> {
    debug_assert!(root.is_absolute(), "{} is not absolute", root.display());
    let mut report = BuildCacheReport {
        root: root.to_path_buf(),
        policy: POLICY,
        bytes: 0,
        max_bytes: None,
        bytes_removed: 0,
        repositories: 0,
        partitions: 0,
        unreadable: 0,
        truncated: false,
        entries: Vec::new(),
        errors: Vec::new(),
        budget: MAX_CACHE_ENTRIES,
    };
    match fs::metadata(root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    }
    for repository in report.real_dirs(root) {
        let name = repository.file_name().and_then(|name| name.to_str());
        if name == Some(TRASH) {
            continue;
        }
        let aether = name.is_some_and(crate::build_cache::is_repository_dir);
        report.repositories += 1;
        for partition in report.real_dirs(&repository) {
            report.partitions += 1;
            let run = partition
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<RunId>().ok())
                .filter(|_| aether && is_marked(&partition, PARTITION_MARKER));
            let before = report.bytes;
            let newest = report.add_tree(&partition);
            report.entries.push(PartitionEntry {
                path: partition,
                run,
                bytes: report.bytes - before,
                last_used: newest.map(Timestamp::from),
                leased_by: Vec::new(),
                action: "kept",
                reason: if run.is_some() {
                    "within the byte budget"
                } else {
                    "not a partition Aether creates"
                },
            });
        }
    }
    debug_assert!(report.budget <= MAX_CACHE_ENTRIES, "{}", report.budget);
    debug_assert_eq!(report.entries.len(), report.partitions);
    Ok(report)
}

impl BuildCacheReport {
    /// The directories directly in `dir`, links and files left out.
    fn real_dirs(&mut self, dir: &Path) -> Vec<PathBuf> {
        let Some(entries) = self.readable(fs::read_dir(dir)) else {
            return Vec::new();
        };
        let mut dirs = Vec::new();
        for entry in entries {
            if !self.spend() {
                break;
            }
            let Some(entry) = self.readable(entry) else {
                continue;
            };
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                dirs.push(entry.path());
            }
        }
        dirs
    }

    /// Add the blocks of everything under `top`, itself included, and give
    /// the newest change to `top` or its entries. Open directories are held
    /// one per level, so memory follows depth, not width.
    fn add_tree(&mut self, top: &Path) -> Option<SystemTime> {
        let meta = self.add_entry(top)?;
        let mut newest = meta.modified().ok();
        let Some(entries) = self.open_dir(top, &meta) else {
            return newest;
        };
        let mut open = vec![entries];
        while let Some(entries) = open.last_mut() {
            let Some(entry) = entries.next() else {
                open.pop();
                continue;
            };
            if !self.spend() {
                return newest;
            }
            let Some(entry) = self.readable(entry) else {
                continue;
            };
            let path = entry.path();
            let Some(meta) = self.add_entry(&path) else {
                continue;
            };
            if open.len() == 1 {
                newest = newest.max(meta.modified().ok());
            }
            let Some(child) = self.open_dir(&path, &meta) else {
                continue;
            };
            if open.len() == MAX_DEPTH {
                self.truncated = true;
                continue;
            }
            open.push(child);
        }
        newest
    }

    /// Count `path`'s own blocks.
    fn add_entry(&mut self, path: &Path) -> Option<fs::Metadata> {
        let meta = self.readable(fs::symlink_metadata(path))?;
        self.bytes = self.bytes.saturating_add(meta.blocks().saturating_mul(512));
        Some(meta)
    }

    /// The entries of `path` if it is a real directory.
    fn open_dir(&mut self, path: &Path, meta: &fs::Metadata) -> Option<fs::ReadDir> {
        if !meta.is_dir() {
            return None;
        }
        self.readable(fs::read_dir(path))
    }

    /// Take one entry from the budget, or mark the report truncated.
    fn spend(&mut self) -> bool {
        if self.budget == 0 {
            self.truncated = true;
            return false;
        }
        self.budget -= 1;
        true
    }

    /// What a read gave, if anything. A path removed mid-walk is expected;
    /// any other failure is counted so the total is known to be short.
    fn readable<T>(&mut self, read: io::Result<T>) -> Option<T> {
        match read {
            Ok(value) => Some(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                self.unreadable += 1;
                None
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
