//! Which workspace the operator is in, and switching it: the `/workspace`
//! picker and `Ctrl+O` land here. An agent's own workspace never changes;
//! switching selects a live root there or starts a new one.

use std::path::{Path, PathBuf};

use aether_core::agent::AgentStatusKind;
use aether_core::time::Timestamp;

use super::{AgentId, App, Effect};
use crate::workspace;

/// One directory holding live root agents, for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWorkspace {
    /// Canonical.
    pub path: PathBuf,
    /// Live agents in trees rooted there.
    pub agents: usize,
    pub running: usize,
    /// Latest update of any of its roots: what orders the picker.
    pub active_at: Timestamp,
}

impl App {
    pub(crate) fn workspace_path(&self, path: &Path) -> PathBuf {
        if self.options.is_remote() {
            path.to_owned()
        } else {
            workspace::canonical(path)
        }
    }
    /// Where the operator is working: the viewed agent's tree root's
    /// workspace, else the one last picked or started in, else the launch directory.
    pub fn active_workspace(&self) -> Option<&Path> {
        self.selected
            .and_then(|id| self.root_workspace(id))
            .or(self.workspace.as_deref())
            .or(self.options.cwd.as_deref())
    }

    /// The workspace of the root of `agent`'s tree.
    pub(crate) fn root_workspace(&self, agent: AgentId) -> Option<&Path> {
        let mut node = self.tree.nodes.get(&agent)?;
        // Bounded by the node count, so a parent cycle cannot spin.
        for _ in 0..self.tree.nodes.len() {
            match node.agent.parent.and_then(|p| self.tree.nodes.get(&p)) {
                Some(parent) => node = parent,
                None => break,
            }
        }
        node.agent.scope.workspace_root.as_deref()
    }

