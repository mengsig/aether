use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, ApprovalPolicy, ExecutionBackend, Scope};
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_store::AgentFilter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A child may gate more than its parent, never less. One rule,
/// one message, whether it fires at creation or on a live agent.
pub(super) fn check_narrows_parent(policy: &ApprovalPolicy, parent: &Agent) -> Result<()> {
    if policy.is_narrowing_of(&parent.config.approval) {
        return Ok(());
    }
    Err(RuntimeError::Invalid(format!(
        "cannot drop a gate its parent {} keeps; change the parent first",
        parent.id
    )))
}

/// The longest agent name, in characters.
const MAX_AGENT_NAME_CHARS: usize = 256;

/// An agent name as stored: trimmed, 1 to [`MAX_AGENT_NAME_CHARS`] characters,
/// one line. Shared by creation and rename so they cannot disagree.
pub(super) fn checked_agent_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_AGENT_NAME_CHARS
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(RuntimeError::Invalid(format!(
            "name must contain 1 to {MAX_AGENT_NAME_CHARS} characters without line breaks or control characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Where `path` leads once its links are followed as the file tools follow
/// them. A relative path is read from the daemon's directory, as the OS reads it.
fn leads_to(path: &Path) -> std::result::Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let resolved = aether_tools::fs::resolve_links(&absolute).map_err(|error| error.to_string())?;
    debug_assert!(resolved.is_absolute(), "resolved from an absolute path");
    Ok(resolved)
}

/// The real path of `path`, or where it will be once created as written.
/// `None`: it cannot be read, or its spelling climbs out of what is missing.
fn real_or_once_created(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(real) => {
                debug_assert!(real.is_absolute());
                return Some(
                    missing
                        .iter()
                        .rev()
                        .fold(real, |path, name| path.join(name)),
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        // `None` for `..`: once created, such a path is somewhere else.
        missing.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// The directories `parent`'s file tools reach, counting a path of its that
/// is not there yet: its root and allowed paths, or only `cwd` without a root.
fn granted_roots(parent: &Scope, cwd: &Path) -> Vec<PathBuf> {
    let named: Vec<&Path> = match &parent.workspace_root {
        Some(root) => std::iter::once(root.as_path())
            .chain(parent.allowed_paths.iter().map(PathBuf::as_path))
            .collect(),
        // Without a root its file tools work in `cwd` and ignore `allowed_paths`.
        None => vec![cwd],
    };
    debug_assert!(!named.is_empty(), "a scope works somewhere");
    let granted: Vec<PathBuf> = named.into_iter().filter_map(real_or_once_created).collect();
    debug_assert!(granted.iter().all(|root| root.is_absolute()));
    granted
}

/// The field on which `scope` leaves `parent` once links are followed, as
/// the file tools follow them. `cwd` is where a parent with no root works.
fn widened_on_disk(scope: &Scope, parent: &Scope, cwd: &Path) -> Result<Option<&'static str>> {
    let granted = granted_roots(parent, cwd);
    let inside = |path: &Path| -> Result<bool> {
        let leads_to = leads_to(path).map_err(|error| {
            RuntimeError::Invalid(format!(
                "child scope path {} cannot be resolved: {error}",
                path.display()
            ))
        })?;
        Ok(granted.iter().any(|root| leads_to.starts_with(root)))
    };
    if let Some(root) = &scope.workspace_root
        && scope.workspace_root != parent.workspace_root
        && !inside(root)?
    {
        return Ok(Some("workspace_root"));
    }
    // The parent's own allowed path leads where the parent's does, unless the
    // child would use it and the parent cannot: a rootless agent's are unused.
    let same_use = parent.workspace_root.is_some() || scope.workspace_root.is_none();
    for path in &scope.allowed_paths {
        let parents_own = same_use && parent.allowed_paths.contains(path);
        if !parents_own && !inside(path)? {
            return Ok(Some("allowed_paths"));
        }
    }
    Ok(None)
}

impl Runtime {
    /// Apply `guard` to every link a turn may land on, primary first. A
    /// fallback is not a lesser model: a turn runs on it with the same budget,
    /// scope and credentials, so it faces the same guards. Only a
    /// refused fallback is labelled, so a single-model spec reads as before.
    fn for_each_link(
        primary: &ModelRef,
        fallbacks: &[ModelRef],
        guard: impl Fn(&ModelRef) -> Result<()>,
    ) -> Result<()> {
        guard(primary)?;
        for link in fallbacks {
            guard(link).map_err(|e| match e {
                // A refusal says which link earned it; anything else - an
                // unresolvable link - keeps its own class and already names
                // the provider and model it could not build.
                RuntimeError::Invalid(why) => {
                    RuntimeError::Invalid(format!("fallback model `{link}`: {why}"))
                }
                other => other,
            })?;
        }
        Ok(())
    }

    /// VISION §16: an external engine (Claude Code, Codex) runs its own tools
    /// on the daemon's host - `turn_engine` never reads `scope.backend` - so
    /// `--backend container` on an engine-backed agent would read as
    /// confinement and provide none. Refuse rather than project a boundary
    /// that is not there.
    pub(super) fn check_backend_supported(
        &self,
        model: &ModelRef,
        fallbacks: &[ModelRef],
        scope: &Scope,
    ) -> Result<()> {
        if scope.backend == ExecutionBackend::Host {
            return Ok(());
        }
        Self::for_each_link(model, fallbacks, |link| {
            let resolved = self.models.resolve(link)?;
            if crate::turn_engine::engine_kind(resolved.provider.protocol).is_some() {
                return Err(RuntimeError::Invalid(format!(
                    "provider `{}` is an external engine and runs its tools on the host, so a \
                     container backend would confine nothing; use an API model for \
                     container-scoped work",
                    link.provider
                )));
            }
            Ok(())
        })
    }

    /// VISION §1.6 with §8: a subscription login (Claude Code, Codex) is paid
    /// for by the seat, an API provider by the token. A chain that starts on
    /// the subscription and rolls onto a metered API bills the person for work
    /// they asked the subscription to do, and it would do it silently, so it
    /// is refused here rather than discovered on the invoice.
    /// The other direction adds no bill and is left alone.
    ///
    /// A subscription is reachable only through the engine that owns it - the
    /// registry refuses one over HTTP (`refuse_subscription_over_http`) - so
    /// an engine protocol is the whole test for whether the primary is on a
    /// seat. A link is on some other bill unless it is an engine too, or a
    /// link nobody is billed for - see [`Self::bills_nobody`].
    pub(crate) fn check_billing_class(
        &self,
        model: &ModelRef,
        fallbacks: &[ModelRef],
    ) -> Result<()> {
        // An unbuildable primary is not an exemption: it fails here exactly as
        // it fails the other two guards, and a patch can move both halves at
        // once. Staying quiet would leave a crossing standing the moment the
        // provider came back. Both resolves here can only be reading a stored
        // value, since every call site resolves what its caller supplied
        // first, so they say which half of the chain the caller never touched.
        if fallbacks.is_empty() {
            return Ok(());
        }
        let primary = self.models.resolve(model).map_err(|e| {
            RuntimeError::Models(aether_model::ModelsError::Unbuildable(format!(
                "this agent's model `{model}` cannot be built: {e}; fix its provider or \
                 credentials, or choose a different model"
            )))
        })?;
        if !primary.provider.protocol.is_engine() {
            return Ok(());
        }
        for link in fallbacks {
            let resolved = self.models.resolve(link).map_err(|e| {
                RuntimeError::Models(aether_model::ModelsError::Unbuildable(format!(
                    "fallback model `{link}` cannot be built: {e}; fix its provider or \
                     credentials, or drop the fallback"
                )))
            })?;
            // Another engine is the same class.
            if resolved.provider.protocol.is_engine() || Self::bills_nobody(&resolved) {
                continue;
            }
            return Err(RuntimeError::Invalid(format!(
                "fallback model `{link}` is separately billed, but the primary `{model}` runs \
                 on a subscription; a fallback must not move this agent onto another bill. Drop \
                 the link, create the agent on `{link}` directly, or if it is genuinely free \
                 declare that in its provider config with an explicit `cost = 0` for every rate"
            )));
        }
        Ok(())
    }

    /// Only an *explicitly* declared price of $0 for every rate, on a link
    /// that also needs no credentials, counts as billing nobody. An absent
    /// price is unknown, not free - it is treated as metered, so a
    /// `providers.toml` entry with no `cost` never passes this guard even
    /// without credentials. The local providers aether ships
    /// (ollama, vllm, lmstudio, llamacpp) declare $0.00 explicitly for this
    /// reason.
    fn bills_nobody(link: &aether_model::ResolvedModel) -> bool {
        let free = link
            .model
            .capabilities
            .cost
            .as_ref()
            .is_some_and(|c| c.rates().all(|rate| rate <= 0.0));
        link.provider.auth.none && free
    }

    /// Where an agent whose scope names no workspace root works.
    pub(crate) fn unscoped_cwd(&self) -> Option<std::path::PathBuf> {
        self.config
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
    }

    /// What a root agent that asked for no scope works in.
    pub(super) fn default_root_scope(&self) -> Scope {
        Scope {
            network: self.config.network_default,
            workspace_root: self.unscoped_cwd(),
            ..Default::default()
        }
    }

    /// The scope a new agent gets. One exit, so no arm can adopt a scope
    /// without the narrowing check: an adopted parent scope goes through it
    /// exactly as an explicit one does. A root agent has nothing above it to
    /// narrow, so its caller chooses.
    pub(super) fn child_scope(
        &self,
        requested: Option<Scope>,
        parent: Option<&Agent>,
        isolated_workspace: Option<&std::path::Path>,
    ) -> Result<Scope> {
        let Some(parent) = parent else {
            return Ok(requested.unwrap_or_else(|| self.default_root_scope()));
        };
        let parent_scope = self.resolved_scope(&parent.scope)?;
        let scope = requested.unwrap_or_else(|| parent_scope.clone());
        let (checked, parent_scope) = if let Some(workspace) = isolated_workspace {
            (
                self.isolated_source_scope(&scope, workspace, parent.id)?,
                Self::canonical_workspace_scope(&parent_scope)?,
            )
        } else {
            (scope.clone(), parent_scope)
        };
        let widened = self.widened_scope(&checked, &parent_scope)?;
        match widened {
            Some(field) => Err(RuntimeError::Invalid(format!(
                "child scope must narrow the parent's scope; `{field}` widens it"
            ))),
            None => Ok(scope),
        }
    }

    fn isolated_source_scope(
        &self,
        scope: &Scope,
        workspace: &Path,
        parent: AgentId,
    ) -> Result<Scope> {
        if scope.workspace_root.as_deref() != Some(workspace) {
            return Err(RuntimeError::Invalid(
                "isolated delegation scope changed before creation; retry delegation".into(),
            ));
        }
        let managed_root = self
            .config
            .delegated_workspace_root
            .as_deref()
            .ok_or_else(|| RuntimeError::Invalid("managed workspaces are not configured".into()))?;
        let manifest = crate::workspace::load(managed_root, workspace)?;
        debug_assert_eq!(manifest.workspace, workspace);
        if manifest.parent != parent {
            return Err(RuntimeError::Invalid(
                "isolated workspace parent does not match the delegating agent".into(),
            ));
        }
        let logical = self.remap_managed_scope(scope, &manifest)?;
        debug_assert_eq!(
            logical.workspace_root.as_deref(),
            Some(manifest.source.as_path())
        );
        Ok(logical)
    }

    fn canonical_workspace_scope(scope: &Scope) -> Result<Scope> {
        let Some(root) = &scope.workspace_root else {
            return Ok(scope.clone());
        };
        let canonical = std::fs::canonicalize(root).map_err(|error| {
            RuntimeError::Invalid(format!(
                "workspace {} is unavailable: {error}",
                root.display()
            ))
        })?;
        let mut normalized = scope.clone();
        normalized.workspace_root = Some(canonical.clone());
        normalized.allowed_paths = scope
            .allowed_paths
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .map_or_else(|_| path.clone(), |relative| canonical.join(relative))
            })
            .collect();
        Ok(normalized)
    }

    fn workspace_needs_remap(child: &Agent, parent: &Agent) -> bool {
        match (&child.scope.workspace_root, &parent.scope.workspace_root) {
            (Some(child), Some(parent)) => !child.starts_with(parent),
            (Some(_), None) => false,
            _ => false,
        }
    }

    fn remap_managed_scope(
        &self,
        scope: &Scope,
        manifest: &crate::workspace::WorkspaceManifest,
    ) -> Result<Scope> {
        let workspace = scope
            .workspace_root
            .as_deref()
            .ok_or_else(|| RuntimeError::Invalid("managed workspace scope has no root".into()))?;
        let relative = workspace.strip_prefix(&manifest.workspace).map_err(|_| {
            RuntimeError::Invalid("scope root is outside its managed workspace".into())
        })?;
        let mut logical = scope.clone();
        logical.workspace_root = Some(manifest.source.join(relative));
        logical.allowed_paths = scope
            .allowed_paths
            .iter()
            .map(|path| {
                path.strip_prefix(&manifest.workspace)
                    .map(|relative| manifest.source.join(relative))
                    .map_err(|_| {
                        RuntimeError::Invalid(format!(
                            "allowed path {} escapes managed workspace {}",
                            path.display(),
                            manifest.workspace.display()
                        ))
                    })
            })
            .collect::<Result<_>>()?;
        let snapshot = Scope {
            workspace_root: Some(manifest.workspace.clone()),
            ..Default::default()
        };
        if let Some(field) = widened_on_disk(scope, &snapshot, &manifest.workspace)? {
            return Err(RuntimeError::Invalid(format!(
                "isolated delegation `{field}` leads outside its workspace snapshot"
            )));
        }
        debug_assert_eq!(logical.allowed_paths.len(), scope.allowed_paths.len());
        debug_assert_eq!(logical.network, scope.network);
        Ok(logical)
    }

    fn scope_through_parent(&self, scope: &Scope, child: &Agent, parent: &Agent) -> Result<Scope> {
        if !Self::workspace_needs_remap(child, parent) {
            return Ok(scope.clone());
        }
        let Some(root) = child.scope.workspace_root.as_deref() else {
            return Ok(scope.clone());
        };
        let Some(managed_root) = self.config.delegated_workspace_root.as_deref() else {
            return Ok(scope.clone());
        };
        let canonical_managed_root = std::fs::canonicalize(managed_root).map_err(|error| {
            RuntimeError::Invalid(format!(
                "managed workspace directory {} is unavailable: {error}",
                managed_root.display()
            ))
        })?;
        if !root.starts_with(&canonical_managed_root) {
            return Ok(scope.clone());
        }
        let manifest = crate::workspace::load(managed_root, root)?;
        if manifest.parent != parent.id {
            return Err(RuntimeError::Invalid(format!(
                "agent {}'s workspace parent does not match {}",
                child.id, parent.id
            )));
        }
        self.remap_managed_scope(scope, &manifest)
    }

    pub(crate) fn scope_widened_for_existing(
        &self,
        scope: &Scope,
        child: &Agent,
        parent: &Agent,
    ) -> Result<Option<&'static str>> {
        let logical = self.scope_through_parent(scope, child, parent)?;
        let parent_scope = if Self::workspace_needs_remap(child, parent) {
            Self::canonical_workspace_scope(&parent.scope)?
        } else {
            parent.scope.clone()
        };
        self.widened_scope(
            &self.resolved_scope(&logical)?,
            &self.resolved_scope(&parent_scope)?,
        )
    }

    fn widened_scope(&self, scope: &Scope, parent: &Scope) -> Result<Option<&'static str>> {
        if let Some(field) = scope.widened_field(parent) {
            return Ok(Some(field));
        }
        let cwd = self.unscoped_cwd().unwrap_or_else(|| PathBuf::from("."));
        widened_on_disk(scope, parent, &cwd)
    }

    /// The field on which `scope` widens `parent`, both repositories resolved:
    /// an agent created before an alias still names the old id.
    pub(crate) fn widened_field_resolved(
        &self,
        scope: &Scope,
        parent: &Scope,
    ) -> Result<Option<&'static str>> {
        Ok(self
            .resolved_scope(scope)?
            .widened_field(&self.resolved_scope(parent)?))
    }

    /// Project a descendant's physical workspace through every managed
    /// worktree boundary on its path to an ancestor.
    pub(crate) fn scope_within_ancestor(
        &self,
        descendant: &Agent,
        ancestor: &Agent,
        ancestor_scope: &Scope,
    ) -> Result<bool> {
        let mut current = descendant.clone();
        let mut scope = descendant.scope.clone();
        let mut canonicalized = false;
        let mut seen = std::collections::HashSet::new();
        while current.id != ancestor.id {
            if !seen.insert(current.id) {
                return Err(RuntimeError::Invalid("cycle in agent ancestry".into()));
            }
            let parent_id = current.parent.ok_or_else(|| {
                RuntimeError::Invalid(format!(
                    "agent {} is not a descendant of {}",
                    descendant.id, ancestor.id
                ))
            })?;
            let parent = self
                .store
                .agent(parent_id)?
                .ok_or_else(|| RuntimeError::NotFound(parent_id.to_string()))?;
            let remapped = Self::workspace_needs_remap(&current, &parent);
            scope = self.scope_through_parent(&scope, &current, &parent)?;
            canonicalized |= remapped;
            let parent_scope = if canonicalized {
                Self::canonical_workspace_scope(&parent.scope)?
            } else {
                parent.scope.clone()
            };
            if parent.id == ancestor.id {
                let comparison = if canonicalized {
                    Self::canonical_workspace_scope(ancestor_scope)?
                } else {
                    ancestor_scope.clone()
                };
                return Ok(self.widened_field_resolved(&scope, &comparison)?.is_none());
            }
            if self
                .widened_field_resolved(&scope, &parent_scope)?
                .is_some()
            {
                return Ok(false);
            }
            current = parent;
        }
        Ok(self
            .widened_field_resolved(&scope, ancestor_scope)?
            .is_none())
    }

    /// Why this run may start no more work, if it may not: it has spent its
    /// `[scheduler] run_budget_usd`. Turns already running are never
    /// cancelled for it - the budget bounds what a run may still start, not
    /// what it has already committed to. The budget is daemon-wide, applies
    /// to every run alike, and is cumulative: spend never ages out.
    ///
    /// The scan is O(agents in the run), so it runs twice: unheld first, to
    /// reject a crossed budget without touching the daemon's write path at
    /// all, then under the commit hold for the authoritative verdict.
    ///
    /// Only the known cost floor can cross a budget. `cost_line` indicates
    /// when that floor leaves out unpriced usage.
    pub(super) fn run_budget_refusal(&self, run: aether_core::id::RunId) -> Result<Option<String>> {
        let Some(budget) = self.config.run_budget_usd else {
            return Ok(None);
        };
        let members = self.store.agents(&AgentFilter {
            run: Some(run),
            ..Default::default()
        })?;
        let spent = aether_core::budget::run_usage(&members);
        if spent.cost_usd < budget {
            return Ok(None);
        }
        Ok(Some(format!(
            "run {run} has spent {} of its ${budget} budget \
             (`[scheduler] run_budget_usd`). Turns already running continue. Finish with \
             the agents this run already has, or ask the operator to raise the budget.",
            spent.cost_line()
        )))
    }

    /// The agent, or it and every live agent beneath it, root first. Refuses
    /// a root that has already ended: acting on what cannot run again says
    /// nothing.
    pub(super) fn live_targets(
        &self,
        root: AgentId,
        subtree: bool,
        role: &str,
    ) -> Result<Vec<Agent>> {
        let agent = self.live_agent(root, role)?;
        if !subtree {
            return Ok(vec![agent]);
        }
        let agents = self.store.subtree_agents(root)?;
        let by_id: HashMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();
        Ok(aether_core::agent::subtree_of(&agents, root)
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .filter(|a| !a.status.is_terminal())
            .cloned()
            .collect())
    }

    /// The first live descendant whose scope would not sit inside `scope`.
    pub(super) fn widest_live_descendant(
        &self,
        root: AgentId,
        scope: &Scope,
    ) -> Result<Option<AgentId>> {
        let agents = self.store.subtree_agents(root)?;
        let by_id: HashMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();
        let ancestor = by_id
            .get(&root)
            .copied()
            .ok_or_else(|| RuntimeError::NotFound(root.to_string()))?;
        for id in aether_core::agent::subtree_of(&agents, root) {
            if id == root {
                continue;
            }
            if let Some(agent) = by_id.get(&id).copied()
                && !agent.status.is_terminal()
                && !self.scope_within_ancestor(agent, ancestor, scope)?
            {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// The first live descendant whose approval would no longer gate at
    /// least as much as `policy` (the inheritance rule, checked
    /// against existing children rather than only at creation).
    pub(super) fn least_gated_live_descendant(
        &self,
        root: AgentId,
        policy: &ApprovalPolicy,
    ) -> Result<Option<AgentId>> {
        let agents = self.store.subtree_agents(root)?;
        let by_id: HashMap<AgentId, &Agent> = agents.iter().map(|a| (a.id, a)).collect();
        Ok(aether_core::agent::subtree_of(&agents, root)
            .into_iter()
            .filter(|id| *id != root)
            .find(|id| {
                by_id.get(id).is_some_and(|a| {
                    !a.status.is_terminal() && !a.config.approval.is_narrowing_of(policy)
                })
            }))
    }

    /// An agent that can still be acted on, named by the role it is being
    /// asked to play. A terminal agent is a record, not a participant: an
    /// imported one carries another machine's scope and instructions, and
    /// letting it be a parent or a task owner would put a file this daemon
    /// was handed in charge of what a live agent may do (§13.2).
    pub(super) fn live_agent(&self, id: AgentId, role: &str) -> Result<Agent> {
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {} and cannot be a {role}",
                agent.status.kind()
            )));
        }
        Ok(agent)
    }

    /// A live agent's policy is held to the same rule as a new child's.
    pub(super) fn check_approval_against_parent(
        &self,
        agent: &Agent,
        policy: &ApprovalPolicy,
    ) -> Result<()> {
        let Some(parent_id) = agent.parent else {
            return Ok(());
        };
        let Some(parent) = self.store.agent(parent_id)? else {
            return Ok(());
        };
        check_narrows_parent(policy, &parent)
    }

    /// Check stored children when they are about to run again. An ancestor may
    /// have tightened its policy while a child was terminal or on an old daemon.
    pub(crate) fn ancestor_approval_refusal(&self, agent: &Agent) -> Result<Option<String>> {
        let Some(parent) = agent.parent.filter(|_| !self.config.yolo) else {
            return Ok(None);
        };
        let ancestors = self.store.ancestry(&[parent])?;
        debug_assert!(ancestors.is_empty() || ancestors.iter().any(|row| row.id == parent));
        let stricter = ancestors.iter().find(|ancestor| {
            !agent
                .config
                .approval
                .is_narrowing_of(&ancestor.config.approval)
        });
        Ok(stricter.map(|ancestor| {
            format!(
                "agent {}'s approval policy drops gates required by ancestor {}; tighten its \
                 approvals before it can run again",
                agent.id, ancestor.id
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::Runtime;
    use aether_core::model::{ModelCost, ModelCostTier, ModelRef};
    use aether_model::Registry;

    #[test]
    fn a_paid_long_prompt_tier_is_not_a_free_fallback() {
        let mut link = Registry::builtin()
            .resolve(&ModelRef::new("ollama", "local"))
            .unwrap();
        assert!(Runtime::bills_nobody(&link));
        link.model.capabilities.cost = Some(ModelCost {
            above_threshold: Some(ModelCostTier {
                prompt_tokens: 100_000,
                input_per_mtok: 0.50,
                output_per_mtok: 2.50,
                cache_read_per_mtok: 0.05,
                cache_write_per_mtok: 0.625,
                cache_write_1h_per_mtok: None,
            }),
            ..Default::default()
        });
        assert!(!Runtime::bills_nobody(&link));
    }
}
