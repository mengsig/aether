//! Operator review of an agent's workspace: its diff, integrating a child's
//! managed worktree into the source it came from, and discarding it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::workspace_review::{
    COMMIT_LIMIT, CappedPatch, WorkspaceCommit, WorkspaceDiff, WorkspaceDiscard,
    WorkspaceIntegration,
};
use aether_store::{AgentFilter, Commit};
use serde_json::json;

use crate::workspace::review::{self, TreeDiff, path_text};
use crate::workspace::{self, IntegrationOutcome, WorkspaceManifest};
use crate::{Result, Runtime, RuntimeError};

mod heads;
use heads::TurnWindow;
pub(crate) use heads::{Boundary, record_turn_head};

/// A child's managed worktree, checked against its private record.
pub(crate) struct Managed {
    pub managed_root: PathBuf,
    pub manifest: WorkspaceManifest,
}

/// Why `target`'s worktree cannot be integrated now, or the worktree. The
/// agent tool and the operator command both pass through here.
pub(crate) fn integration_preflight(
    runtime: &Runtime,
    target: &Agent,
) -> Result<std::result::Result<Managed, String>> {
    if !target.status.is_terminal() {
        return Ok(Err(format!(
            "agent {} is still {}; wait for it to finish before integrating",
            target.id,
            target.status.kind()
        )));
    }
    let managed = match managed_workspace(runtime, target)? {
        Ok(managed) => managed,
        Err(refusal) => return Ok(Err(format!("integration refused: {refusal}"))),
    };
    if let Some(live) = live_user(runtime, target, &managed.manifest.workspace)? {
        return Ok(Err(format!("integration refused: {live}")));
    }
    Ok(Ok(managed))
}

fn managed_workspace(
    runtime: &Runtime,
    target: &Agent,
) -> Result<std::result::Result<Managed, String>> {
    let Some(managed_root) = runtime.config.delegated_workspace_root.as_ref() else {
        return Ok(Err("managed workspaces are not configured".into()));
    };
    let Some(root) = target.scope.workspace_root.as_ref() else {
        return Ok(Err(format!(
            "agent {} has no isolated workspace",
            target.id
        )));
    };
    if discarded(managed_root, root) {
        return Ok(Err(discarded_message(target.id)));
    }
    let manifest = match workspace::load(managed_root, root) {
        Ok(manifest) => manifest,
        Err(error) => return Ok(Err(error.to_string())),
    };
    if Some(manifest.parent) != target.parent {
        return Ok(Err(format!(
            "agent {} shares a workspace delegated to another agent",
            target.id
        )));
    }
    Ok(Ok(Managed {
        managed_root: managed_root.clone(),
        manifest,
    }))
}

fn discarded_message(agent: AgentId) -> String {
    format!("workspace for agent {agent} was discarded")
}

