//! What `delegate` decides about a child before it exists. The tool and an
//! agent's spawn schedule both ask here, so neither admits what the other
//! refuses.

use aether_core::agent::{Agent, HostContext, NetworkPolicy, Scope};
use aether_core::event::Actor;
use aether_core::model::{Effort, ModelRef};
use aether_engine::EngineKind;

use crate::commands::{Preflight, PreparedAgent};
use crate::engine_access::{DetectedEngines, unavailable_text};
use crate::{CreateAgentSpec, Result, Runtime, RuntimeError};

/// The child one `delegate` call asks for.
pub(crate) struct ChildRequest {
    pub role: String,
    pub name: Option<String>,
    pub objective: String,
    pub instructions: Option<String>,
    pub model: ModelRef,
    pub effort: Effort,
    /// Absent: the parent's scope as it is when the child is created.
    pub scope: Option<Scope>,
    pub leaf: bool,
    /// Absent: the parent's.
    pub host_context: Option<HostContext>,
    pub reason: ChildReason,
}

/// Why a child may run on its parent's own frontier-tier model.
pub(crate) enum ChildReason {
    Given(String),
    Omitted,
    /// A spawn schedule's template has nowhere to carry one.
    Unsupported,
}

impl ChildReason {
    pub(crate) fn from_tool(reason: Option<String>) -> Self {
        reason.map_or(Self::Omitted, Self::Given)
    }

    fn given(self) -> Option<String> {
        match self {
            Self::Given(reason) => Some(reason),
            Self::Omitted | Self::Unsupported => None,
        }
    }

    /// What the caller can do about a refused frontier clone.
    fn remedy(&self) -> &'static str {
        match self {
            Self::Given(_) | Self::Omitted => {
                "choose one, or pass `reason` saying why this child needs your model. \
                 The reason is recorded on the child's task."
            }
            Self::Unsupported => {
                "choose one: a spawn schedule cannot carry the `reason` such a child needs, \
                 so delegate it yourself when it is due."
            }
        }
    }
}

/// Whether the verdict before the hold counts `max_parallel`. The hold
/// always does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slots {
    /// As `delegate` answers: a full `max_parallel` refuses before anything
    /// is built.
    Counted,
    /// The caller waits for a slot instead, so a count that moves between its
    /// own read and this one must not read as a refusal.
    LeftToCaller,
}

impl Preflight<'_> {
    /// What every delegated child is held to under its creation hold.
    pub(crate) fn delegated() -> Preflight<'static> {
        Preflight {
            run_budget: true,
            inherit_limits: true,
            parallel_limit: true,
            ..Preflight::default()
        }
    }
}

/// Why a child cannot run on its model's engine with the network it gets.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EngineRefusal {
    /// The CLI is missing or failed its probe; a later probe may find it.
    Unavailable { kind: EngineKind, why: String },
    /// The engine cannot run under the child's network policy.
    Network(String),
}

impl std::fmt::Display for EngineRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Unavailable { why, .. } | Self::Network(why)) = self;
        f.write_str(why)
    }
}

/// `kind` as unusable for now, with the reason `detected` has for it.
pub(crate) fn engine_unavailable(
    detected: &DetectedEngines,
    kind: EngineKind,
    provider: &str,
) -> EngineRefusal {
    let why = format!(
        "its engine is not available in this daemon: {}",
        unavailable_text(kind, provider, detected.reason(&kind))
    );
    EngineRefusal::Unavailable { kind, why }
}

/// What stops `kind` from running a child of `provider` under `network`, by
/// what `detected` holds. `delegate` asks it of a fresh probe.
pub(crate) fn engine_refusal(
    detected: &DetectedEngines,
    kind: EngineKind,
    provider: &str,
    network: NetworkPolicy,
) -> Option<EngineRefusal> {
    let Some(engine) = detected.get(&kind) else {
        return Some(engine_unavailable(detected, kind, provider));
    };
    engine
        .validate_network_policy(network == NetworkPolicy::Allowed)
        .err()
        .map(|error| EngineRefusal::Network(error.to_string()))
}

