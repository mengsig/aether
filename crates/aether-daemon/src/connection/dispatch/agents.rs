//! Creating, reading, updating and archiving agents, their workspaces and
//! their engine tools.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::{AgentUpdate, CreateAgentSpec};

use crate::connection::dispatch::blocking;
use crate::connection::{map_err, store_err};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::CreateAgent(_)
            | Command::GetAgent { .. }
            | Command::ListAgents { .. }
            | Command::Archive { .. }
            | Command::UpdateAgent { .. }
            | Command::WorkspaceDiff { .. }
            | Command::WorkspaceIntegrate { .. }
            | Command::WorkspaceDiscard { .. }
            | Command::EngineTool { .. }
            | Command::EngineTools { .. }
    )
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let store = rt.store().as_ref();
    match command {
        Command::CreateAgent(req) => create_agent(daemon, req, by, submission).await,
        Command::GetAgent { id } => {
            let mut agents = vec![
                store
                    .agent(id)
                    .map_err(store_err)?
                    .ok_or_else(|| ErrorReply::not_found(id))?,
            ];
            rt.annotate_agents(&mut agents).map_err(map_err)?;
            Ok(Reply::Agent(agents.remove(0)))
        }
        Command::ListAgents { query } => {
            let runtime = rt.clone();
            let agents = blocking(move || {
                let mut agents = runtime.store().agents(&query.into())?;
                runtime.annotate_agents(&mut agents)?;
                Ok::<_, aether_runtime::RuntimeError>(agents)
            })
            .await?;
            Ok(Reply::Agents(agents.map_err(map_err)?))
        }
        Command::Archive {
            target, archived, ..
        } => Ok(Reply::Agents(
            rt.archive_agents_keyed(target, archived, by, submission)
                .map_err(map_err)?,
        )),
        Command::UpdateAgent {
            id, patch, subtree, ..
        } => update_agent(daemon, id, patch, subtree, by, submission),
        Command::WorkspaceDiff { agent } => workspace_diff(daemon, agent).await,
        Command::WorkspaceIntegrate { agent } => workspace_integrate(daemon, agent, by).await,
        Command::WorkspaceDiscard { agent, force } => {
            workspace_discard(daemon, agent, force, by).await
        }
        Command::EngineTool {
            agent,
            token,
            name,
            input,
        } => engine_tool(daemon, agent, &token, name, input).await,
        Command::EngineTools { agent, token } => Ok(Reply::EngineTools {
            tools: rt.engine_tools(agent, &token).map_err(map_err)?,
        }),
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn create_agent(
    daemon: &Daemon,
    req: aether_protocol::CreateAgent,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    if rt.default_model().is_none() && req.model.is_none() {
        rt.set_default_model(daemon.models.default_model().await);
    }
    // A parked agent may wait for its CLI to be installed; one started now cannot.
    if req.start {
        // A retry of a creation that succeeded is answered, not checked again.
        if let Some(submission) = &submission
            && let Some((agent, _)) = rt
                .replay_created_agent(submission, by.clone())
                .map_err(map_err)?
        {
            return Ok(Reply::Agent(agent));
        }
        rt.check_engine_for(req.model.as_ref(), req.parent)
            .await
            .map_err(map_err)?;
    }
    let spec = CreateAgentSpec {
        role: req.role,
        name: req.name,
        objective: req.objective,
        model: req.model,
        fallbacks: req.fallbacks,
        scope: req.scope,
        repository_root: req.repository_root,
        parent: req.parent,
        config: req.config,
        approval: req.approval,
        run: req.run,
        start: req.start,
    };
    let (agent, _) = rt
        .create_agent_keyed(spec, by, submission)
        .map_err(map_err)?;
    Ok(Reply::Agent(agent))
}

fn update_agent(
    daemon: &Daemon,
    id: aether_core::id::AgentId,
    patch: aether_protocol::AgentPatch,
    subtree: bool,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let update = AgentUpdate {
        name: patch.name,
        model: patch.model,
        fallbacks: patch.fallbacks,
        effort: patch.effort,
        instructions: patch.instructions,
        max_turns: patch.max_turns,
        token_budget: patch.token_budget,
        cost_budget: patch.cost_budget,
        tools: patch.tools,
        approval: patch.approval,
        priority: patch.priority,
        concurrency: patch.concurrency,
        deadline: patch.deadline,
        long_lived: patch.long_lived,
        network: patch.network,
        backend: patch.backend,
    };
    Ok(Reply::Agent(
        rt.update_agent_keyed(id, update, subtree, by, submission)
            .map_err(map_err)?,
    ))
}

async fn workspace_diff(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
) -> Result<Reply, ErrorReply> {
    let reading = daemon.runtime.clone();
    let diff = blocking(move || reading.workspace_diff(agent).map_err(map_err)).await??;
    Ok(Reply::WorkspaceDiff(Box::new(diff)))
}

async fn workspace_integrate(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let writing = daemon.runtime.clone();
    let integration =
        blocking(move || writing.workspace_integrate(agent, by).map_err(map_err)).await??;
    Ok(Reply::WorkspaceIntegration(integration))
}

async fn workspace_discard(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    force: bool,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let writing = daemon.runtime.clone();
    let discard =
        blocking(move || writing.workspace_discard(agent, force, by).map_err(map_err)).await??;
    Ok(Reply::WorkspaceDiscarded(discard))
}

async fn engine_tool(
    daemon: &Daemon,
    agent: aether_core::id::AgentId,
    token: &str,
    name: String,
    input: serde_json::Value,
) -> Result<Reply, ErrorReply> {
    let (content, is_error) = daemon
        .runtime
        .engine_tool(agent, token, name, input)
        .await
        .map_err(map_err)?;
    Ok(Reply::EngineTool { content, is_error })
}
