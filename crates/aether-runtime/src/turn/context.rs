use super::*;

pub(crate) fn tool_context(runtime: &Arc<Runtime>, ctx: &TurnCtx) -> ToolContext {
    let cwd = ctx
        .agent
        .scope
        .workspace_root
        .clone()
        .or_else(|| runtime.config.workspace_root.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    ToolContext {
        agent_id: ctx.agent.id,
        task_id: ctx.task.as_ref().map(|t| t.id),
        instance_id: runtime.config.instance_id.clone(),
        scope: ctx.agent.scope.clone(),
        cwd,
        artifacts: Arc::new(StoreArtifactSink {
            runtime: runtime.clone(),
        }),
        env: BTreeMap::new(),
        hidden_env: runtime.tools.hidden_env(),
        temp_dir: agent_temp_path(runtime, ctx.agent.id),
        build_env: crate::build_cache::exports(runtime, &ctx.agent),
    }
}

/// Where `agent`'s temp dir lives; nothing is created.
pub(crate) fn agent_temp_path(runtime: &Runtime, agent: AgentId) -> Option<std::path::PathBuf> {
    let root = runtime.config.agent_temp_root.as_deref()?;
    Some(aether_core::agent_temp::leaf_path(root, agent))
}

/// `agent`'s temp dir, created (or recreated) now because a process of the
/// agent's is about to start. A dir that is not safely ours stops the turn.
pub(crate) fn agent_temp_dir(
    runtime: &Runtime,
    agent: AgentId,
) -> Result<Option<std::path::PathBuf>> {
    let Some(leaf) = agent_temp_path(runtime, agent) else {
        return Ok(None);
    };
    aether_core::agent_temp::ensure_agent_leaf(&leaf, agent, &runtime.config.instance_id)
        .map_err(|e| RuntimeError::Invalid(format!("agent temp dir: {e}")))?;
    Ok(Some(leaf))
}

/// Memories for this turn, ranked by the task objective and the inbox about
/// to be consumed. Shared by the API and engine paths so both see one set.
pub(crate) fn retrieve_memories(
    runtime: &Runtime,
    ctx: &TurnCtx,
    inbox: &[aether_core::inbox::InboxItem],
) -> Result<Vec<aether_memory::Retrieved>> {
    let mut query = ctx
        .task
        .as_ref()
        .map(|t| t.objective.clone())
        .unwrap_or_default();
    for item in inbox {
        if let InboxKind::Message { content, .. } = &item.kind {
            query.push(' ');
            query.push_str(&aether_core::model::text_of(content));
        }
    }
    let scope = ctx
        .agent
        .scope
        .repository
        .clone()
        .map(|id| MemoryScope::Repository { id });
    let ancestors = lineage(ctx.agent.id, ctx.agent.parent, |id| {
        Ok::<_, aether_store::StoreError>(runtime.store.agent(id)?.and_then(|a| a.parent))
    })?;
    Ok(runtime.memory.retrieve(&RetrievalRequest {
        scope,
        role: Some(ctx.agent.role.as_str().to_owned()),
        agent: Some(ctx.agent.id),
        ancestors,
        query: Some(query).filter(|q| !q.trim().is_empty()),
        tags: Vec::new(),
        kinds: Vec::new(),
        limit: 6,
    })?)
}

/// `agent`'s nearest ancestors, at most [`aether_memory::MAX_LINEAGE`], whose
/// subtree memories it inherits. Stops early at a missing agent or a cycle.
pub(super) fn lineage<E>(
    agent: AgentId,
    parent: Option<AgentId>,
    mut parent_of: impl FnMut(AgentId) -> std::result::Result<Option<AgentId>, E>,
) -> std::result::Result<Vec<AgentId>, E> {
    let mut lineage = Vec::new();
    let mut next = parent;
    while let Some(id) = next {
        if lineage.len() == aether_memory::MAX_LINEAGE || id == agent || lineage.contains(&id) {
            break;
        }
        lineage.push(id);
        next = parent_of(id)?;
    }
    Ok(lineage)
}
