//! Known workspaces, workspace validation and the git summary command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::id::AgentId;
use aether_core::time::Timestamp;
use aether_protocol::remote::{GitSummary, Workspace, WorkspaceValidation};
use aether_protocol::{ErrorReply, Reply};
use aether_store::AgentFilter;
use futures::StreamExt as _;
use rustix::fs::Access;

use super::{check_absolute, check_text, effective_limit, git, home_dir, on_blocking_pool};
use crate::Daemon;
use crate::connection::store_err;

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
/// Workspaces whose git state is read at the same time.
const GIT_PARALLELISM: usize = 4;
/// Time to read one workspace's git state when listing many.
const LISTED_GIT_TIMEOUT: Duration = Duration::from_secs(2);
/// Time to read the git state a client asked about directly.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a path cannot host an agent, and where it resolved to if it did.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Unusable {
    pub reason: String,
    pub resolved: Option<PathBuf>,
}

/// The canonical form of `path` when it is a directory the daemon user can
/// read and enter.
pub(super) fn resolve_directory(path: &Path) -> Result<PathBuf, Unusable> {
    let unresolved = |reason: &str| Unusable {
        reason: reason.to_owned(),
        resolved: None,
    };
    let real = std::fs::canonicalize(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => unresolved("no such directory"),
        std::io::ErrorKind::PermissionDenied => unresolved("permission denied"),
        _ => unresolved(&format!("cannot be resolved: {e}")),
    })?;
    let refuse = |reason: &str| Unusable {
        reason: reason.to_owned(),
        resolved: Some(real.clone()),
    };
    assert!(real.is_absolute(), "canonicalize returns an absolute path");
    let meta = std::fs::metadata(&real).map_err(|_| refuse("cannot be read"))?;
    if !meta.is_dir() {
        return Err(refuse("not a directory"));
    }
    rustix::fs::access(&real, Access::READ_OK | Access::EXEC_OK)
        .map_err(|_| refuse("the daemon user cannot read and enter it"))?;
    Ok(real)
}

pub(super) async fn git_summary(workspace: PathBuf) -> Result<Reply, ErrorReply> {
    let workspace = check_absolute("workspace", &workspace)?;
    let real = on_blocking_pool("workspace", move || resolve_directory(&workspace))
        .await?
        .map_err(|unusable| ErrorReply::invalid(format!("`workspace` {}", unusable.reason)))?;
    Ok(Reply::GitSummary(git::summarize(&real, GIT_TIMEOUT).await?))
}

/// A git summary for a listing, where one unreadable repository must not
/// fail the rest: the error is logged and the summary left out.
async fn git_for_listing(path: &Path, timeout: Duration) -> Option<GitSummary> {
    match git::summarize(path, timeout).await {
        Ok(summary) => Some(summary),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "no git summary for workspace");
            None
        }
    }
}

pub(super) async fn validate_workspace(path: String) -> Result<Reply, ErrorReply> {
    check_text("path", &path)?;
    if path.is_empty() {
        return Err(ErrorReply::invalid("`path` is empty"));
    }
    let invalid = |reason: &str| {
        Ok(Reply::WorkspaceValidation(WorkspaceValidation {
            valid: false,
            path: None,
            reason: Some(reason.to_owned()),
            git: None,
        }))
    };
    let Some(expanded) = expand_home(&path, home_dir().as_deref()) else {
        return invalid("the path must be absolute or start with `~/`");
    };
    let resolved = on_blocking_pool("workspace", move || resolve_directory(&expanded)).await?;
    let real = match resolved {
        Ok(real) => real,
        Err(unusable) => {
            return Ok(Reply::WorkspaceValidation(WorkspaceValidation {
                valid: false,
                path: unusable.resolved,
                reason: Some(unusable.reason),
                git: None,
            }));
        }
    };
    let git = git_for_listing(&real, GIT_TIMEOUT).await;
    Ok(Reply::WorkspaceValidation(WorkspaceValidation {
        valid: true,
        path: Some(real),
        reason: None,
        git,
    }))
}

/// An absolute path for `text`: `~` and `~/…` are the daemon user's home;
/// `None` for anything else that is not already absolute (`~user` included).
fn expand_home(text: &str, home: Option<&Path>) -> Option<PathBuf> {
    if text == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    text.starts_with('/').then(|| PathBuf::from(text))
}

/// A directory some root agent worked in, with what is running there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownWorkspace {
    path: PathBuf,
    agents: u32,
    running: u32,
    active_at: Timestamp,
    exists: bool,
}

