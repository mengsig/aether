use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, Scope};
use aether_core::event::Actor;
use std::path::Path;

impl Runtime {
    /// The requested scope, its repository resolved before `child_scope` narrows it: a named
    /// id is normalized or refused (never dropped); a checkout gets the id its agents derive.
    pub(super) fn requested_scope(
        &self,
        scope: Option<Scope>,
        repository_root: Option<&Path>,
        parent: Option<&Agent>,
    ) -> Result<Option<Scope>> {
        let mut requested = scope;
        if let Some(s) = &mut requested
            && let Some(named) = s.repository.take()
        {
            if repository_root.is_some() {
                return Err(RuntimeError::Invalid(
                    "give either repository_root or scope.repository, not both".into(),
                ));
            }
            let normal = aether_core::repository::normalize_id(&named).ok_or_else(|| {
                RuntimeError::Invalid(format!(
                    "repository `{named}` cannot be stored as a memory scope; expected a plain \
                     name, an absolute path, or a URL/SSH remote without embedded credentials"
                ))
            })?;
            s.repository = Some(self.store.resolve_repository(&normal)?);
        }
        let Some(root) = repository_root else {
            return Ok(requested);
        };
        let id = self
            .store
            .resolve_repository(&checkout_repository_id(root)?)?;
        let mut scope = requested.unwrap_or_else(|| match parent {
            Some(parent) => parent.scope.clone(),
            None => self.default_root_scope(),
        });
        debug_assert!(scope.repository.is_none() || parent.is_some());
        scope.repository = Some(id);
        Ok(Some(scope))
    }

    /// `scope` with its repository resolved through the registered aliases.
    /// For a parent created before an alias: its children store the canonical
    /// id, and are narrowed against it.
    pub(crate) fn resolved_scope(&self, scope: &Scope) -> Result<Scope> {
        let mut resolved = scope.clone();
        if let Some(id) = &scope.repository {
            resolved.repository = Some(self.store.resolve_repository(id)?);
        }
        debug_assert_eq!(resolved.workspace_root, scope.workspace_root);
        debug_assert_eq!(resolved.repository.is_some(), scope.repository.is_some());
        Ok(resolved)
    }

    /// The repository of an agent whose scope names none: its parent's, else
    /// the one its workspace derives. Resolved either way, so it stores the canonical id.
    pub(super) fn unnamed_repository(
        &self,
        scope: &Scope,
        parent: Option<&Agent>,
    ) -> Result<Option<String>> {
        debug_assert!(scope.repository.is_none(), "only for a scope naming none");
        // Inherited, never re-derived: a child narrowed to a subdirectory of a
        // workspace with no `origin` would derive a different id from its own
        // path and be unable to read the repository's notes or write memory its
        // siblings can see.
        let id = match parent {
            Some(p) => p.scope.repository.clone(),
            None => scope.workspace_root.as_deref().and_then(repository_id),
        };
        id.map(|id| self.store.resolve_repository(&id))
            .transpose()
            .map_err(RuntimeError::from)
    }

    /// §6.4: the notes a repository carries in its own tree become memory the
    /// first time an agent works there.
    ///
    /// Off the create path: reading the tree is filesystem work plus a git
    /// subprocess per file, and creating an agent must not wait for a
    /// repository's notes - nor fail because they cannot be read. Seeding is
    /// idempotent and takes the write path per note, so landing a moment
    /// later, or twice at once, changes nothing.
    ///
    /// Bounded at most once per agent id per process (not "already holds its
    /// notes" - that still costs the tree walk and the memory gate to find
    /// out): a client retry-storming a keyed root `create_agent` names the
    /// same agent id on every replay, so a process-local set turns every
    /// retry after the first into a hash lookup instead of a `read_dir` and a
    /// `WriteGate` acquisition per note.
    pub(crate) fn seed_repository_knowledge(&self, agent: &Agent, by: Actor) {
        // Root agents only. A child inherits its parent's repository id, so
        // the notes are already in by the time it exists, and a supervisor
        // fanning out twenty workers would otherwise rescan the tree twenty
        // times for nothing.
        if agent.parent.is_some() {
            return;
        }
        let (Some(root), Some(id)) = (&agent.scope.workspace_root, &agent.scope.repository) else {
            return;
        };
        if !self.seeded_repositories.lock().insert(agent.id) {
            return;
        }
        let (memory, root, id) = (self.memory.clone(), root.clone(), id.clone());
        let seed = move || {
            if !aether_memory::repo::has_knowledge(&root) {
                return;
            }
            match memory.seed_repository(&root, &id, by) {
                Ok(report) if report.admitted + report.superseded + report.retracted > 0 => {
                    tracing::info!(repository = %id, admitted = report.admitted, superseded = report.superseded, retracted = report.retracted, "seeded repository knowledge");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(repository = %id, error = %e, "could not read this repository's committed notes")
                }
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(seed);
            }
            // No runtime to hand it to, so it is this thread's work after all.
            Err(_) => seed(),
        }
    }
}

/// A stable name for the repository at `root`: its `origin` remote when it has
/// one, so every clone of it shares one memory scope, else the canonical path.
///
/// The remote is normalized to host and path first. A remote can carry a
/// token in its userinfo, and this id reaches the system prompt, the event
/// log and every memory row - none of which can be retracted.
pub(crate) fn repository_id(root: &std::path::Path) -> Option<String> {
    let remote = aether_memory::repo::git(root, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(aether_core::repository::normalize_remote);
    remote.or_else(|| {
        root.canonicalize()
            .ok()
            .and_then(|p| aether_core::repository::normalize_id(&p.to_string_lossy()))
    })
}

/// The id [`repository_id`] gives the checkout at `root`, which a client
/// names so agents in its worktrees share one memory scope with it.
///
/// Not bounded to known workspaces: a paired device is refused before this,
/// and it reads no more than creating an agent with `root` as its workspace
/// would - the `origin` remote and the canonical path.
pub(crate) fn checkout_repository_id(root: &Path) -> Result<String> {
    if !root.is_absolute() {
        return Err(RuntimeError::Invalid(format!(
            "repository_root `{}` must be an absolute path",
            root.display()
        )));
    }
    let metadata = std::fs::metadata(root)
        .map_err(|e| RuntimeError::Invalid(format!("repository_root {}: {e}", root.display())))?;
    if !metadata.is_dir() {
        return Err(RuntimeError::Invalid(format!(
            "repository_root {} is not a directory",
            root.display()
        )));
    }
    repository_id(root).ok_or_else(|| {
        RuntimeError::Invalid(format!(
            "repository_root {} has no remote or path that can name a memory scope",
            root.display()
        ))
    })
}
