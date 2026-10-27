//! Conservative, bounded collection of private delegated Git worktrees.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use aether_core::agent::{Agent, Scope};
use aether_core::id::AgentId;
use aether_store::AgentFilter;
use serde::{Deserialize, Serialize};

use crate::workspace::gc::BuildOutputAction;
use crate::{CommitHold, Result, Runtime, RuntimeError, SchedulerState, workspace};

const MAX_CANDIDATES: usize = 16;
const MAX_DIRECTORY_ENTRIES: usize = 128;

#[derive(Default)]
pub(crate) struct WorkspaceGcCursor {
    root: Option<PathBuf>,
    directory: Option<fs::ReadDir>,
    pending: std::collections::VecDeque<PathBuf>,
}

struct Reservation<'a>(&'a Runtime);

#[derive(Debug, Clone, Copy)]
struct Sweep {
    grace: Duration,
    dry_run: bool,
    /// Also prune build output of workspaces kept for another reason.
    prune_kept: bool,
    now: SystemTime,
}

/// Held from the owner check through removal in a real sweep.
struct Gate<'a> {
    _hold: Option<CommitHold<'a>>,
    _sched: Option<parking_lot::MutexGuard<'a, SchedulerState>>,
}

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
    /// Wholly ignored build output; its bytes are part of `bytes`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_output: Vec<WorkspaceBuildOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceBuildOutput {
    pub path: PathBuf,
    /// Allocated disk bytes, as `du` counts them.
    pub bytes: u64,
    pub action: BuildOutputAction,
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

impl WorkspaceGcReport {
    /// Pruned build output counts as removed even when its workspace stays.
    fn add(
        &mut self,
        candidate: &workspace::gc::Candidate,
        action: WorkspaceGcAction,
        reason: String,
    ) {
        let bytes_with = |wanted: BuildOutputAction| -> u64 {
            candidate
                .build_output
                .iter()
                .filter(|output| output.action == wanted)
                .map(|output| output.bytes)
                .sum()
        };
        let pruned = bytes_with(BuildOutputAction::Pruned);
        let would_prune = bytes_with(BuildOutputAction::WouldPrune);
        debug_assert!(
            pruned + would_prune <= candidate.bytes,
            "build output is in bytes"
        );
        debug_assert!(pruned == 0 || !self.dry_run, "a preview never prunes");
        match action {
            WorkspaceGcAction::Kept => {
                self.bytes_kept = self.bytes_kept.saturating_add(candidate.bytes - pruned);
                self.bytes_removed = self.bytes_removed.saturating_add(pruned);
                self.bytes_eligible = self.bytes_eligible.saturating_add(pruned + would_prune);
            }
            WorkspaceGcAction::WouldRemove => {
                self.bytes_kept = self.bytes_kept.saturating_add(candidate.bytes);
                self.bytes_eligible = self.bytes_eligible.saturating_add(candidate.bytes);
            }
            WorkspaceGcAction::Removed => {
                self.bytes_removed = self.bytes_removed.saturating_add(candidate.bytes);
                self.bytes_eligible = self.bytes_eligible.saturating_add(candidate.bytes);
            }
        }
        self.workspaces.push(WorkspaceGcEntry {
            workspace: candidate.directory.join("workspace"),
            bytes: candidate.bytes,
            bytes_complete: candidate.bytes_complete,
            action,
            reason,
            build_output: candidate
                .build_output
                .iter()
                .map(|output| WorkspaceBuildOutput {
                    path: output.path.clone(),
                    bytes: output.bytes,
                    action: output.action,
                })
                .collect(),
        });
    }
}

/// Drop snapshot refs left behind by failed creations in the candidate's
/// source repository. A failure is logged and never blocks the sweep.
fn sweep_candidate_source(root: &Path, candidate: &workspace::gc::Candidate) {
    let result = candidate.source().and_then(|source| {
        if workspace::source_missing(&source)? {
            return Ok(vec![]);
        }
        workspace::sweep_orphan_refs(root, &source)
    });
    match result {
        Ok(removed) => {
            for reference in removed {
                tracing::info!(reference, "removed orphan workspace snapshot reference");
            }
        }
        Err(error) => tracing::warn!(%error, "orphan workspace reference sweep skipped"),
    }
}