pub(super) async fn list_workspaces(
    daemon: &Daemon,
    limit: Option<u32>,
) -> Result<Reply, ErrorReply> {
    let limit = effective_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)?;
    let runtime = daemon.runtime.clone();
    let mut known = on_blocking_pool("workspace", move || {
        let agents = runtime.store().agents(&AgentFilter::default())?;
        let own_root = runtime.config().workspace_root.clone();
        Ok(known_workspaces(
            &agents,
            own_root.as_deref(),
            runtime.started_at(),
            &canonical_form,
        ))
    })
    .await?
    .map_err(store_err)?;
    known.truncate(limit);
    let listed: Vec<Workspace> = futures::stream::iter(known)
        .map(|known| async move {
            let git = match known.exists {
                true => git_for_listing(&known.path, LISTED_GIT_TIMEOUT).await,
                false => None,
            };
            Workspace {
                path: known.path,
                agents: known.agents,
                running: known.running,
                active_at: known.active_at,
                exists: known.exists,
                git,
            }
        })
        .buffered(GIT_PARALLELISM)
        .collect()
        .await;
    assert!(listed.len() <= limit, "a listing never exceeds its limit");
    Ok(Reply::Workspaces(listed))
}

/// The canonical spelling of `path` and whether it is a directory now; a path
/// that cannot be resolved stays as written.
fn canonical_form(path: &Path) -> (PathBuf, bool) {
    match std::fs::canonicalize(path) {
        Ok(real) => {
            let is_dir = real.is_dir();
            (real, is_dir)
        }
        Err(_) => (path.to_path_buf(), false),
    }
}

/// Every workspace of a root agent (live or finished, archived or not) and the daemon's own root,
/// most recently active first; a root with no agent in it is as recent as the daemon's start.
fn known_workspaces(
    agents: &[Agent],
    own_root: Option<&Path>,
    started_at: Timestamp,
    canonical: &dyn Fn(&Path) -> (PathBuf, bool),
) -> Vec<KnownWorkspace> {
    let by_id: HashMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();
    let mut registry = Registry {
        canonical,
        resolved: HashMap::new(),
        found: HashMap::new(),
    };
    for root in agents.iter().filter(|a| a.parent.is_none()) {
        let Some(path) = usable_workspace(root) else {
            continue;
        };
        let known = registry.entry(path, root.updated_at);
        known.active_at = known.active_at.max(root.updated_at);
    }
    for agent in agents.iter().filter(|a| a.is_live() && !a.archived) {
        let Some(path) = tree_root(agent, &by_id).and_then(usable_workspace) else {
            continue;
        };
        let known = registry.entry(path, agent.updated_at);
        known.agents += 1;
        if agent.status.kind() == AgentStatusKind::Running {
            known.running += 1;
        }
    }
    if let Some(own) = own_root.filter(|own| own.to_str().is_some()) {
        registry.entry(own, started_at);
    }
    let found = registry.found;
    let mut known: Vec<KnownWorkspace> = found.into_values().collect();
    known.sort_by(|a, b| b.active_at.cmp(&a.active_at).then(a.path.cmp(&b.path)));
    assert!(
        known
            .iter()
            .all(|k| k.path.is_absolute() && k.running <= k.agents),
        "a workspace is an absolute path and runs no more agents than it has"
    );
    known
}

/// Workspaces keyed by canonical path, each path resolved once.
struct Registry<'a> {
    canonical: &'a dyn Fn(&Path) -> (PathBuf, bool),
    resolved: HashMap<PathBuf, (PathBuf, bool)>,
    found: HashMap<PathBuf, KnownWorkspace>,
}

impl Registry<'_> {
    /// The record for `path`'s canonical form, started at `at` if new.
    fn entry(&mut self, path: &Path, at: Timestamp) -> &mut KnownWorkspace {
        let canonical = self.canonical;
        let (path, exists) = self
            .resolved
            .entry(path.to_path_buf())
            .or_insert_with(|| canonical(path))
            .clone();
        self.found.entry(path.clone()).or_insert(KnownWorkspace {
            path,
            agents: 0,
            running: 0,
            active_at: at,
            exists,
        })
    }
}

/// The workspace an agent was given, when it can be sent to a client as text.
fn usable_workspace(agent: &Agent) -> Option<&Path> {
    agent
        .scope
        .workspace_root
        .as_deref()
        .filter(|path| path.is_absolute() && path.to_str().is_some())
}

/// The root of `agent`'s tree; `None` when a parent is missing from the store,
/// and bounded so a parent cycle cannot spin.
fn tree_root<'a>(agent: &'a Agent, by_id: &HashMap<AgentId, &'a Agent>) -> Option<&'a Agent> {
    let mut node = agent;
    for _ in 0..=by_id.len() {
        match node.parent {
            None => return Some(node),
            Some(parent) => node = by_id.get(&parent).copied()?,
        }
    }
    None
}

#[cfg(test)]
mod tests;
