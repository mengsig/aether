use aether_core::agent::{AgentConfig, ApprovalPolicy, NetworkPolicy, Scope};
use aether_core::id::AgentId;
use aether_core::model::ModelRef;
use aether_protocol::{AgentPatch, ErrorReply};
use aether_runtime::Runtime;

pub(super) fn creation(
    rt: &Runtime,
    scope: Option<&Scope>,
    config: Option<&AgentConfig>,
    approval: Option<&ApprovalPolicy>,
    parent: Option<AgentId>,
    model: Option<&ModelRef>,
    fallbacks: &[ModelRef],
) -> Result<(), ErrorReply> {
    if let Some(scope) = scope {
        check_scope(rt, scope)?;
    }
    let ceiling = approval_ceiling(rt, parent)?;
    let requested = approval.or_else(|| {
        config
            .map(|c| &c.approval)
            .filter(|p| **p != ApprovalPolicy::default())
    });
    if requested.is_some_and(|policy| !policy.is_narrowing_of(&ceiling)) {
        return Err(ErrorReply::forbidden(
            "remote creation cannot relax the host's approval ceiling",
        ));
    }
    let effective = requested.unwrap_or(&ceiling);
    let model = model.cloned().or_else(|| rt.default_model());
    check_model(rt, model.as_ref(), effective)?;
    for model in fallbacks {
        check_model(rt, Some(model), effective)?;
    }
    Ok(())
}

fn check_scope(rt: &Runtime, scope: &Scope) -> Result<(), ErrorReply> {
    if scope.network == NetworkPolicy::Allowed
        && rt.config().network_default == NetworkPolicy::Denied
    {
        return Err(ErrorReply::forbidden(
            "remote scope cannot enable networking denied by the host",
        ));
    }
    if scope.repository.is_some() {
        return Err(ErrorReply::forbidden(
            "remote scope must derive its repository identity from the workspace",
        ));
    }
    if !scope.allowed_paths.is_empty() {
        return Err(ErrorReply::forbidden(
            "extra filesystem roots must be granted at the host",
        ));
    }
    Ok(())
}

fn approval_ceiling(rt: &Runtime, parent: Option<AgentId>) -> Result<ApprovalPolicy, ErrorReply> {
    match parent {
        Some(id) => Ok(rt
            .store()
            .agent(id)
            .map_err(ErrorReply::internal)?
            .ok_or_else(|| ErrorReply::not_found(id))?
            .config
            .approval),
        None if rt.config().yolo => Ok(ApprovalPolicy::none()),
        None => Ok(rt.config().approval_default.policy()),
    }
}

pub(super) fn approvals(
    rt: &Runtime,
    id: AgentId,
    policy: &ApprovalPolicy,
) -> Result<(), ErrorReply> {
    let agent = rt
        .store()
        .agent(id)
        .map_err(ErrorReply::internal)?
        .ok_or_else(|| ErrorReply::not_found(id))?;
    let ceiling = approval_ceiling(rt, agent.parent)?;
    if !policy.is_narrowing_of(&ceiling) {
        return Err(ErrorReply::forbidden(
            "remote approval policy cannot relax the host's ceiling",
        ));
    }
    check_model(rt, Some(&agent.model), policy)
}

pub(super) fn update(rt: &Runtime, id: AgentId, patch: &AgentPatch) -> Result<(), ErrorReply> {
    if patch.network == Some(NetworkPolicy::Allowed)
        && rt.config().network_default == NetworkPolicy::Denied
    {
        return Err(ErrorReply::forbidden(
            "remote update cannot enable networking denied by the host",
        ));
    }
    if let Some(policy) = &patch.approval {
        approvals(rt, id, policy)?;
    }
    let agent = rt
        .store()
        .agent(id)
        .map_err(ErrorReply::internal)?
        .ok_or_else(|| ErrorReply::not_found(id))?;
    let policy = patch.approval.as_ref().unwrap_or(&agent.config.approval);
    check_model(rt, patch.model.as_ref().or(Some(&agent.model)), policy)?;
    for model in patch.fallbacks.as_ref().unwrap_or(&agent.fallbacks) {
        check_model(rt, Some(model), policy)?;
    }
    Ok(())
}

fn check_model(
    rt: &Runtime,
    model: Option<&ModelRef>,
    policy: &ApprovalPolicy,
) -> Result<(), ErrorReply> {
    if !policy.is_yolo() && model.is_some_and(|model| !rt.engine_can_gate(model)) {
        return Err(ErrorReply::forbidden(
            "a remote client cannot select an engine that drops required approvals",
        ));
    }
    Ok(())
}
