//! Approvals and questions from the agents `aether run`'s root delegated to.
//! `--yes` answers theirs exactly as it answers the root's, and nobody else's.

use super::{sane, short};
use aether_client::{Client, ClientError};
use aether_core::event::{Event, EventKind};
use aether_core::id::{AgentId, ApprovalId, RunId};
use aether_protocol::ErrorCode;
use std::collections::HashMap;

/// Deeper than any tree the daemon lets an agent build; a longer parent
/// chain is a cycle or corruption, not a descendant.
const MAX_DEPTH: usize = 64;

/// Which agents of the run descend from its root, asked of the daemon once
/// per agent: an agent another client placed in the run is not the root's.
pub(super) struct Lineage {
    root: AgentId,
    run: RunId,
    known: HashMap<AgentId, bool>,
}

impl Lineage {
    pub(super) fn new(root: AgentId, run: RunId) -> Self {
        Self {
            root,
            run,
            known: HashMap::new(),
        }
    }

    async fn descends(&mut self, client: &Client, agent: AgentId) -> anyhow::Result<bool> {
        let mut path = Vec::new();
        let mut at = agent;
        let verdict = loop {
            if at == self.root {
                break true;
            }
            if let Some(&known) = self.known.get(&at) {
                break known;
            }
            anyhow::ensure!(
                path.len() < MAX_DEPTH,
                "agent {agent} has a parent chain deeper than {MAX_DEPTH}; run `aether doctor`"
            );
            path.push(at);
            let parent = match client.get_agent(at).await {
                Ok(found) if found.run == self.run => found.parent,
                Ok(_) => None,
                // A missing ancestor cannot be shown to be the root's: fail closed.
                Err(ClientError::Protocol(error)) if error.code == ErrorCode::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            match parent {
                Some(parent) => at = parent,
                None => break false,
            }
        };
        for visited in path {
            self.known.insert(visited, verdict);
        }
        Ok(verdict)
    }
}

/// Approve for `--yes`; false when the approval had already ended, as one
/// whose turn ended expires, so watching the run goes on. Other errors stop it.
pub(super) async fn auto_approve(client: &Client, approval: ApprovalId) -> anyhow::Result<bool> {
    match client
        .resolve_approval(
            approval,
            aether_core::tool::ApprovalDecision::Approved,
            Some("auto".into()),
        )
        .await
    {
        Ok(()) => Ok(true),
        Err(error) if super::super::pending_approvals::is_already_resolved(&error, approval) => {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Answer or describe an approval or question that another agent of the run
/// raised; the line to print, if any. Only the root's descendants are
/// approved under `yes`; others get the command that would answer them.
pub(super) async fn other_agent_ask(
    client: &Client,
    lineage: &mut Lineage,
    command: &str,
    yes: bool,
    event: &Event,
) -> anyhow::Result<Option<String>> {
    let Some(from) = event.agent_id else {
        return Ok(None);
    };
    if event.run != Some(lineage.run) || from == lineage.root {
        return Ok(None);
    }
    match &event.kind {
        EventKind::ApprovalRequested {
            approval,
            tool,
            description,
            ..
        } => {
            let ours = lineage.descends(client, from).await?;
            if yes && ours {
                let verb = if auto_approve(client, *approval).await? {
                    "approved"
                } else {
                    "already resolved, not approved:"
                };
                return Ok(Some(format!(
                    "{verb} {} for agent {from}: {}",
                    short(tool, 40),
                    short(description, 120)
                )));
            }
            let outside = if ours {
                ""
            } else {
                ", not started by this run"
            };
            Ok(Some(format!(
                "approval needed for {} (agent {from}{outside}): {}\n  run: {command} approve {approval}",
                short(tool, 40),
                short(description, 120)
            )))
        }
        EventKind::QuestionAsked { question } => Ok(Some(format!(
            "\n? {} (agent {from})\n  run: {command} send {from} \"...\"",
            sane(question)
        ))),
        _ => Ok(None),
    }
}
