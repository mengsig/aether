//! Conservative, bounded collection of private delegated Git worktrees.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use aether_core::agent::{Agent, AgentStatusKind, Scope};
use aether_core::id::AgentId;
use aether_store::AgentFilter;
use serde::{Deserialize, Serialize};

use crate::{Result, Runtime, RuntimeError, SchedulerState, workspace};

const MAX_CANDIDATES: usize = 16;
const MAX_DIRECTORY_ENTRIES: usize = 128;

#[derive(Default)]
pub(crate) struct WorkspaceGcCursor {
    root: Option<PathBuf>,
    directory: Option<fs::ReadDir>,
    pending: std::collections::VecDeque<PathBuf>,
}

struct Reservation<'a>(&'a Runtime);

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.0.workspace_gc_active.lock().take();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceGcAction {
    Kept,
    WouldRemove,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceGcEntry {
    pub workspace: PathBuf,
    /// File and symlink payload lengths, excluding shared Git objects.
    pub bytes: u64,
    /// False means `bytes` is a lower bound because inspection stopped early.
    pub bytes_complete: bool,
    pub action: WorkspaceGcAction,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WorkspaceGcReport {
    pub dry_run: bool,
    /// Totals cover only inspected candidates. See `truncated` and each
    /// entry's `bytes_complete`; they are not a complete disk usage census.
    pub bytes_kept: u64,
    pub bytes_removed: u64,
    pub bytes_eligible: u64,
    pub truncated: bool,
    pub workspaces: Vec<WorkspaceGcEntry>,
}

impl Runtime {
    /// Remove old, clean delegated worktrees whose saved state is retained
    /// elsewhere. Each candidate is independent; refusals appear in the report.
    pub fn gc_workspaces(&self, grace: Duration, dry_run: bool) -> Result<WorkspaceGcReport> {
        self.gc_workspaces_at(grace, dry_run, SystemTime::now())
    }

    fn gc_workspaces_at(
        &self,
        grace: Duration,
        dry_run: bool,
        now: SystemTime,
    ) -> Result<WorkspaceGcReport> {
        let mut report = WorkspaceGcReport {
            dry_run,
            ..Default::default()
        };
        let Some(configured) = &self.config.delegated_workspace_root else {
            return Ok(report);
        };
        match fs::symlink_metadata(configured) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(error) => return Err(invalid(format!("inspect workspace GC root: {error}"))),
            Ok(metadata) if !metadata.is_dir() => {
                return Err(invalid("workspace GC root must be a real directory"));
            }
            Ok(_) => {}
        }
        let root = fs::canonicalize(configured)
            .map_err(|error| invalid(format!("resolve workspace GC root: {error}")))?;
        let mut cursor = self.workspace_gc_cursor.lock();
        if cursor.root.as_ref() != Some(&root)
            || (cursor.directory.is_none() && cursor.pending.is_empty())
        {
            cursor.root = Some(root.clone());
            cursor.directory = Some(
                fs::read_dir(&root)
                    .map_err(|error| invalid(format!("read workspace GC root: {error}")))?,
            );
            cursor.pending.clear();
        }
        for _ in 0..MAX_DIRECTORY_ENTRIES {
            if cursor.pending.len() > MAX_CANDIDATES {
                break;
            }
            let Some(entries) = cursor.directory.as_mut() else {
                break;
            };
            let Some(entry) = entries.next() else {
                cursor.directory = None;
                break;
            };
            let entry = entry.map_err(|error| invalid(format!("read GC candidate: {error}")))?;
            if entry.file_name() == ".integration.lock" {
                continue;
            }
            cursor.pending.push_back(entry.path());
        }
        report.truncated = cursor.pending.len() > MAX_CANDIDATES || cursor.directory.is_some();
        let selected: Vec<_> = cursor
            .pending
            .iter()
            .take(MAX_CANDIDATES)
            .cloned()
            .collect();
        // Preview only snapshots discovery; its Git inspection must not delay a real sweep.
        let mut cursor = if dry_run {
            drop(cursor);
            None
        } else {
            Some(cursor)
        };
        for directory in selected {
            if let Some(cursor) = cursor.as_mut() {
                cursor.pending.pop_front();
            }
            let mut candidate = workspace::gc::Candidate::new(directory);
            let outcome = self.gc_workspace_candidate(&root, &mut candidate, grace, dry_run, now);
            // Restoration failures are fatal and visible, including after a
            // refusal. Drop only provides a second cleanup attempt.
            candidate.restore_permissions()?;
            let (action, reason) = match outcome {
                Ok(action) => (
                    action,
                    "clean workspace with saved commits; grace elapsed".into(),
                ),
                Err(RuntimeError::Store(error)) => return Err(error.into()),
                Err(error @ RuntimeError::Internal(_)) => return Err(error),
                Err(error) => (WorkspaceGcAction::Kept, error.to_string()),
            };
            match action {
                WorkspaceGcAction::Kept | WorkspaceGcAction::WouldRemove => {
                    report.bytes_kept = report.bytes_kept.saturating_add(candidate.bytes);
                }
                WorkspaceGcAction::Removed => {
                    report.bytes_removed = report.bytes_removed.saturating_add(candidate.bytes);
                }
            }
            if action != WorkspaceGcAction::Kept {
                report.bytes_eligible = report.bytes_eligible.saturating_add(candidate.bytes);
            }
            report.workspaces.push(WorkspaceGcEntry {
                workspace: candidate.directory.join("workspace"),
                bytes: candidate.bytes,
                bytes_complete: candidate.bytes_complete,
                action,
                reason,
            });
        }
        Ok(report)
    }

    fn gc_workspace_candidate(
        &self,
        root: &Path,
        candidate: &mut workspace::gc::Candidate,
        grace: Duration,
        dry_run: bool,
        now: SystemTime,
    ) -> Result<WorkspaceGcAction> {
        let name = candidate
            .directory
            .file_name()
            .and_then(|name| name.to_str());
        if name.is_none_or(|name| name.parse::<ulid::Ulid>().is_err()) {
            return Err(invalid("unrecognized workspace directory name"));
        }
        if !fs::symlink_metadata(&candidate.directory)
            .map_err(|error| invalid(format!("inspect workspace GC directory: {error}")))?
            .is_dir()
        {
            return Err(invalid("workspace GC candidate must be a real directory"));
        }
        let workspace_path = candidate.directory.join("workspace");
        let _integration = if dry_run {
            None
        } else {
            workspace::gc::integration_lock(root, false)?
        };
        let _reservation = if dry_run {
            None
        } else {
            let _hold = self.hold();
            let agents = self.store.agents(&AgentFilter::default())?;
            self.gc_agent_blocker(&agents, &workspace_path, None, &self.sched.lock())?;
            *self.workspace_gc_active.lock() = Some(workspace_path.clone());
            // Keep creation out until permission restoration, including on errors.
            Some(Reservation(self))
        };
        let result = (|| {
            candidate.measure(dry_run)?;
            let manifest = candidate.inspect(root)?;
            candidate.recheck(&manifest)?;
            let _hold = (!dry_run).then(|| self.hold());
            let agents = self.store.agents(&AgentFilter::default())?;
            let sched = self.sched.lock();
            self.gc_agent_blocker(&agents, &workspace_path, Some(manifest.parent), &sched)?;
            let _sched = if dry_run {
                drop(sched);
                None
            } else {
                Some(sched)
            };
            let owners = manifest_owners(&agents, Some(manifest.parent));
            let latest = agents
                .iter()
                .filter(|agent| {
                    owners.contains(&agent.id)
                        || scope_owns_workspace(&agent.scope, &workspace_path)
                })
                .map(|agent| SystemTime::from(agent.updated_at))
                .fold(candidate.modified, SystemTime::max);
            if !grace_elapsed(now, latest, grace) {
                return Err(invalid("workspace is within the grace period"));
            }
            candidate.recheck_unchanged(&manifest)?;
            if dry_run {
                return Ok(WorkspaceGcAction::WouldRemove);
            }
            candidate.remove(&manifest).map_err(|error| {
                RuntimeError::Internal(format!(
                    "workspace GC removal failed and may be partial: {error}"
                ))
            })?;
            Ok(WorkspaceGcAction::Removed)
        })();
        if let Err(error) = candidate.restore_permissions() {
            return Err(RuntimeError::Internal(format!(
                "workspace GC permission restoration failed: {error}; candidate result: {result:?}"
            )));
        }
        result
    }

    fn gc_agent_blocker(
        &self,
        agents: &[Agent],
        workspace: &Path,
        parent: Option<aether_core::id::AgentId>,
        sched: &SchedulerState,
    ) -> Result<()> {
        let engine_turns: std::collections::HashSet<_> =
            self.engine_turns.lock().keys().copied().collect();
        let resources = self.resources.holdings();
        let owners = manifest_owners(agents, parent);
        let leases: HashSet<_> = self
            .store
            .leases()?
            .iter()
            .map(|lease| lease.agent_id)
            .collect();
        // A path lease is explicit use, even when its holder has a broad scope
        // or its agent row has already disappeared.
        if resources
            .iter()
            .flat_map(|holding| &holding.keys)
            .any(|key| {
                key.0
                    .strip_prefix("file:")
                    .or_else(|| key.0.strip_prefix("dir:"))
                    .or_else(|| key.0.strip_prefix("repo:"))
                    .is_some_and(|path| paths_overlap(Path::new(path), workspace))
            })
        {
            return Err(invalid("workspace has an explicit resource lease"));
        }
        if parent.is_some_and(|id| sched.running.contains_key(&id) || engine_turns.contains(&id)) {
            return Err(invalid("workspace parent has an in-flight turn"));
        }
        for agent in agents {
            let using = owners.contains(&agent.id) || scope_owns_workspace(&agent.scope, workspace);
            let in_flight =
                sched.running.contains_key(&agent.id) || engine_turns.contains(&agent.id);
            let held_resources = resources.iter().any(|holding| holding.agent == agent.id);
            if using
                && (in_flight
                    || held_resources
                    || leases.contains(&agent.id)
                    || !final_status(agent.status.kind()))
            {
                return Err(invalid(format!(
                    "workspace has live or resumable agent {}",
                    agent.id
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn workspace_gc_reserves(&self, scope: &Scope) -> bool {
        self.workspace_gc_active
            .lock()
            .as_ref()
            .is_some_and(|workspace| {
                scope
                    .workspace_root
                    .iter()
                    .chain(&scope.allowed_paths)
                    .any(|path| paths_overlap(path, workspace))
            })
    }

    /// Called under the creation hold, after preparatory filesystem checks.
    pub(crate) fn recheck_workspace_gc_admission(&self, scope: &Scope) -> Result<()> {
        if self.workspace_gc_reserves(scope) {
            return Err(invalid(
                "managed workspace is being inspected for collection; retry",
            ));
        }
        let (Some(managed), Some(path)) =
            (&self.config.delegated_workspace_root, &scope.workspace_root)
        else {
            return Ok(());
        };
        let managed = normalized_path(managed);
        let path = normalized_path(path);
        if path.starts_with(&managed) && !path.is_dir() {
            return Err(invalid(
                "managed workspace disappeared before agent admission; retry",
            ));
        }
        Ok(())
    }
}

fn final_status(status: AgentStatusKind) -> bool {
    matches!(
        status,
        AgentStatusKind::Completed | AgentStatusKind::Imported
    )
}

fn scope_owns_workspace(scope: &Scope, workspace: &Path) -> bool {
    scope
        .workspace_root
        .as_ref()
        .is_some_and(|path| normalized_path(path).starts_with(workspace))
}

fn paths_overlap(path: &Path, workspace: &Path) -> bool {
    let path = normalized_path(path);
    path.starts_with(workspace) || workspace.starts_with(path)
}

/// The manifest's delegator and its direct ancestry/descendancy own the
/// workspace even when their current scopes no longer name it. Missing
/// intermediate rows must not sever a recorded parent-child edge.
fn manifest_owners(agents: &[Agent], parent: Option<AgentId>) -> HashSet<AgentId> {
    let Some(parent) = parent else {
        return HashSet::new();
    };
    let by_id: HashMap<_, _> = agents.iter().map(|agent| (agent.id, agent)).collect();
    let mut children: HashMap<_, Vec<_>> = HashMap::new();
    for agent in agents {
        if let Some(parent) = agent.parent {
            children.entry(parent).or_default().push(agent.id);
        }
    }
    let mut owners = HashSet::from([parent]);
    let mut pending = VecDeque::from([parent]);
    while let Some(id) = pending.pop_front() {
        for child in children.get(&id).into_iter().flatten() {
            if owners.insert(*child) {
                pending.push_back(*child);
            }
        }
    }
    let mut ancestor = by_id.get(&parent).and_then(|agent| agent.parent);
    while let Some(id) = ancestor {
        if !owners.insert(id) {
            break;
        }
        ancestor = by_id.get(&id).and_then(|agent| agent.parent);
    }
    owners
}

fn normalized_path(path: &Path) -> PathBuf {
    resolve_path(path, 0)
}

fn resolve_path(path: &Path, links: usize) -> PathBuf {
    // Resolve the existing prefix as well: scopes may name a missing child
    // directory or an alias left dangling by collection before admission.
    if links < 32 {
        for ancestor in path.ancestors() {
            let Ok(suffix) = path.strip_prefix(ancestor) else {
                continue;
            };
            if let Ok(resolved) = fs::canonicalize(ancestor) {
                return lexical_path(&resolved.join(suffix));
            }
            if let Ok(target) = fs::read_link(ancestor) {
                let target = if target.is_absolute() {
                    target
                } else {
                    ancestor.parent().unwrap_or(Path::new("")).join(target)
                };
                return resolve_path(&target.join(suffix), links + 1);
            }
        }
    }
    lexical_path(path)
}

fn lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

fn grace_elapsed(now: SystemTime, modified: SystemTime, grace: Duration) -> bool {
    now.duration_since(modified).is_ok_and(|age| age >= grace)
}

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

#[cfg(test)]
mod tests;
