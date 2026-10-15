//! Application memories.
use super::{AgentId, App, Command, Effect, MemoryQuerySpec, Then, commands};

impl App {
    pub(super) fn inspection_memory_query(provenance_agent: Option<AgentId>) -> MemoryQuerySpec {
        MemoryQuerySpec {
            provenance_agent,
            recorded_by: None,
            scope: None,
            kinds: Vec::new(),
            tags: Vec::new(),
            text: None,
            include_superseded: true,
            include_forgotten: true,
            proposed: false,
            id: None,
            limit: 200,
        }
    }

    pub(super) fn create_operator_memory(
        &mut self,
        text: String,
        scope: commands::RememberScope,
    ) -> Vec<Effect> {
        use aether_core::memory::{MemoryKind, OperatorMemoryScope, OperatorMemorySpec};
        let scope = match scope {
            commands::RememberScope::Agent => {
                let Some(id) = self.need_agent() else {
                    return Vec::new();
                };
                OperatorMemoryScope::Agent { id }
            }
            commands::RememberScope::Global => OperatorMemoryScope::Global,
            commands::RememberScope::Repository => {
                let root = self
                    .selected_agent()
                    .and_then(|agent| agent.scope.workspace_root.clone());
                if self.selected.is_some() && root.is_none() {
                    self.flash("selected agent has no workspace; use /remember --agent or --global explicitly");
                    return Vec::new();
                }
                OperatorMemoryScope::Repository {
                    root,
                    repository: None,
                }
            }
        };
        vec![Effect::Request {
            command: Command::CreateMemory {
                spec: OperatorMemorySpec {
                    text,
                    kind: MemoryKind::Fact,
                    scope,
                    importance: None,
                    confidence: None,
                    tags: Vec::new(),
                    supersedes: None,
                    proposed: false,
                },
                idempotency_key: None,
            },
            then: Then::MemoryCreated {
                selected: self.selected,
            },
        }]
    }

    pub(super) fn memories_request(&self) -> Effect {
        Effect::Request {
            command: Command::Memories {
                query: Self::inspection_memory_query(None),
            },
            then: Then::Memories,
        }
    }
}
