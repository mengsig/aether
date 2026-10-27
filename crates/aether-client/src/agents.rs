use crate::{Client, ClientError, Result, expect_ok, unexpected};
use aether_core::agent::Agent;
use aether_core::id::{AgentId, MessageId, RunId};
use aether_protocol::{
    AgentPatch, AgentQuery, AgentTree, Command, CreateAgent, ImageAttachment, MessageDelivery,
    Reply,
};

impl Client {
    /// A daemon that cannot derive a repository from `repository_root` would
    /// create the agent under its workspace's repository instead, and one
    /// without `build_cache` would build elsewhere, so both are refused here.
    pub async fn create_agent(&self, req: CreateAgent) -> Result<Agent> {
        let supports = |wanted: &str| {
            self.welcome()
                .is_some_and(|w| w.features.iter().any(|f| f == wanted))
        };
        if req.repository_root.is_some() && !supports(aether_protocol::features::REPOSITORY_ROOT) {
            return Err(ClientError::Incompatible(
                "daemon cannot derive a repository from repository_root; update Aether".into(),
            ));
        }
        let names_cache = req.config.as_ref().is_some_and(|c| c.build_cache.is_some());
        if names_cache && !supports(aether_protocol::features::BUILD_CACHE) {
            return Err(ClientError::Incompatible(
                "daemon cannot set an agent's build_cache; update Aether".into(),
            ));
        }
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
        self.patch_agent(id, patch, false).await
    }

    /// Whether the daemon serves [`Self::update_agent_subtree`]. Without it a
    /// caller raises each live descendant with [`Self::update_agent`].
    pub fn serves_subtree_update(&self) -> bool {
        self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::SUBTREE_UPDATE)
        })
    }

    /// [`Self::update_agent`], and a patched deadline or cost budget also
    /// raises every live descendant whose own is lower. Returns the named
    /// agent. A daemon that does not announce `subtree_update` is refused
    /// before anything is sent.
    pub async fn update_agent_subtree(&self, id: AgentId, patch: AgentPatch) -> Result<Agent> {
        self.patch_agent(id, patch, true).await
    }

    async fn patch_agent(&self, id: AgentId, patch: AgentPatch, subtree: bool) -> Result<Agent> {
        match self
            .request(Command::UpdateAgent {
                id,
                patch,
                subtree,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agent(a) if a.id == id => Ok(a),
            Reply::Agent(_) => Err(ClientError::Malformed(
                "daemon returned another agent than the one updated".into(),
            )),
            other => Err(unexpected("agent", &other)),
        }
    }

    pub async fn send_message(&self, to: AgentId, content: impl Into<String>) -> Result<MessageId> {
        self.send_message_with_images(to, content, Vec::new()).await
    }

    /// Send a message with images attached; see [`crate::images`] for
    /// building them from files. The daemon refuses the whole message when
    /// it refuses any image.
    pub async fn send_message_with_images(
        &self,
        to: AgentId,
        content: impl Into<String>,
        attachments: Vec<ImageAttachment>,
    ) -> Result<MessageId> {
        match self
            .request(Command::SendMessage {
                to,
                content: content.into(),
                subtree: false,
                attachments,
                idempotency_key: None,
            })
            .await?
        {
            Reply::MessageSent { message } => Ok(message),
            other => Err(unexpected("message_sent", &other)),
        }
    }

    /// Append `note` to `agent`'s transcript without starting a turn.
    /// Returns the note's event sequence number. A daemon that does not
    /// announce `notes` is refused before anything is sent.
    pub async fn append_note(&self, agent: AgentId, note: aether_core::note::Note) -> Result<u64> {
        let aether_core::note::Note {
            text,
            links,
            context,
            level,
        } = note;
        let command = Command::AppendNote {
            agent,
            text,
            links,
            context,
            level,
        };
        match self.request(command).await? {
            Reply::NoteAppended { seq } => Ok(seq),
            other => Err(unexpected("note_appended", &other)),
        }
    }

    /// Fan a message out to `root` and every live agent beneath it. Returns
    /// one delivery per agent reached, in delivery order.
    pub async fn send_message_subtree(
        &self,
        root: AgentId,
        content: impl Into<String>,
    ) -> Result<Vec<MessageDelivery>> {
        self.send_message_subtree_with_images(root, content, Vec::new())
            .await
    }

    /// [`Self::send_message_subtree`] with images attached to every copy.
    pub async fn send_message_subtree_with_images(
        &self,
        root: AgentId,
        content: impl Into<String>,
        attachments: Vec<ImageAttachment>,
    ) -> Result<Vec<MessageDelivery>> {
        match self
            .request(Command::SendMessage {
                to: root,
                content: content.into(),
                subtree: true,
                attachments,
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
