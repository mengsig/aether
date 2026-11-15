//! Which children a task's agent may delegate to: model tiers and the
//! delegation policy a task can carry.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{Effort, ModelRef};

/// A capability band within one provider, weakest first. The vocabulary is
/// shared with Archon's catalog tiers: keep the names stable. `strongest` is
/// the legacy name of `strong` and is still accepted when reading.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Fast,
    Balanced,
    #[serde(alias = "strongest")]
    Strong,
    Frontier,
}

impl ModelTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Strong => "strong",
            Self::Frontier => "frontier",
        }
    }
}

impl fmt::Display for ModelTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Longest `reason` a delegation may record.
pub const MAX_DELEGATION_REASON_BYTES: usize = 2048;

/// Most worker entries one policy may list.
pub const MAX_POLICY_WORKERS: usize = 32;

/// One allowed kind of child: an exact model or a whole tier, optionally
/// capped in effort. Exactly one of `model` and `tier` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkerChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
    /// Highest effort for a child on this entry; absent means `max_effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

/// The children a task's agent may create. Every field is optional; an empty
/// policy bounds nothing beyond the rules for a task without one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegationPolicy {
    /// Allowed children. Empty: any model at or below `default_tier`, or
    /// without that, any model not above the delegating agent's tier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<WorkerChoice>,
    /// The tier a worker starts from when the lead has no better reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tier: Option<ModelTier>,
    /// Highest effort for any child the policy bounds (exempt roles are not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_effort: Option<Effort>,
    /// Most direct children unfinished at once, exempt ones not counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,
    /// Child roles the policy does not bound (list, effort, parallel limit),
    /// such as `reviewer`. Such a child still may not be above the parent's
    /// tier unless its model is in `exempt_models`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exempt_roles: Vec<String>,
    /// The models an exempt-role child may use regardless of tier: the
    /// orchestrator's own plan for those roles (Archon's `<stage>.review`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exempt_models: Vec<ModelRef>,
}

/// A child as delegation sees it: its model, that model's tier when ranked,
/// and its effort.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub role: &'a str,
    pub model: &'a ModelRef,
    pub tier: Option<ModelTier>,
    pub effort: Effort,
}

impl WorkerChoice {
    fn matches_model(&self, child: &Candidate<'_>) -> bool {
        match (&self.model, self.tier) {
            (Some(model), _) => model == child.model,
            (None, Some(tier)) => child.tier == Some(tier),
            (None, None) => false,
        }
    }
}

impl fmt::Display for WorkerChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.model, self.tier) {
            (Some(model), _) => write!(f, "{model}")?,
            (None, Some(tier)) => write!(f, "any {tier}-tier model")?,
            (None, None) => f.write_str("(invalid entry)")?,
        }
        if let Some(effort) = self.effort {
            write!(f, " at effort {effort} or lower")?;
        }
        Ok(())
    }
}

impl DelegationPolicy {
    /// Boundary validation for a policy a client sent.
    pub fn validate(&self) -> Result<(), String> {
        if self.workers.len() > MAX_POLICY_WORKERS {
            return Err(format!(
                "delegation policy lists {} workers; at most {MAX_POLICY_WORKERS}",
                self.workers.len()
            ));
        }
        for (index, worker) in self.workers.iter().enumerate() {
            if worker.model.is_some() == worker.tier.is_some() {
                return Err(format!(
                    "delegation policy workers[{index}] must name exactly one of `model` and `tier`"
                ));
            }
            if let Some(model) = &worker.model {
                check_model_ref(model, &format!("workers[{index}].model"))?;
            }
        }
        for (index, model) in self.exempt_models.iter().enumerate() {
            check_model_ref(model, &format!("exempt_models[{index}]"))?;
        }
        if self.exempt_roles.iter().any(|role| role.trim().is_empty()) {
            return Err("delegation policy exempt_roles has an empty role".into());
        }
        if self.max_parallel == Some(0) {
            return Err("delegation policy max_parallel must be at least 1".into());
        }
        Ok(())
    }

    /// Whether `child` may be created while `unfinished` direct children are
    /// still running; the refusal names what the policy allows.
    pub fn admits(
        &self,
        child: &Candidate<'_>,
        parent_tier: Option<ModelTier>,
        unfinished: usize,
    ) -> Result<(), String> {
        debug_assert!(self.validate().is_ok(), "a stored policy was validated");
        if self.exempts(child.role) {
            if self.exempt_models.contains(child.model) {
                return Ok(());
            }
            return admits_without_policy(child, parent_tier);
        }
        if let Some(refusal) = self
            .max_parallel
            .and_then(|limit| self.parallel_refusal(limit, unfinished))
        {
            return Err(refusal);
        }
        if let Some(max) = self.max_effort
            && child.effort > max
        {
            return Err(self.refusal(child, &format!("effort {} is above", child.effort)));
        }
        if self.workers.is_empty() {
            return self.admits_by_tier(child, parent_tier);
        }
        let matching: Vec<&WorkerChoice> = self
            .workers
            .iter()
            .filter(|worker| worker.matches_model(child))
            .collect();
        if matching.is_empty() {
            return Err(self.refusal(child, "the model is outside"));
        }
        let effort_fits = matching
            .iter()
            .any(|worker| worker.effort.is_none_or(|max| child.effort <= max));
        if !effort_fits {
            return Err(self.refusal(child, &format!("effort {} is above", child.effort)));
        }
        Ok(())
    }

