//! Call-specific approval for an agent's recurring delegation.
use aether_core::agent::Agent;
use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::shell_network::gated_class;
use aether_core::tool::SideEffectClass;
use serde_json::Value;

use crate::{CommitHold, Result, Runtime, RuntimeError};

impl Runtime {
    pub(crate) fn tool_call_class(
        &self,
        agent: AgentId,
        name: &str,
        input: &Value,
        declared: SideEffectClass,
    ) -> Result<SideEffectClass> {
        let spawns = match name {
            "create_schedule" => input.get("spawn").is_some(),
            "update_schedule" if input["action"].as_str() == Some("resume") => {
                // Mode and owner cannot change. Admission rechecks under the write lock.
                match input["id"].as_str().and_then(|id| id.parse().ok()) {
                    Some(id) => self
                        .store
                        .schedule(id)?
                        .is_some_and(|job| job.spawns_children_of() == Some(agent)),
                    None => false,
                }
            }
            _ => false,
        };
        let class = if spawns {
            declared.max(SideEffectClass::Mutation)
        } else {
            gated_class(declared, input)
        };
        debug_assert!(class >= declared, "an override cannot lower the gate");
        Ok(class)
    }

    pub(crate) fn tool_needs_approval(
        &self,
        agent: &Agent,
        name: &str,
        declared: SideEffectClass,
        class: SideEffectClass,
    ) -> Result<bool> {
        if matches!(name, "create_schedule" | "update_schedule")
            && class == SideEffectClass::Mutation
        {
            return Ok(self
                .schedule_creator(agent.id)?
                .config
                .approval
                .requires_raised(declared, class));
        }
        Ok(agent.config.approval.requires_raised(declared, class))
    }

    fn schedule_creator(&self, id: AgentId) -> Result<Agent> {
        self.store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))
    }

    pub(super) fn check_spawn_approval(
        &self,
        _hold: &CommitHold<'_>,
        owner: &Actor,
        mutation_approved: bool,
    ) -> Result<()> {
        let Actor::Agent { id } = owner else {
            return Ok(());
        };
        if !mutation_approved
            && self
                .schedule_creator(*id)?
                .config
                .approval
                .requires(SideEffectClass::Mutation)
        {
            return Err(RuntimeError::Invalid(
                "a spawn schedule requires mutation approval; retry the tool call".into(),
            ));
        }
        Ok(())
    }
}
