//! Spawn templates an agent creates: each run gets no more than `delegate`
//! would give a child of that agent, rechecked at every occurrence.

use std::path::Path;

use aether_core::agent::{Agent, AgentConfig, Scope};
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
        self.check_unbudgeted_line(&creator)?;
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
        // Compared as resolved on disk, so a link inside your scope cannot
        // reach outside it; the resolved paths are what the run gets.
        let creator_scope = canonical_scope(&creator.scope)?;
        let scope = match template.scope.take() {
            Some(asked) => canonical_scope(&asked)?,
            None => creator_scope.clone(),
        };
        if let Some(field) = self.widened_field_resolved(&scope, &creator_scope)? {
            return refuse(format!("the template's scope widens your `{field}`"));
        }
        let approval = template
            .approval
            .take()
            .unwrap_or_else(|| creator.config.approval.clone());
        if !approval.is_narrowing_of(&creator.config.approval) {
            return refuse("the template's approval policy drops a gate you keep");
        }
        // Creation strips the gates of a run whose engine cannot ask first.
        if !self.config.yolo && !approval.is_yolo() && !self.engine_can_gate(&model)? {
            return refuse(format!(
                "{model} runs on an engine that cannot ask before acting, so its run would \
                 drop your approval gates"
            ));
        }
        template.scope = Some(scope);
        template.approval = Some(approval);
        template.config = Some(bounded_config(&creator.config, asked)?);
        Ok(template)
    }

    /// The live, non-leaf agent whose authority an agent's spawn schedule
    /// runs under: a finished creator's schedules fail rather than outlive it.
    pub(super) fn schedule_creator(&self, id: AgentId) -> Result<Agent> {
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

    /// A scheduled run is a root run, outside the subtree whose spend a
    /// budget sums; so no agent on the creator's line may carry one.
    fn check_unbudgeted_line(&self, creator: &Agent) -> Result<()> {
        let mut cursor = Some(creator.clone());
        let mut seen = std::collections::HashSet::new();
        while let Some(agent) = cursor {
            if !seen.insert(agent.id) {
                return Err(RuntimeError::Invalid("cycle in agent ancestry".into()));
            }
            if agent.config.cost_budget.is_some() || agent.config.token_budget.is_some() {
                return refuse(format!(
                    "{} has a cost or token budget, and a scheduled run is its own root run \
                     outside that budget; delegate the check instead",
                    agent.id
                ));
            }
            cursor = match agent.parent {
                Some(parent) => Some(
                    self.store
                        .agent(parent)?
                        .ok_or_else(|| RuntimeError::NotFound(parent.to_string()))?,
                ),
                None => None,
            };
        }
        Ok(())
    }
}

/// `scope` with its workspace root and allowed paths resolved on disk.
fn canonical_scope(scope: &Scope) -> Result<Scope> {
    let resolve = |path: &Path| {
        std::fs::canonicalize(path).or_else(|error| {
            refuse(format!(
                "scope path {} cannot be resolved: {error}",
                path.display()
            ))
        })
    };
    let mut resolved = scope.clone();
    if let Some(root) = &scope.workspace_root {
        resolved.workspace_root = Some(resolve(root)?);
    }
    resolved.allowed_paths = scope
        .allowed_paths
        .iter()
        .map(|path| resolve(path))
        .collect::<Result<_>>()?;
    debug_assert_eq!(resolved.allowed_paths.len(), scope.allowed_paths.len());
    Ok(resolved)
}

/// The run's configuration as `delegate` derives a child's: the creator's,
/// with the choices a delegating agent may make.
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
    debug_assert!(creator.cost_budget.is_none() && creator.token_budget.is_none());
    config.cost_budget = asked.cost_budget;
    config.token_budget = asked.token_budget;
    config.max_output_tokens = narrower(asked.max_output_tokens, creator.max_output_tokens);
    // A run must not outlive its creator's deadline; it is absolute, so it
    // stops every later run once it passes.
    config.deadline = creator.deadline;
    Ok(config)
}

fn narrower<T: PartialOrd + Copy>(asked: Option<T>, creator: Option<T>) -> Option<T> {
    match (asked, creator) {
        (Some(asked), Some(creator)) if asked <= creator => Some(asked),
        (_, Some(creator)) => Some(creator),
        (asked, None) => asked,
    }
}
