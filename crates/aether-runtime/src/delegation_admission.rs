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
    pub fallbacks: Vec<ModelRef>,
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
    pub(crate) fn delegated(parent: &Agent) -> Preflight<'_> {
        Preflight {
            run_budget: true,
            inherit_limits: true,
            parallel_limit: true,
            delegating_parent: Some(parent),
            ..Preflight::default()
        }
    }
}

impl ChildRequest {
    pub(crate) fn network(&self, parent: &Agent) -> NetworkPolicy {
        self.scope
            .as_ref()
            .map_or(parent.scope.network, |scope| scope.network)
    }
}

/// Why a child cannot run on its model's engine under its inherited policy.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EngineRefusal {
    /// The CLI is missing or failed its probe; a later probe may find it.
    Unavailable { kind: EngineKind, why: String },
    /// The engine cannot honor a policy the child must keep.
    Policy(String),
}

impl std::fmt::Display for EngineRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Unavailable { why, .. } | Self::Policy(why)) = self;
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

/// What stops `kind` from running a child of `provider` under its policy, by
/// what `detected` holds. `delegate` asks it of a fresh probe.
pub(crate) fn engine_refusal(
    detected: &DetectedEngines,
    kind: EngineKind,
    provider: &str,
    network: NetworkPolicy,
    approvals_required: bool,
) -> Option<EngineRefusal> {
    let Some(engine) = detected.get(&kind) else {
        return Some(engine_unavailable(detected, kind, provider));
    };
    if approvals_required && !engine.gates_permissions() {
        return Some(EngineRefusal::Policy(
            "its engine cannot enforce the creator's required approvals".into(),
        ));
    }
    engine
        .validate_network_policy(network == NetworkPolicy::Allowed)
        .err()
        .map(|error| EngineRefusal::Policy(error.to_string()))
}