impl Runtime {
    /// Remove old, clean delegated worktrees whose saved state is retained
    /// elsewhere. Each candidate is independent; refusals appear in the report.
    /// Declared build output is pruned only from workspaces then removed.
    pub fn gc_workspaces(&self, grace: Duration, dry_run: bool) -> Result<WorkspaceGcReport> {
        let sweep = Sweep {
            grace,
            dry_run,
            prune_kept: false,
            now: SystemTime::now(),
        };
        self.gc_workspaces_at(sweep)
    }

    /// Like `gc_workspaces`, and also prune the declared build output of
    /// finished workspaces that are kept for another reason.
    pub fn gc_workspaces_pruning_build_output(
        &self,
        grace: Duration,
        dry_run: bool,
    ) -> Result<WorkspaceGcReport> {
        let sweep = Sweep {
            grace,
            dry_run,
            prune_kept: true,
            now: SystemTime::now(),
        };
        self.gc_workspaces_at(sweep)
    }

    fn gc_workspaces_at(&self, sweep: Sweep) -> Result<WorkspaceGcReport> {
        let dry_run = sweep.dry_run;
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
        if cursor.root.as_ref() == Some(&root) {
            cursor.pending.retain(|directory| {
                !matches!(fs::symlink_metadata(directory), Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound)
            });
        }
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
            if matches!(fs::symlink_metadata(&directory), Err(error)
                if error.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            let mut candidate = workspace::gc::Candidate::new(directory);
            if !dry_run {
                sweep_candidate_source(&root, &candidate);
            }
            let outcome = self.gc_workspace_candidate(&root, &mut candidate, sweep);
            // Restoration failures are fatal and visible, including after a
            // refusal. Drop only provides a second cleanup attempt.
            candidate.restore_permissions()?;
            let (action, reason) = match outcome {
                Ok(action) => (action, eligible_reason(&candidate)),
                Err(RuntimeError::Store(error)) => return Err(error.into()),
                Err(error @ RuntimeError::Internal(_)) => return Err(error),
                Err(error) => (WorkspaceGcAction::Kept, error.to_string()),
            };
            report.add(&candidate, action, reason);
        }
        Ok(report)
    }

    fn gc_workspace_candidate(
        &self,
        root: &Path,
        candidate: &mut workspace::gc::Candidate,
        sweep: Sweep,
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
        let _integration = if sweep.dry_run {
            None
        } else {
            workspace::gc::integration_lock(root, false)?
        };
        let _reservation = if sweep.dry_run {
            None
        } else {
            let _hold = self.hold();
            let agents = self.store.agents(&AgentFilter::default())?;
            self.gc_agent_blocker(&agents, &workspace_path, None, false, &self.sched.lock())?;
            *self.workspace_gc_active.lock() = Some(workspace_path.clone());
            // Keep creation out until permission restoration, including on errors.
            Some(Reservation(self))
        };
        let result = self.collect_candidate(root, candidate, &workspace_path, sweep);
        if let Err(error) = candidate.restore_permissions() {
            return Err(RuntimeError::Internal(format!(
                "workspace GC permission restoration failed: {error}; candidate result: {result:?}"
            )));
        }
        result
    }

