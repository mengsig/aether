//! Which model and effort an agent may start a run on: one rule for the
//! children `delegate` creates and the runs an agent's spawn schedule starts.

use aether_core::agent::Agent;
use aether_core::delegation::{Candidate, DelegationPolicy, admits_without_policy, needs_reason};
use aether_core::model::{Effort, ModelRef};

use crate::{Result, Runtime};

/// The run an agent asks to start.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChildChoice<'a> {
    pub role: &'a str,
    pub model: &'a ModelRef,
    pub effort: Effort,
    /// Count the parent's unfinished children against `max_parallel`.
    pub parallel: bool,
    /// The caller gave a `reason`, which admits the parent's frontier model.
    pub reason_given: bool,
    /// Honour the policy's `exempt_roles`; unattended runs never are exempt.
    pub exemptions: bool,
}

/// Why the parent may not start the run it asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The parent's policy or tier ceiling; the text names what is allowed.
    Policy(String),
    /// The parent's own frontier-tier model, with no reason and not named
    /// by its policy.
    FrontierClone(ModelRef),
}

impl Runtime {
    /// Whether `parent` may start `child`: its delegation policy, or without
    /// one the tier ceiling and the frontier-clone rule.
    pub(crate) fn delegation_verdict(
        &self,
        parent: &Agent,
        child: ChildChoice<'_>,
    ) -> Result<std::result::Result<(), Refusal>> {
        let role = child.role.trim();
        debug_assert!(!role.is_empty(), "a delegated run has a role");
        let models = &self.models;
        let model = models.canonical(child.model);
        let parent_model = models.canonical(&parent.model);
        let parent_tier = models.tier(&parent_model);
        let candidate = Candidate {
            role,
            model: &model,
            tier: models.tier(&model),
            effort: child.effort,
        };
        let policy = parent
            .config
            .delegation_policy
            .as_ref()
            .map(|policy| self.canonical_policy(policy, child.exemptions));
        let verdict = match &policy {
            Some(policy) => {
                let unfinished = match policy.max_parallel {
                    Some(_) if child.parallel => self.unfinished_children(parent.id, policy)?,
                    _ => 0,
                };
                policy.admits(&candidate, parent_tier, unfinished)
            }
            None => admits_without_policy(&candidate, parent_tier),
        };
        if let Err(refusal) = verdict {
            return Ok(Err(Refusal::Policy(refusal)));
        }
        let named_by_policy = policy
            .as_ref()
            .is_some_and(|policy| policy.names_model(role, &model));
        if needs_reason(&candidate, &parent_model) && !named_by_policy && !child.reason_given {
            return Ok(Err(Refusal::FrontierClone(model)));
        }
        Ok(Ok(()))
    }

    /// The policy with each named model in canonical form, so an alias
    /// matches, and without its exemptions unless they apply.
    fn canonical_policy(&self, policy: &DelegationPolicy, exemptions: bool) -> DelegationPolicy {
        let mut canonical = policy.clone();
        if !exemptions {
            canonical.exempt_roles.clear();
            canonical.exempt_models.clear();
        }
        for worker in &mut canonical.workers {
            if let Some(model) = &worker.model {
                worker.model = Some(self.models.canonical(model));
            }
        }
        for model in &mut canonical.exempt_models {
            *model = self.models.canonical(model);
        }
        debug_assert_eq!(canonical.workers.len(), policy.workers.len());
        canonical
    }
}