    /// The rule for an empty `workers` list: no child above `default_tier`,
    /// or without one, above the parent's own tier.
    fn admits_by_tier(
        &self,
        child: &Candidate<'_>,
        parent_tier: Option<ModelTier>,
    ) -> Result<(), String> {
        debug_assert!(self.workers.is_empty());
        let Some(ceiling) = self.default_tier else {
            return admits_without_policy(child, parent_tier);
        };
        match child.tier {
            Some(tier) if tier <= ceiling => Ok(()),
            Some(tier) => Err(self.refusal(child, &format!("tier {tier} is above"))),
            None => Err(self.refusal(child, "an unranked model is outside")),
        }
    }

    /// The refusal when `unfinished` bounded children already fill `limit`.
    pub fn parallel_refusal(&self, limit: u32, unfinished: usize) -> Option<String> {
        debug_assert_eq!(self.max_parallel, Some(limit), "the policy's own limit");
        (unfinished >= limit as usize).then(|| {
            format!(
                "your delegation policy allows at most {limit} unfinished children at once and {unfinished} are; wait for one to finish"
            )
        })
    }

    /// Whether children with `role` are outside what the policy bounds.
    pub fn exempts(&self, role: &str) -> bool {
        self.exempt_roles.iter().any(|exempt| exempt == role)
    }

    /// Whether the policy names `model` outright, for this child's role:
    /// an explicit choice that needs no `reason`.
    pub fn names_model(&self, role: &str, model: &ModelRef) -> bool {
        if self.exempts(role) {
            return self.exempt_models.contains(model);
        }
        self.workers
            .iter()
            .any(|worker| worker.model.as_ref() == Some(model))
    }

    fn refusal(&self, child: &Candidate<'_>, what: &str) -> String {
        assert!(!what.is_empty(), "a refusal says what failed");
        format!(
            "delegation refused: {} at effort {}: {what} your delegation policy. Allowed: {}",
            child.model,
            child.effort,
            self.describe()
        )
    }

    /// One line naming every allowed choice, for refusals and prompts.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.workers.is_empty() {
            match self.default_tier {
                Some(tier) => parts.push(format!("any model at tier {tier} or below")),
                None => parts.push("any model not above your own tier".to_owned()),
            }
        } else {
            let workers: Vec<String> = self.workers.iter().map(ToString::to_string).collect();
            parts.push(format!("workers {}", workers.join("; ")));
        }
        if let Some(tier) = self.default_tier {
            parts.push(format!("default tier {tier}"));
        }
        if let Some(effort) = self.max_effort {
            parts.push(format!("max effort {effort}"));
        }
        if let Some(limit) = self.max_parallel {
            parts.push(format!("at most {limit} unfinished at once"));
        }
        if !self.exempt_roles.is_empty() {
            parts.push(self.describe_exemptions());
        }
        debug_assert!(!parts.is_empty());
        parts.join(", ")
    }

    fn describe_exemptions(&self) -> String {
        debug_assert!(!self.exempt_roles.is_empty());
        let roles = self.exempt_roles.join(", ");
        if self.exempt_models.is_empty() {
            return format!("roles {roles} unbounded by this policy but not above your tier");
        }
        let models: Vec<String> = self.exempt_models.iter().map(ToString::to_string).collect();
        format!(
            "roles {roles} unbounded by this policy: any of {} at any tier, other models not above your tier",
            models.join(", ")
        )
    }
}

fn check_model_ref(model: &ModelRef, field: &str) -> Result<(), String> {
    if model.provider.trim().is_empty() || model.model.trim().is_empty() {
        return Err(format!(
            "delegation policy {field} needs a provider and a model"
        ));
    }
    Ok(())
}

/// The rule without a policy: a child may not be above its parent's tier.
/// Either tier unknown leaves nothing to compare, so it passes.
pub fn admits_without_policy(
    child: &Candidate<'_>,
    parent_tier: Option<ModelTier>,
) -> Result<(), String> {
    match (child.tier, parent_tier) {
        (Some(tier), Some(parent)) if tier > parent => Err(format!(
            "delegation refused: {} is a {tier}-tier model, above your own {parent} tier; choose a model at tier {parent} or below",
            child.model
        )),
        _ => Ok(()),
    }
}

/// A child on its parent's own frontier-tier model must say why: the lead's
/// model is a deliberate choice for a worker, never the default.
pub fn needs_reason(child: &Candidate<'_>, parent_model: &ModelRef) -> bool {
    child.model == parent_model && child.tier == Some(ModelTier::Frontier)
}

/// Trim and bound a caller's `reason`; an empty one counts as none.
pub fn normalize_reason(reason: Option<&str>) -> Result<Option<String>, String> {
    let Some(reason) = reason.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    if reason.len() > MAX_DELEGATION_REASON_BYTES {
        return Err(format!(
            "reason is {} bytes; at most {MAX_DELEGATION_REASON_BYTES}",
            reason.len()
        ));
    }
    Ok(Some(reason.to_owned()))
}

#[cfg(test)]
mod tests;