    fn collect_candidate(
        &self,
        root: &Path,
        candidate: &mut workspace::gc::Candidate,
        workspace_path: &Path,
        sweep: Sweep,
    ) -> Result<WorkspaceGcAction> {
        let names = self.build_output_names(candidate)?;
        candidate.measure(sweep.dry_run, &names)?;
        // Only a preservation refusal falls back to pruning; a failed recheck
        // means the workspace changed and nothing is deleted.
        let manifest = match candidate.inspect(root) {
            Ok(manifest) => manifest,
            Err(RuntimeError::Invalid(reason))
                if sweep.prune_kept && !candidate.build_output.is_empty() =>
            {
                return Err(self.prune_kept_build_output(
                    root,
                    candidate,
                    workspace_path,
                    sweep,
                    reason,
                ));
            }
            Err(error) => return Err(error),
        };
        candidate.recheck(&manifest)?;
        let gate = self.gc_gate(&manifest, workspace_path, sweep, candidate)?;
        candidate.recheck_unchanged(&manifest)?;
        if sweep.dry_run {
            candidate.mark_build_output(BuildOutputAction::WouldPrune);
            return Ok(WorkspaceGcAction::WouldRemove);
        }
        if candidate.build_output.is_empty() {
            remove_candidate(candidate, &manifest)?;
            drop(gate);
            return Ok(WorkspaceGcAction::Removed);
        }
        // Deleting build output can take long: do it outside the commit hold.
        // The reservation still keeps new agents out of this workspace.
        drop(gate);
        candidate.prune_build_output()?;
        self.remove_pruned(root, candidate, workspace_path, sweep)
    }

    fn build_output_names(&self, candidate: &workspace::gc::Candidate) -> Result<Vec<String>> {
        let storage = &self.models.config().storage;
        if storage.repository_build_output_dirs.is_empty() {
            return Ok(storage.build_output_dirs.clone());
        }
        Ok(storage.build_output_dirs_for(&candidate.source()?).to_vec())
    }

    /// Run every check again, now with nothing excluded, then remove. Grace
    /// uses the first pass's age: pruning itself touched the parent directories.
    fn remove_pruned(
        &self,
        root: &Path,
        pruned: &mut workspace::gc::Candidate,
        workspace_path: &Path,
        sweep: Sweep,
    ) -> Result<WorkspaceGcAction> {
        debug_assert!(!sweep.dry_run, "a preview never prunes");
        pruned.restore_permissions().map_err(|error| {
            RuntimeError::Internal(format!(
                "workspace GC permission restoration failed: {error}"
            ))
        })?;
        let mut strict = workspace::gc::Candidate::new(pruned.directory.clone());
        let result = (|| {
            strict.measure(false, &[])?;
            let manifest = strict.inspect(root)?;
            strict.recheck(&manifest)?;
            strict.modified = pruned.modified;
            let _gate = self.gc_gate(&manifest, workspace_path, sweep, &strict)?;
            strict.recheck_unchanged(&manifest)?;
            remove_candidate(&mut strict, &manifest)?;
            Ok(WorkspaceGcAction::Removed)
        })();
        pruned.source_gone = strict.source_gone;
        pruned.prune_error = strict.prune_error.take();
        if let Err(error) = strict.restore_permissions() {
            return Err(RuntimeError::Internal(format!(
                "workspace GC permission restoration failed: {error}; candidate result: {result:?}"
            )));
        }
        result
    }

    /// The workspace stays, but its owners are finished and past grace: its
    /// build output can go. Returns the reason the workspace is kept.
    fn prune_kept_build_output(
        &self,
        root: &Path,
        candidate: &mut workspace::gc::Candidate,
        workspace_path: &Path,
        sweep: Sweep,
        reason: String,
    ) -> RuntimeError {
        let outcome = (|| {
            let manifest = candidate.manifest(root)?;
            drop(self.gc_gate(&manifest, workspace_path, sweep, candidate)?);
            candidate.confirm_build_output()?;
            if sweep.dry_run {
                candidate.mark_build_output(BuildOutputAction::WouldPrune);
                return Ok(());
            }
            candidate.prune_build_output()
        })();
        match outcome {
            Ok(()) => invalid(reason),
            Err(RuntimeError::Invalid(why)) => {
                invalid(format!("{reason}; build output kept: {why}"))
            }
            Err(error) => error,
        }
    }

