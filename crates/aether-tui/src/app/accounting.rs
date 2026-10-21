//! Absolute live accounting for the board. Transcript separators use durable events only.
use aether_core::model::Usage;

use super::{AgentId, AgentTree, App};

#[derive(Debug, Clone, Copy)]
pub(super) struct AccountingProjection {
    total: Usage,
    pub(super) context_tokens: Option<u64>,
}

impl App {
    pub(super) fn project_accounting(
        &mut self,
        agent: AgentId,
        total: Usage,
        context_tokens: Option<u64>,
    ) {
        let reading = AccountingProjection {
            total,
            context_tokens,
        };
        self.live_accounting.insert(agent, reading);
        if let Some(tokens) = context_tokens {
            self.context_tokens.insert(agent, tokens);
        }
        project(&mut self.tree, agent, reading);
        self.rebuild_board();
        if self.selected == Some(agent) {
            self.dirty = true;
        } else {
            self.background_dirty = true;
        }
    }

    /// A fresh context reading for the inspector, footer and board row.
    pub(super) fn record_context(&mut self, agent: AgentId, tokens: u64) {
        self.context_tokens.insert(agent, tokens);
        if let Some(reading) = self.live_accounting.get_mut(&agent) {
            reading.context_tokens = Some(tokens);
        }
        if let Some(node) = self.tree.nodes.get_mut(&agent) {
            node.agent.stats.context_tokens = Some(tokens);
        }
        debug_assert_eq!(self.context_tokens.get(&agent), Some(&tokens));
        debug_assert!(
            self.live_accounting
                .get(&agent)
                .is_none_or(|reading| reading.context_tokens == Some(tokens))
        );
    }

    pub(super) fn restore_live_accounting(&mut self) {
        for (&agent, &reading) in &self.live_accounting {
            project(&mut self.tree, agent, reading);
        }
    }
}

fn project(tree: &mut AgentTree, agent: AgentId, reading: AccountingProjection) {
    let Some(node) = tree.nodes.get_mut(&agent) else {
        return;
    };
    node.agent.stats.usage = reading.total;
    if let Some(tokens) = reading.context_tokens {
        node.agent.stats.context_tokens = Some(tokens);
    }
    let mut next = Some(agent);
    // Rebuild ancestors from their children so coverage/provenance can also change.
    // The bound prevents a malformed lineage from looping forever.
    for _ in 0..tree.nodes.len() {
        let Some(id) = next else { break };
        let Some(node) = tree.nodes.get(&id) else {
            break;
        };
        let mut total = node.agent.stats.usage;
        if let Some(hidden) = node.hidden_usage {
            total += hidden;
        }
        for child in &node.children {
            if let Some(child) = tree.nodes.get(child) {
                total += child.subtree_usage;
            }
        }
        next = node.agent.parent;
        if let Some(node) = tree.nodes.get_mut(&id) {
            node.subtree_usage = total;
        }
    }
}
