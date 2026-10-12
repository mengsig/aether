//! `get_attention`: everything waiting on the operator, read in one place.

use aether_core::agent::{Agent, AgentStatus, AgentStatusKind};
use aether_core::event::EventKind;
use aether_protocol::remote::{AgentAttention, Attention, AttentionKind, PendingQuestion};
use aether_protocol::{ErrorReply, Reply};
use aether_runtime::{RuntimeError, pending_question};
use aether_store::{AgentFilter, Store};

use crate::Daemon;
use crate::connection::map_err;

/// Items in each list. A list over this keeps its oldest approvals and
/// questions, which block work, and its newest failed or blocked agents.
const MAX_ITEMS: usize = 200;
/// Characters of an agent's question or failure kept in the reply.
const MAX_TEXT_CHARS: usize = 4000;
/// Characters of the one-line reason for a failed or blocked agent.
const MAX_REASON_CHARS: usize = 300;

pub(super) async fn get_attention(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let runtime = daemon.runtime.clone();
    let read = tokio::task::spawn_blocking(move || runtime.read_at_rest(read_attention))
        .await
        .map_err(|e| ErrorReply::internal(format!("the attention read did not finish: {e}")))?;
    Ok(Reply::Attention(read.map_err(map_err)?))
}

/// Statuses that can hold something for the operator: every live one, which
/// may have a question pending, and failed.
fn candidate_statuses() -> Vec<AgentStatusKind> {
    AgentStatusKind::ALL
        .into_iter()
        .filter(|kind| !kind.is_terminal() || *kind == AgentStatusKind::Failed)
        .collect()
}

fn read_attention(store: &dyn Store, last_seq: u64) -> Result<Attention, RuntimeError> {
    let mut approvals = store.pending_approvals()?;
    approvals.truncate(MAX_ITEMS);
    let agents = store.agents(&AgentFilter {
        statuses: candidate_statuses(),
        exclude_archived: true,
        ..Default::default()
    })?;
    let questions = pending_questions(store, &agents)?;
    let agents = agents_needing_attention(&agents);
    assert!(
        approvals.len() <= MAX_ITEMS && questions.len() <= MAX_ITEMS && agents.len() <= MAX_ITEMS,
        "every list is bounded"
    );
    Ok(Attention {
        last_seq,
        approvals,
        questions,
        agents,
    })
}

/// The latest unanswered question of each live agent, oldest first.
fn pending_questions(
    store: &dyn Store,
    agents: &[Agent],
) -> Result<Vec<PendingQuestion>, RuntimeError> {
    let mut questions = Vec::new();
    for agent in agents.iter().filter(|agent| agent.is_live()) {
        let Some(asked) = pending_question(store, agent.id)? else {
            continue;
        };
        let EventKind::QuestionAsked { question } = &asked.kind else {
            continue;
        };
        questions.push(PendingQuestion {
            agent: agent.id,
            agent_name: named(agent),
            question_seq: asked.seq,
            question: truncated(question, MAX_TEXT_CHARS),
            asked_at: asked.at,
        });
    }
    questions.sort_by_key(|question| question.question_seq);
    questions.truncate(MAX_ITEMS);
    assert!(
        questions
            .windows(2)
            .all(|pair| pair[0].agent != pair[1].agent),
        "an agent has at most one pending question"
    );
    Ok(questions)
}

/// Failed agents nobody has restarted, and agents blocked on something only
/// the operator can clear, oldest first.
fn agents_needing_attention(agents: &[Agent]) -> Vec<AgentAttention> {
    let mut needing: Vec<AgentAttention> = agents
        .iter()
        .filter(|agent| !agent.archived)
        .filter_map(|agent| {
            let (kind, reason) = match &agent.status {
                AgentStatus::Failed { error } if agent.restarted_as.is_none() => {
                    (AttentionKind::AgentFailed, one_line(error))
                }
                AgentStatus::Blocked { reason } if reason.needs_operator() => {
                    (AttentionKind::AgentBlocked, one_line(&reason.describe()))
                }
                _ => return None,
            };
            Some(AgentAttention {
                agent: agent.id,
                agent_name: named(agent),
                kind,
                reason,
                since: agent.status_since.unwrap_or(agent.updated_at),
            })
        })
        .collect();
    needing.sort_by(|a, b| a.since.cmp(&b.since).then(a.agent.cmp(&b.agent)));
    let excess = needing.len().saturating_sub(MAX_ITEMS);
    needing.drain(..excess);
    needing
}

fn named(agent: &Agent) -> Option<String> {
    Some(agent.name.clone()).filter(|name| !name.is_empty())
}

fn truncated(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// The first non-empty line of `text`, control characters removed.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    truncated(&clean, MAX_REASON_CHARS)
}

#[cfg(test)]
mod tests;