/// True when `root` is gone from the managed area: its owner discarded it.
fn discarded(managed_root: &Path, root: &Path) -> bool {
    let managed = std::fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.into());
    root.starts_with(&managed)
        && matches!(
            std::fs::symlink_metadata(root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
}

/// The first live agent that could still change `workspace`, described: the
/// target, a descendant, or any agent working inside it, such as a restart.
fn live_user(runtime: &Runtime, target: &Agent, workspace: &Path) -> Result<Option<String>> {
    debug_assert!(workspace.is_absolute(), "managed worktrees are absolute");
    let agents = runtime.store.agents(&AgentFilter::default())?;
    let descendants: HashSet<_> = aether_core::agent::subtree_of(&agents, target.id)
        .into_iter()
        .collect();
    debug_assert!(
        descendants.contains(&target.id),
        "the target heads its subtree"
    );
    for agent in agents.iter().filter(|agent| !agent.status.is_terminal()) {
        let status = agent.status.kind();
        let inside = agent
            .scope
            .workspace_root
            .as_deref()
            .is_some_and(|root| root.starts_with(workspace));
        let role = if agent.id == target.id {
            "agent"
        } else if descendants.contains(&agent.id) {
            "descendant"
        } else if inside {
            "agent working in this workspace"
        } else {
            continue;
        };
        return Ok(Some(format!("{role} {} is still {status}", agent.id)));
    }
    Ok(None)
}

fn invalid_with(prefix: &str, error: RuntimeError) -> RuntimeError {
    match error {
        RuntimeError::Invalid(message) => RuntimeError::Invalid(format!("{prefix}: {message}")),
        other => other,
    }
}

fn workspace_event(by: Actor, target: &Agent, name: &str, payload: serde_json::Value) -> Commit {
    Commit::new().event(
        EventDraft::new(
            by,
            EventKind::Custom {
                name: name.into(),
                payload,
            },
        )
        .agent(target.id)
        .parent(target.parent)
        .run(target.run),
    )
}

impl Runtime {
    fn review_target(&self, agent: AgentId) -> Result<Agent> {
        self.store
            .agent(agent)?
            .ok_or_else(|| RuntimeError::NotFound(format!("agent {agent}")))
    }

    /// Land a finished child's worktree in its source through the same
    /// checks as `integrate_workspace`. Conflicts refuse it unwritten.
    pub fn workspace_integrate(&self, agent: AgentId, by: Actor) -> Result<WorkspaceIntegration> {
        let target = self.review_target(agent)?;
        let managed = integration_preflight(self, &target)?.map_err(RuntimeError::Invalid)?;
        let outcome = workspace::integrate(&managed.managed_root, &managed.manifest)
            .map_err(|error| invalid_with("integration refused", error))?;
        let source = managed.manifest.source.clone();
        let report = match outcome {
            IntegrationOutcome::Conflicts(paths) => {
                debug_assert!(!paths.is_empty(), "a refusal names its conflicts");
                return Ok(WorkspaceIntegration {
                    agent,
                    source,
                    integrated: false,
                    already_integrated: false,
                    changed_paths: Vec::new(),
                    conflicts: paths.iter().map(|path| path_text(path)).collect(),
                });
            }
            IntegrationOutcome::Landed(report) => report,
        };
        let integration = WorkspaceIntegration {
            agent,
            source,
            integrated: true,
            already_integrated: report.already_integrated,
            changed_paths: report.changed_paths.iter().map(|p| path_text(p)).collect(),
            conflicts: Vec::new(),
        };
        self.apply(workspace_event(
            by,
            &target,
            "workspace.integrated",
            json!(integration),
        ))?;
        Ok(integration)
    }

    /// Throw away a child's managed worktree. A live agent using it refuses
    /// the discard unless `force`; the discard is recorded as an event.
    pub fn workspace_discard(
        &self,
        agent: AgentId,
        force: bool,
        by: Actor,
    ) -> Result<WorkspaceDiscard> {
        let target = self.review_target(agent)?;
        let managed = managed_workspace(self, &target)?
            .map_err(|refusal| RuntimeError::Invalid(format!("discard refused: {refusal}")))?;
        let live = live_user(self, &target, &managed.manifest.workspace)?;
        if let Some(live) = live.as_ref().filter(|_| !force) {
            return Err(RuntimeError::Invalid(format!(
                "discard refused: {live}; force it to discard the workspace anyway"
            )));
        }
        review::abandon(&managed.managed_root, &managed.manifest)
            .map_err(|error| invalid_with("discard refused", error))?;
        let discard = WorkspaceDiscard {
            agent,
            workspace: managed.manifest.workspace.clone(),
            forced: live.is_some(),
        };
        let payload = json!({
            "workspace": discard.workspace,
            "source": managed.manifest.source,
            "forced": discard.forced,
            "status": target.status.kind(),
        });
        self.apply(workspace_event(by, &target, "workspace.discarded", payload))?;
        Ok(discard)
    }

    /// What `agent` changed in its workspace: against the snapshot it
    /// started from when the worktree is its own, otherwise across the
    /// commits its turns made, marked ambiguous.
    pub fn workspace_diff(&self, agent: AgentId) -> Result<WorkspaceDiff> {
        let target = self.review_target(agent)?;
        let root = target
            .scope
            .workspace_root
            .clone()
            .ok_or_else(|| RuntimeError::Invalid(format!("agent {agent} has no workspace")))?;
        if let Some(managed_root) = self.config.delegated_workspace_root.as_ref()
            && discarded(managed_root, &root)
        {
            return Err(RuntimeError::Invalid(discarded_message(agent)));
        }
        let windows = heads::turn_windows(self, &target)?;
        match self.own_worktree(&target, &root)? {
            Some(managed) => isolated_diff(&target, &managed, &windows),
            None => shared_diff(&target, &root, &windows),
        }
    }

    /// The managed worktree `target` was delegated, when `root` is one.
    fn own_worktree(&self, target: &Agent, root: &Path) -> Result<Option<Managed>> {
        let Some(managed_root) = self.config.delegated_workspace_root.as_ref() else {
            return Ok(None);
        };
        let Ok(real_root) = std::fs::canonicalize(managed_root) else {
            return Ok(None);
        };
        if !root.starts_with(&real_root) {
            return Ok(None);
        }
        let manifest = workspace::load(managed_root, root)?;
        Ok((Some(manifest.parent) == target.parent).then(|| Managed {
            managed_root: managed_root.clone(),
            manifest,
        }))
    }
}

/// The turn `target` is running now, if any: the last one recorded.
fn running_turn(target: &Agent, windows: &[TurnWindow]) -> Option<u32> {
    let running = target.status.kind() == AgentStatusKind::Running;
    windows.last().filter(|_| running).map(|window| window.turn)
}

/// Where a turn's commits end; the current HEAD stands in for the end of the
/// running turn. `None` when the turn moved nothing or never closed.
fn window_end(window: &TurnWindow, running: Option<u32>, head: Option<&str>) -> Option<String> {
    let open = running == Some(window.turn);
    window
        .end
        .clone()
        .or_else(|| open.then(|| head.map(str::to_owned)).flatten())
        .filter(|end| *end != window.start)
}

fn label_turns(
    repo: &Path,
    target: &Agent,
    commits: &mut [WorkspaceCommit],
    windows: &[TurnWindow],
    head: Option<&str>,
) -> Result<()> {
    let running = running_turn(target, windows);
    let mut turn_of = HashMap::new();
    for window in windows {
        let Some(end) = window_end(window, running, head) else {
            continue;
        };
        for commit in review::log(repo, &window.start, &end, COMMIT_LIMIT + 1)? {
            turn_of.entry(commit.id).or_insert(window.turn);
        }
    }
    for commit in commits {
        commit.turn = turn_of.get(&commit.id).copied();
    }
    Ok(())
}

fn isolated_diff(
    target: &Agent,
    managed: &Managed,
    windows: &[TurnWindow],
) -> Result<WorkspaceDiff> {
    let manifest = &managed.manifest;
    let tree = review::current_tree(&managed.managed_root, manifest)?;
    let diff = review::tree_diff(&manifest.workspace, &manifest.baseline_tree, &tree)?;
    let head = review::head(&manifest.workspace)?;
    let mut commits = match head.as_deref() {
        Some(head) => {
            let start = review::snapshot_commit(manifest)?;
            review::log(&manifest.workspace, &start, head, COMMIT_LIMIT + 1)?
        }
        None => Vec::new(),
    };
    label_turns(
        &manifest.workspace,
        target,
        &mut commits,
        windows,
        head.as_deref(),
    )?;
    Ok(assemble(
        target,
        Placement {
            workspace: manifest.workspace.clone(),
            source: Some(manifest.source.clone()),
            isolated: true,
            integrated: manifest.integrated_tree.as_deref() == Some(tree.as_str()),
        },
        diff,
        commits,
    ))
}

fn shared_diff(target: &Agent, root: &Path, windows: &[TurnWindow]) -> Result<WorkspaceDiff> {
    let head = review::head(root)?;
    let running = running_turn(target, windows);
    let spans: Vec<_> = windows
        .iter()
        .filter_map(|window| window_end(window, running, head.as_deref()).map(|end| (window, end)))
        .collect();
    let mut commits: Vec<WorkspaceCommit> = Vec::new();
    let mut seen = HashSet::new();
    for (window, end) in spans.iter().rev() {
        for mut commit in review::log(root, &window.start, end, COMMIT_LIMIT + 1)? {
            if !seen.insert(commit.id.clone()) {
                continue;
            }
            commit.turn = Some(window.turn);
            commits.push(commit);
        }
        if commits.len() > COMMIT_LIMIT {
            break;
        }
    }
    let diff = match (spans.first(), spans.last()) {
        (Some((first, _)), Some((_, last))) => review::tree_diff(root, &first.start, last)?,
        _ => empty_diff(),
    };
    Ok(assemble(
        target,
        Placement {
            workspace: root.to_path_buf(),
            source: None,
            isolated: false,
            integrated: false,
        },
        diff,
        commits,
    ))
}

fn empty_diff() -> TreeDiff {
    TreeDiff {
        files: Vec::new(),
        files_truncated: false,
        insertions: 0,
        deletions: 0,
        patch: CappedPatch {
            text: String::new(),
            bytes: 0,
            truncated: false,
        },
    }
}

struct Placement {
    workspace: PathBuf,
    source: Option<PathBuf>,
    isolated: bool,
    integrated: bool,
}

fn assemble(
    target: &Agent,
    placement: Placement,
    diff: TreeDiff,
    mut commits: Vec<WorkspaceCommit>,
) -> WorkspaceDiff {
    let commits_truncated = commits.len() > COMMIT_LIMIT;
    commits.truncate(COMMIT_LIMIT);
    WorkspaceDiff {
        agent: target.id,
        workspace: placement.workspace,
        source: placement.source,
        isolated: placement.isolated,
        ambiguous: !placement.isolated,
        integrated: placement.integrated,
        files: diff.files,
        files_truncated: diff.files_truncated,
        insertions: diff.insertions,
        deletions: diff.deletions,
        commits,
        commits_truncated,
        patch: diff.patch.text,
        patch_bytes: diff.patch.bytes,
        patch_truncated: diff.patch.truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(turn: u32, start: &str, end: Option<&str>) -> TurnWindow {
        TurnWindow {
            turn,
            start: start.into(),
            end: end.map(str::to_owned),
        }
    }

    #[test]
    fn only_the_running_turn_borrows_the_current_head() {
        let closed = window(1, "a", Some("a"));
        assert_eq!(window_end(&closed, None, Some("later")), None);
        assert_eq!(window_end(&closed, Some(1), Some("later")), None);
        let crashed = window(2, "a", None);
        assert_eq!(window_end(&crashed, None, Some("later")), None);
        assert_eq!(window_end(&crashed, Some(2), Some("b")), Some("b".into()));
        let moved = window(3, "a", Some("c"));
        assert_eq!(window_end(&moved, Some(4), Some("later")), Some("c".into()));
    }
}