/// Check the authority used to prepare the child against the parent at commit.
pub(crate) fn check_child_authority(
    runtime: &Runtime,
    staged: &Agent,
    parent: &Agent,
    child: &Agent,
) -> Result<()> {
    debug_assert_eq!(staged.id, parent.id);
    debug_assert_eq!(child.parent, Some(parent.id));
    debug_assert_eq!(child.run, parent.run);
    if parent.config.leaf
        || parent.model != staged.model
        || parent.scope != staged.scope
        || parent.config.delegation_policy != staged.config.delegation_policy
        || parent.config.tools != child.config.tools
    {
        return Err(RuntimeError::Invalid(
            "the parent's model, scope or delegation authority changed; retry delegation".into(),
        ));
    }
    if !runtime.config.yolo {
        if !child
            .config
            .approval
            .is_narrowing_of(&parent.config.approval)
        {
            return Err(RuntimeError::Invalid(
                "the prepared child dropped required approvals".into(),
            ));
        }
        if !child.config.approval.is_yolo() {
            for model in std::iter::once(&child.model).chain(&child.fallbacks) {
                if !runtime.engine_can_gate(model)? {
                    return Err(RuntimeError::Invalid(format!(
                        "the child's engine for `{model}` cannot enforce required approvals"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Put `request`'s chain on the route its child will run: each secondary link
/// on its original where usable. A route the parent's policy refuses while it
/// admits the chain as asked keeps the chain as asked.
pub(crate) fn route_request(
    runtime: &Runtime,
    parent: &Agent,
    request: &mut ChildRequest,
    slots: Slots,
) -> Result<()> {
    let (model, fallbacks) =
        runtime.preferred_chain(request.model.clone(), request.fallbacks.clone());
    if model == request.model && fallbacks == request.fallbacks {
        return Ok(());
    }
    debug_assert_eq!(
        fallbacks.len(),
        request.fallbacks.len(),
        "routing keeps every link"
    );
    let asked_model = std::mem::replace(&mut request.model, model);
    let asked_fallbacks = std::mem::replace(&mut request.fallbacks, fallbacks);
    if policy_refusal(runtime, parent, request, slots)?.is_none() {
        return Ok(());
    }
    let routed_model = std::mem::replace(&mut request.model, asked_model);
    let routed_fallbacks = std::mem::replace(&mut request.fallbacks, asked_fallbacks);
    if policy_refusal(runtime, parent, request, slots)?.is_some() {
        // Refused either way: the refusal names the route that would run.
        request.model = routed_model;
        request.fallbacks = routed_fallbacks;
    }
    Ok(())
}

/// The first refusal across the child's chain; the same rules apply to each link.
pub(crate) fn policy_refusal(
    runtime: &Runtime,
    parent: &Agent,
    request: &ChildRequest,
    slots: Slots,
) -> Result<Option<String>> {
    use aether_core::delegation::{Candidate, admits_without_policy};
    let models = &runtime.models;
    let parent_model = models.canonical(&parent.model);
    let parent_tier = models.delegation_tier(&parent_model);
    let role = request.role.trim();
    let policy = parent
        .config
        .delegation_policy
        .as_ref()
        .map(|policy| canonical_policy(runtime, policy));
    let unfinished = match &policy {
        Some(policy) if policy.max_parallel.is_some() && slots == Slots::Counted => {
            runtime.unfinished_children(parent.id, policy)?
        }
        _ => 0,
    };
    let independent = policy
        .as_ref()
        .is_some_and(|p| p.require_independent_vendor);
    let parent_vendor = independent
        .then(|| model_vendor(runtime, &parent_model))
        .flatten();
    for (index, link) in std::iter::once(&request.model)
        .chain(&request.fallbacks)
        .enumerate()
    {
        let model = models.canonical(link);
        let child = Candidate {
            role,
            model: &model,
            tier: models.delegation_tier(&model),
            effort: request.effort,
        };
        let admitted = match &policy {
            Some(policy) => policy.admits(&child, parent_tier, unfinished),
            None => admits_without_policy(&child, parent_tier),
        };
        let verdict = admitted.and_then(|()| {
            if independent {
                check_vendor(runtime, link, parent_vendor)?;
            }
            let named_by_policy = policy.as_ref().is_some_and(|p| p.names_model(role, &model));
            check_frontier_reason(&child, &parent_model, named_by_policy, &request.reason)
        });
        if let Err(refusal) = verdict {
            return Ok(Some(if index == 0 {
                refusal
            } else {
                format!("fallback model `{link}`: {refusal}")
            }));
        }
    }
    Ok(None)
}

fn check_frontier_reason(
    child: &aether_core::delegation::Candidate<'_>,
    parent_model: &ModelRef,
    named_by_policy: bool,
    reason: &ChildReason,
) -> std::result::Result<(), String> {
    if aether_core::delegation::needs_reason(child, parent_model)
        && !named_by_policy
        && !matches!(reason, ChildReason::Given(_))
    {
        return Err(format!(
            "delegation refused: {} is your own frontier-tier model. Workers default to the \
             balanced or fast tier (list_models shows each model's tier); {}",
            child.model,
            reason.remedy()
        ));
    }
    Ok(())
}

fn model_vendor(runtime: &Runtime, model: &ModelRef) -> Option<&'static str> {
    use aether_model::profile::ModelFamily;

    let resolved = runtime.models.resolve(model).ok()?;
    resolved
        .provider
        .protocol
        .api_equivalent_provider()
        .or(match resolved.family() {
            Some(ModelFamily::Claude) => Some("anthropic"),
            Some(ModelFamily::Gpt | ModelFamily::Codex) => Some("openai"),
            None => None,
        })
}

fn check_vendor(
    runtime: &Runtime,
    model: &ModelRef,
    parent_vendor: Option<&str>,
) -> std::result::Result<(), String> {
    match (parent_vendor, model_vendor(runtime, model)) {
        (Some(parent), Some(child)) if parent != child => Ok(()),
        (Some(parent), Some(_)) => Err(format!(
            "delegation refused: {model} shares your vendor `{parent}` and the policy requires a different vendor"
        )),
        _ => Err(format!(
            "delegation refused: cannot establish that {model} is from a different vendor, as the policy requires"
        )),
    }
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
    // The default plus `approval: None` lets preparation inherit the current policy.
    config.approval = Default::default();
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
        fallbacks: request.fallbacks,
        scope: request.scope,
        // A delegated child inherits its parent's repository.
        repository_root: None,
        parent: Some(parent.id),
        config: Some(config),
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
/// caller routes `request` first, with [`route_request`], and admits the
/// result with `Runtime::admit_prepared` under its own hold.
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
    let network = request.network(parent);
    let prepared = runtime.prepare_agent(
        child_spec(parent, request),
        by,
        Preflight {
            checked: Some((&model, network)),
            ..Preflight::delegated(parent)
        },
    )?;
    debug_assert_eq!(prepared.agent.parent, Some(parent.id));
    debug_assert_eq!(prepared.agent.run, parent.run, "a child joins its run");
    Ok(prepared)
}