    /// Roots that can still take work, with their canonical workspace.
    fn live_roots(&self) -> impl Iterator<Item = (AgentId, Option<PathBuf>)> + '_ {
        self.tree
            .roots
            .iter()
            .filter_map(|id| self.tree.nodes.get(id))
            .filter(|node| node.agent.is_live() && !node.agent.archived)
            .map(|node| {
                let path = node.agent.scope.workspace_root.as_deref();
                (node.agent.id, path.map(|path| self.workspace_path(path)))
            })
    }

    /// The most recently updated live root working in `path`.
    pub(crate) fn latest_live_root_in(&self, path: &Path) -> Option<AgentId> {
        let path = self.workspace_path(path);
        self.live_roots()
            .filter(|(_, root)| root.as_deref() == Some(path.as_path()))
            .filter_map(|(id, _)| self.tree.nodes.get(&id))
            .max_by_key(|node| node.agent.updated_at)
            .map(|node| node.agent.id)
    }

    /// Every workspace with a live root, most recently active first.
    pub fn live_workspaces(&self) -> Vec<LiveWorkspace> {
        let mut out: Vec<LiveWorkspace> = Vec::new();
        for (id, path) in self.live_roots().collect::<Vec<_>>() {
            let Some(path) = path else { continue };
            let (agents, running) = self.live_counts(id);
            let at = self.tree.nodes[&id].agent.updated_at;
            match out.iter_mut().find(|w| w.path == path) {
                Some(seen) => {
                    seen.agents += agents;
                    seen.running += running;
                    seen.active_at = seen.active_at.max(at);
                }
                None => out.push(LiveWorkspace {
                    path,
                    agents,
                    running,
                    active_at: at,
                }),
            }
        }
        out.sort_by_key(|w| std::cmp::Reverse(w.active_at));
        out
    }

    /// Live agents and running agents in the tree under `root`, itself included.
    fn live_counts(&self, root: AgentId) -> (usize, usize) {
        let mut stack = vec![root];
        let mut seen = std::collections::HashSet::new();
        let (mut live, mut running) = (0, 0);
        while let Some(id) = stack.pop() {
            let Some(node) = self.tree.nodes.get(&id) else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            if node.agent.is_live() {
                live += 1;
            }
            if node.agent.status.kind() == AgentStatusKind::Running {
                running += 1;
            }
            stack.extend(node.children.iter().copied());
        }
        (live, running)
    }

    /// `/workspace <path>`: `~` and paths relative to the active workspace
    /// are resolved, and anything that is not a directory is refused.
    pub(crate) fn open_workspace(&mut self, typed: &Path) -> Vec<Effect> {
        let Some(text) = typed.to_str() else {
            self.refuse_non_utf8(typed);
            return Vec::new();
        };
        if self.options.is_remote() {
            return vec![super::host::request(
                super::Command::ValidateWorkspace { path: text.into() },
                super::host::HostRequest::Validate,
            )];
        }
        let home = self.options.home.as_deref();
        let resolved = workspace::resolve(text, home, self.active_workspace());
        match resolved {
            Some(path) if path.is_dir() => self.use_workspace(&path),
            _ => {
                self.flash_error(format!("no such directory: {text}"));
                Vec::new()
            }
        }
    }

    /// Work in `path`: view its live root, or start one there.
    pub(crate) fn use_workspace(&mut self, path: &Path) -> Vec<Effect> {
        let path = self.workspace_path(path);
        if path.to_str().is_none() {
            self.refuse_non_utf8(&path);
            return Vec::new();
        }
        let label = workspace::display(&path, self.options.home.as_deref());
        self.workspace = Some(path.clone());
        let viewing_here = self
            .selected
            .filter(|id| !self.is_terminal(*id))
            .and_then(|id| self.root_workspace(id))
            .is_some_and(|root| self.workspace_path(root) == path);
        // A root the operator waits on here may already be in the tree; until
        // its reply, messages still queue behind its start. A failed start
        // gives way to a live root, saying what it held.
        let effects = if viewing_here {
            self.flash(format!("working in {label}"));
            Vec::new()
        } else if let Some(root) = self
            .latest_live_root_in(&path)
            .filter(|_| !self.awaits_reply_in(&path))
        {
            self.flash(format!("working in {label}"));
            self.select(root)
        } else {
            // Nothing viewed until the new root arrives, so a message typed
            // meanwhile waits for it rather than reaching the old workspace.
            self.clear_selection();
            self.dirty = true;
            self.flash(format!("new agent in {label}"));
            self.await_root_in(Some(path.clone()), None)
                .into_iter()
                .collect()
        };
        // After the flash, so a failed save is what stays on screen.
        self.remember_workspace(&path);
        effects
    }

    /// Refused before anything uses it; see [`workspace::NotUtf8`].
    fn refuse_non_utf8(&mut self, path: &Path) {
        self.flash_error(workspace::NotUtf8(path.to_path_buf()).to_string());
    }

    /// Move `path` to the front of the picker's recents, in memory and on disk.
    pub(super) fn remember_workspace(&mut self, path: &Path) {
        let path = self.workspace_path(path);
        let Some(name) = path.to_str().map(str::to_owned) else {
            self.refuse_non_utf8(&path);
            return;
        };
        self.recent_workspaces.retain(|w| *w != name);
        self.recent_workspaces.insert(0, name.clone());
        self.recent_workspaces
            .truncate(crate::prefs::RECENT_WORKSPACES);
        if let Err(reason) = self.save_prefs(|p| p.remember_workspace(&name)) {
            self.flash_error(format!("recent workspace not saved: {reason}"));
        }
    }

    /// Pin `path` after the other pins, or unpin it, in memory and on disk.
    pub(crate) fn pin_workspace(&mut self, path: &Path, pinned: bool) {
        let path = self.workspace_path(path);
        let Some(name) = path.to_str().map(str::to_owned) else {
            self.refuse_non_utf8(&path);
            return;
        };
        let home = self.options.home.clone();
        let home = home.as_deref();
        set_pin(&mut self.pinned_workspaces, &name, pinned, home);
        let label = workspace::display(&path, home);
        let saved = self.save_prefs(|p| set_pin(&mut p.pinned_workspaces, &name, pinned, home));
        match (saved, pinned) {
            (Ok(()), true) => self.flash(format!("pinned {label}")),
            (Ok(()), false) => self.flash(format!("unpinned {label}")),
            (Err(reason), _) => self.flash_error(format!("pin not saved: {reason}")),
        }
    }

    /// Opening the picker: say which configured discovery roots it could not
    /// search, the first few by name.
    pub(crate) fn flag_missing_workspace_roots(&mut self, missing: &[String]) {
        const NAMED: usize = 3;
        if missing.is_empty() {
            return;
        }
        let noun = if missing.len() == 1 { "root" } else { "roots" };
        let mut text = format!(
            "workspace {noun} not found: {}",
            missing[..missing.len().min(NAMED)].join(", ")
        );
        if missing.len() > NAMED {
            text.push_str(&format!(" and {} more", missing.len() - NAMED));
        }
        self.flash(text);
    }
}

/// `pins` with the directory `name` names added at the end, or removed
/// however `ui.toml` spells it; pinning one already pinned keeps its place.
fn set_pin(pins: &mut Vec<String>, name: &str, pinned: bool, home: Option<&Path>) {
    let path = Path::new(name);
    let names_it = |pin: &String| workspace::pinned_path(pin, home).as_deref() == Some(path);
    if !pinned {
        pins.retain(|pin| !names_it(pin));
    } else if !pins.iter().any(names_it) {
        pins.push(name.to_owned());
    }
}