    /// Owners must be finished and past grace. A real sweep returns holding
    /// the commit hold and scheduler lock, so no turn starts before removal.
    fn gc_gate(
        &self,
        manifest: &workspace::WorkspaceManifest,
        workspace_path: &Path,
        sweep: Sweep,
        candidate: &workspace::gc::Candidate,
    ) -> Result<Gate<'_>> {
        let hold = (!sweep.dry_run).then(|| self.hold());
        let agents = self.store.agents(&AgentFilter::default())?;
        let sched = self.sched.lock();
        self.gc_agent_blocker(
            &agents,
            workspace_path,
            Some(manifest.parent),
            candidate.source_gone,
            &sched,
        )?;
        let sched = if sweep.dry_run {
            drop(sched);
            None
        } else {
            Some(sched)
        };
        let owners = manifest_owners(&agents, Some(manifest.parent));
        let latest = agents
            .iter()
            .filter(|agent| {
                owners.contains(&agent.id) || scope_owns_workspace(&agent.scope, workspace_path)
            })
            .map(|agent| SystemTime::from(agent.updated_at))
            .fold(candidate.modified, SystemTime::max);
        if !grace_elapsed(sweep.now, latest, sweep.grace) {
            return Err(invalid("workspace is within the grace period"));
        }
        Ok(Gate {
            _hold: hold,
            _sched: sched,
        })
    }

    /// Every owner must be stopped and terminal. The manifest parent may stay
    /// live only once its source checkout is gone: it has nowhere to integrate.
    fn gc_agent_blocker(
        &self,
        agents: &[Agent],
        workspace: &Path,
        parent: Option<aether_core::id::AgentId>,
        source_gone: bool,
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
        self.require_gc_ownership()?;
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
            let settled = agent.status.is_terminal() || (source_gone && parent == Some(agent.id));
            if using && (in_flight || held_resources || leases.contains(&agent.id) || !settled) {
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
        if self.managed_workspace_missing(scope) {
            return Err(invalid(
                "managed workspace disappeared before agent admission; retry",
            ));
        }
        Ok(())
    }

    /// A stopped agent cannot wake or restart into a managed workspace that
    /// collection removed. Called under the commit hold.
    pub(crate) fn refuse_collected_workspace(&self, agent: &Agent) -> Result<()> {
        if self.workspace_gc_reserves(&agent.scope) {
            return Err(invalid(
                "managed workspace is being inspected for collection; retry",
            ));
        }
        if self.managed_workspace_missing(&agent.scope) {
            return Err(invalid(format!(
                "agent {}'s managed workspace was collected; delegate a new child instead",
                agent.id
            )));
        }
        Ok(())
    }

    fn managed_workspace_missing(&self, scope: &Scope) -> bool {
        let (Some(managed), Some(path)) =
            (&self.config.delegated_workspace_root, &scope.workspace_root)
        else {
            return false;
        };
        let managed = normalized_path(managed);
        let path = normalized_path(path);
        path.starts_with(&managed) && !path.is_dir()
    }
}

fn remove_candidate(
    candidate: &mut workspace::gc::Candidate,
    manifest: &workspace::WorkspaceManifest,
) -> Result<()> {
    candidate.remove(manifest).map_err(|error| {
        RuntimeError::Internal(format!(
            "workspace GC removal failed and may be partial: {error}"
        ))
    })
}

fn eligible_reason(candidate: &workspace::gc::Candidate) -> String {
    let mut reason = String::from("clean workspace with saved commits; grace elapsed");
    if candidate.source_gone {
        reason.push_str("; source checkout is gone");
    }
    if let Some(error) = &candidate.prune_error {
        reason.push_str(&format!("; git worktree prune failed: {error}"));
    }
    reason
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

/// The manifest's delegator and its descendants own the workspace even when
/// their current scopes no longer name it. Ancestors do not: only the direct
/// parent integrates, and a long-lived ancestor must not pin the workspace.
fn manifest_owners(agents: &[Agent], parent: Option<AgentId>) -> HashSet<AgentId> {
    let Some(parent) = parent else {
        return HashSet::new();
    };
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
