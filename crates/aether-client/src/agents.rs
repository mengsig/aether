use crate::{Client, Result, expect_ok, unexpected};
use aether_core::agent::Agent;
use aether_core::id::{AgentId, MessageId, RunId};
use aether_protocol::{
    AgentPatch, AgentQuery, AgentTree, Command, CreateAgent, MessageDelivery, Reply,
};

impl Client {
    pub async fn create_agent(&self, req: CreateAgent) -> Result<Agent> {
        match self.request(Command::CreateAgent(req)).await? {
            Reply::Agent(a) => Ok(a),
            other => Err(unexpected("agent", &other)),
        }
    }

    pub async fn get_agent(&self, id: AgentId) -> Result<Agent> {
        match self.request(Command::GetAgent { id }).await? {
            Reply::Agent(a) => Ok(a),
            other => Err(unexpected("agent", &other)),
        }
    }

    pub async fn list_agents(&self, query: AgentQuery) -> Result<Vec<Agent>> {
        match self.request(Command::ListAgents { query }).await? {
            Reply::Agents(a) => Ok(a),
            other => Err(unexpected("agents", &other)),
        }
    }

    /// The live graph. `include_archived` brings back what `/clear` hid.
    pub async fn tree(&self, run: Option<RunId>, include_archived: bool) -> Result<AgentTree> {
        match self
            .request(Command::GetTree {
                run,
                include_archived,
            })
            .await?
        {
            Reply::Tree(t) => Ok(t),
            other => Err(unexpected("tree", &other)),
        }
    }

    /// Clear finished work out of the live view, or bring it back.
    pub async fn archive(
        &self,
        target: aether_protocol::ArchiveTarget,
        archived: bool,
    ) -> Result<Vec<Agent>> {
        match self
            .request(Command::Archive {
                target,
                archived,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agents(a) => Ok(a),
            other => Err(unexpected("agents", &other)),
        }
    }

    pub async fn update_agent(&self, id: AgentId, patch: AgentPatch) -> Result<Agent> {
        match self
            .request(Command::UpdateAgent {
                id,
                patch,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agent(a) => Ok(a),
            other => Err(unexpected("agent", &other)),
        }
    }

    pub async fn send_message(&self, to: AgentId, content: impl Into<String>) -> Result<MessageId> {
        match self
            .request(Command::SendMessage {
                to,
                content: content.into(),
                subtree: false,
                idempotency_key: None,
            })
            .await?
        {
            Reply::MessageSent { message } => Ok(message),
            other => Err(unexpected("message_sent", &other)),
        }
    }

    /// Fan a message out to `root` and every live agent beneath it. Returns
    /// one delivery per agent reached, in delivery order.
    pub async fn send_message_subtree(
        &self,
        root: AgentId,
        content: impl Into<String>,
    ) -> Result<Vec<MessageDelivery>> {
        match self
            .request(Command::SendMessage {
                to: root,
                content: content.into(),
                subtree: true,
                idempotency_key: None,
            })
            .await?
        {
            Reply::MessagesSent { deliveries } => Ok(deliveries),
            other => Err(unexpected("messages_sent", &other)),
        }
    }

    pub async fn cancel(&self, agent: AgentId, reason: Option<String>) -> Result<()> {
        expect_ok(
            self.request(Command::Cancel {
                agent,
                reason,
                idempotency_key: None,
            })
            .await?,
        )
    }

    /// Stop the agent's running turn, keeping the agent. True when a turn
    /// was running.
    pub async fn interrupt(&self, agent: AgentId) -> Result<bool> {
        match self
            .request(Command::Interrupt {
                agent,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Interrupted { was_running } => Ok(was_running),
            other => Err(unexpected("interrupted", &other)),
        }
    }

    /// Recreate a failed or cancelled agent from its last checkpoint.
    /// Returns the new agent.
    pub async fn restart(&self, agent: AgentId) -> Result<Agent> {
        match self
            .request(Command::Restart {
                agent,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agent(a) => Ok(a),
            other => Err(unexpected("agent", &other)),
        }
    }

    /// Bring a failed agent back in place, keeping its history and context.
    /// Returns the agent, now Ready.
    pub async fn wake(&self, agent: AgentId) -> Result<Agent> {
        match self.request(Command::Wake { agent }).await? {
            Reply::Agent(a) => Ok(a),
            other => Err(unexpected("agent", &other)),
        }
    }

    /// Pause an agent, and with `subtree` every live agent beneath it.
    /// Returns the agents it moved and those finishing a turn first.
    pub async fn pause(&self, agent: AgentId, subtree: bool) -> Result<(Vec<Agent>, Vec<AgentId>)> {
        match self
            .request(Command::Pause {
                agent,
                subtree,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Paused { agents, mid_turn } => Ok((agents, mid_turn)),
            other => Err(unexpected("paused", &other)),
        }
    }

    /// Lift a pause, or clear a wait or an inspection. Returns the agents it moved.
    pub async fn resume(&self, agent: AgentId, subtree: bool) -> Result<Vec<Agent>> {
        match self
            .request(Command::Resume {
                agent,
                subtree,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agents(agents) => Ok(agents),
            other => Err(unexpected("agents", &other)),
        }
    }

    pub async fn compact(&self, agent: AgentId) -> Result<()> {
        expect_ok(self.request(Command::Compact { agent }).await?)
    }
}
