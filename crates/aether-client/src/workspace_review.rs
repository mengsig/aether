use aether_core::id::AgentId;
use aether_core::workspace_review::{
    PATCH_LIMIT, WorkspaceDiff, WorkspaceDiscard, WorkspaceIntegration, truncation_marker,
};
use aether_protocol::{Command, Reply};

use crate::{Client, ClientError, Result, unexpected};

fn about(expected: AgentId, actual: AgentId) -> Result<()> {
    if expected != actual {
        return Err(ClientError::Malformed(format!(
            "daemon answered for agent {actual} instead of {expected}"
        )));
    }
    Ok(())
}

impl Client {
    pub async fn workspace_diff(&self, agent: AgentId) -> Result<WorkspaceDiff> {
        let diff = match self.request(Command::WorkspaceDiff { agent }).await? {
            Reply::WorkspaceDiff(diff) => *diff,
            other => return Err(unexpected("workspace diff", &other)),
        };
        about(agent, diff.agent)?;
        let bound = PATCH_LIMIT + truncation_marker(PATCH_LIMIT, diff.patch_bytes).len();
        if diff.patch.len() > bound {
            return Err(ClientError::Malformed(format!(
                "daemon sent a {}-byte patch, over the {bound}-byte bound",
                diff.patch.len()
            )));
        }
        Ok(diff)
    }

    pub async fn workspace_integrate(&self, agent: AgentId) -> Result<WorkspaceIntegration> {
        let integration = match self.request(Command::WorkspaceIntegrate { agent }).await? {
            Reply::WorkspaceIntegration(integration) => integration,
            other => return Err(unexpected("workspace integration", &other)),
        };
        about(agent, integration.agent)?;
        if integration.integrated == !integration.conflicts.is_empty() {
            return Err(ClientError::Malformed(
                "daemon reported an integration that both landed and conflicted, or neither".into(),
            ));
        }
        Ok(integration)
    }

    pub async fn workspace_discard(&self, agent: AgentId, force: bool) -> Result<WorkspaceDiscard> {
        let discard = match self
            .request(Command::WorkspaceDiscard { agent, force })
            .await?
        {
            Reply::WorkspaceDiscarded(discard) => discard,
            other => return Err(unexpected("workspace discard", &other)),
        };
        about(agent, discard.agent)?;
        if discard.forced && !force {
            return Err(ClientError::Malformed(
                "daemon forced a discard that was not asked to be forced".into(),
            ));
        }
        Ok(discard)
    }
}
