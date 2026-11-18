//! Spawn templates an agent creates: each run gets no more than `delegate`
//! would give a child of that agent, rechecked at every occurrence.

use aether_core::agent::{Agent, AgentConfig};
use aether_core::id::AgentId;
use aether_core::schedule::SpawnSpec;

use crate::delegation_admission::{ChildChoice, Refusal};
use crate::{Result, Runtime, RuntimeError};

fn refuse<T>(why: impl std::fmt::Display) -> Result<T> {
    Err(RuntimeError::Invalid(format!(
        "spawn schedule refused: {why}"
    )))
}

impl Runtime {
    /// `template` bounded by its creator as it is now: model and effort by
    /// its delegation policy, scope and approval no wider than its own.
    pub(super) fn bounded_agent_template(
        &self,
        mut template: SpawnSpec,
        creator: AgentId,
    ) -> Result<SpawnSpec> {
        let creator = self.schedule_creator(creator)?;
        let Some(model) = template.model.clone() else {
            return refuse("an agent's spawn template must name its model");
        };
        let Some(asked) = template.config.take() else {
            return refuse("an agent's spawn template must set an effort");
        };
        let Some(effort) = asked.effort else {
            return refuse("an agent's spawn template must set an effort");
        };
        if !template.fallbacks.is_empty() {
            return refuse("an agent's spawn template cannot name fallback models");
        }
        let choice = ChildChoice {
            role: &template.role,
            model: &model,
            effort,
            parallel: false,
            reason_given: false,
            exemptions: false,
        };
        match self.delegation_verdict(&creator, choice)? {
            Ok(()) => {}
            Err(Refusal::Policy(why)) => return refuse(why),
            Err(Refusal::FrontierClone(model)) => {
                return refuse(format!(
                    "{model} is your own frontier-tier model and your delegation policy \
                     does not name it; schedule a balanced or fast model"
                ));
            }
        }
        let scope = template
            .scope
            .take()
            .unwrap_or_else(|| creator.scope.clone());
        if let Some(field) = self.widened_field_resolved(&scope, &creator.scope)? {
            return refuse(format!("the template's scope widens your `{field}`"));
        }
        let approval = template
            .approval
            .take()
            .unwrap_or_else(|| creator.config.approval.clone());
        if !approval.is_narrowing_of(&creator.config.approval) {
            return refuse("the template's approval policy drops a gate you keep");
        }
        template.scope = Some(scope);
        template.approval = Some(approval);
        template.config = Some(bounded_config(&creator.config, asked)?);
        Ok(template)
    }

    /// The live, non-leaf agent whose authority an agent's spawn schedule
    /// runs under: a finished creator's schedules fail rather than outlive it.
    fn schedule_creator(&self, id: AgentId) -> Result<Agent> {
        let creator = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if creator.status.is_terminal() {
            return refuse(format!("its creator {id} is {}", creator.status.kind()));
        }
        if creator.config.leaf {
            return refuse("leaf agents cannot start other agents");
        }
        Ok(creator)
    }
}

/// The run's configuration as `delegate` derives a child's: the creator's,
/// with the choices a delegating agent may make and its budgets as ceilings.
fn bounded_config(creator: &AgentConfig, asked: AgentConfig) -> Result<AgentConfig> {
    if asked.delegation_policy.is_some() {
        return refuse(
            "an agent's spawn template cannot carry a delegation policy; its run is bounded \
             by its model's tier",
        );
    }
    debug_assert!(asked.effort.is_some(), "checked by the caller");
    let mut config = creator.clone();
    config.instructions = asked.instructions;
    config.effort = asked.effort;
    config.leaf = asked.leaf;
    config.max_turns = asked.max_turns;
    config.host_context = asked.host_context;
    config.long_lived = false;
    config.delegation_policy = None;
    config.cost_budget = narrower(asked.cost_budget, creator.cost_budget);
    config.token_budget = narrower(asked.token_budget, creator.token_budget);
    config.max_output_tokens = narrower(asked.max_output_tokens, creator.max_output_tokens);
    // A run must not outlive its creator's deadline; it is absolute, so it
    // stops every later run once it passes.
    config.deadline = creator.deadline;
    debug_assert!(
        creator
            .cost_budget
            .is_none_or(|limit| config.cost_budget.is_some_and(|run| run <= limit))
    );
    Ok(config)
}

fn narrower<T: PartialOrd + Copy>(asked: Option<T>, creator: Option<T>) -> Option<T> {
    match (asked, creator) {
        (Some(asked), Some(creator)) if asked <= creator => Some(asked),
        (_, Some(creator)) => Some(creator),
        (asked, None) => asked,
    }
}