/// Why `parent` may not have this child: its delegation policy, or without
/// one the tier ceiling, then the frontier-clone rule. `None` admits it.
pub(crate) fn policy_refusal(
    runtime: &Runtime,
    parent: &Agent,
    request: &ChildRequest,
    slots: Slots,
) -> Result<Option<String>> {
    use aether_core::delegation::{Candidate, admits_without_policy, needs_reason};
    let models = &runtime.models;
    let model = models.canonical(&request.model);
    let parent_model = models.canonical(&parent.model);
    let parent_tier = models.delegation_tier(&parent_model);
    let role = request.role.trim();
    let child = Candidate {
        role,
        model: &model,
        tier: models.delegation_tier(&model),
        effort: request.effort,
    };
    let policy = parent
        .config
        .delegation_policy
        .as_ref()
        .map(|policy| canonical_policy(runtime, policy));
    let verdict = match &policy {
        Some(policy) => {
            let unfinished = match policy.max_parallel {
                Some(_) if slots == Slots::Counted => {
                    runtime.unfinished_children(parent.id, policy)?
                }
                _ => 0,
            };
            policy.admits(&child, parent_tier, unfinished)
        }
        None => admits_without_policy(&child, parent_tier),
    };
    if let Err(refusal) = verdict {
        return Ok(Some(refusal));
    }
    let named_by_policy = policy
        .as_ref()
        .is_some_and(|policy| policy.names_model(role, &model));
    let has_reason = matches!(request.reason, ChildReason::Given(_));
    if needs_reason(&child, &parent_model) && !named_by_policy && !has_reason {
        return Ok(Some(format!(
            "delegation refused: {model} is your own frontier-tier model. Workers default to the \
             balanced or fast tier (list_models shows each model's tier); {}",
            request.reason.remedy()
        )));
    }
    Ok(None)
}

/// The policy with each named model in canonical form, so an alias matches.
fn canonical_policy(
    runtime: &Runtime,
    policy: &aether_core::delegation::DelegationPolicy,
) -> aether_core::delegation::DelegationPolicy {
    let mut canonical = policy.clone();
    for worker in &mut canonical.workers {
        if let Some(model) = &worker.model {
            worker.model = Some(runtime.models.canonical(model));
        }
    }
    for model in &mut canonical.exempt_models {
        *model = runtime.models.canonical(model);
    }
    debug_assert_eq!(canonical.workers.len(), policy.workers.len());
    canonical
}

/// The creation request for `parent`'s child: the parent's configuration,
/// less what belongs to the parent alone, with the request's own choices.
pub(crate) fn child_spec(parent: &Agent, request: ChildRequest) -> CreateAgentSpec {
    let mut config = parent.config.clone();
    config.deadline = None;
    config.cost_budget = None;
    // The parent's policy bounds the parent's children, not the child's.
    config.delegation_policy = None;
    // Chosen for the parent's model and context, not for the child's.
    config.compaction_tokens = None;
    // None at create_agent means inherit. Delegation instructions belong to
    // this task: an inherited "spawn a child" would repeat down the tree.
    config.instructions = Some(request.instructions.unwrap_or_default());
    config.effort = Some(request.effort);
    config.long_lived = false;
    config.leaf = request.leaf;
    config.max_turns = None;
    if let Some(host_context) = request.host_context {
        config.host_context = host_context;
    }
    CreateAgentSpec {
        subsystem: None,
        role: request.role,
        name: request.name,
        objective: Some(request.objective),
        model: Some(request.model),
        // Delegation always names its model; do not inherit the parent's chain.
        fallbacks: Vec::new(),
        scope: request.scope,
        // A delegated child inherits its parent's repository.
        repository_root: None,
        parent: Some(parent.id),
        config: Some(config),
        // Inherited from the parent by the runtime; a delegating agent does
        // not get to choose its child's gates.
        approval: None,
        run: Some(parent.run),
        start: true,
        delegation_reason: request.reason.given(),
        references: Vec::new(),
        inherited_references: Vec::new(),
        reference_scope: None,
    }
}

/// Everything `delegate` decides before the hold, for a child that shares its
/// parent's workspace: the policy's verdict, then the agent, unwritten. The
/// caller admits it with `Runtime::admit_prepared` under its own hold.
pub(crate) fn prepare_child(
    runtime: &Runtime,
    parent: &Agent,
    request: ChildRequest,
    by: &Actor,
    slots: Slots,
) -> Result<PreparedAgent> {
    if let Some(refusal) = policy_refusal(runtime, parent, &request, slots)? {
        return Err(RuntimeError::Invalid(refusal));
    }
    let model = request.model.clone();
    let network = request
        .scope
        .as_ref()
        .map_or(parent.scope.network, |scope| scope.network);
    let prepared = runtime.prepare_agent(
        child_spec(parent, request),
        by,
        Preflight {
            checked: Some((&model, network)),
            ..Preflight::delegated()
        },
    )?;
    debug_assert_eq!(prepared.agent.parent, Some(parent.id));
    debug_assert_eq!(prepared.agent.run, parent.run, "a child joins its run");
    Ok(prepared)
}
