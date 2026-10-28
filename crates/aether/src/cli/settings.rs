use super::{
    ApprovalPolicy, DeadlineUpdate, Effort, ModelChain, NetworkPolicy, Priority, SetKnobs, connect,
    parse_backend,
};
use anyhow::{anyhow, bail};

pub async fn set(
    socket: impl Into<aether_client::Endpoint>,
    agent: super::AgentRef,
    knobs: SetKnobs,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    use aether_core::tool::ToolExposure;
    use aether_protocol::AgentPatch;

    let client = connect(&socket).await?;
    let id = agent.resolve(&client, &socket).await?;
    let tools = match (knobs.tools.is_empty(), knobs.deny_tools.is_empty()) {
        (true, true) => None,
        (false, true) => Some(ToolExposure::Only {
            names: knobs.tools.into_iter().collect(),
        }),
        (true, false) => Some(ToolExposure::Except {
            names: knobs.deny_tools.into_iter().collect(),
        }),
        (false, false) => bail!("--tool and --deny-tool are alternatives, not a pair"),
    };
    let approval = knobs
        .approval
        .map(|spec| parse_approval(&spec))
        .transpose()?;
    if let Some(warning) = approval.as_ref().and_then(ungated_writes) {
        eprintln!("{warning}");
    }
    let chain = knobs
        .model
        .map(|m| ModelChain::parse(&m).ok_or_else(super::model_format_error))
        .transpose()?;
    let patch = AgentPatch {
        name: None,
        model: chain.as_ref().map(|c| c.primary.clone()),
        // Always sent with the model, so `--model x/y` clears a chain set
        // earlier instead of leaving a stale fallback.
        fallbacks: chain.map(|c| c.fallbacks),
        effort: knobs
            .effort
            .map(|e| e.parse::<Effort>().map_err(|e| anyhow!(e)))
            .transpose()?,
        instructions: knobs.instructions,
        max_turns: knobs.max_turns,
        token_budget: knobs.token_budget,
        cost_budget: knobs.cost_budget,
        deadline: knobs.deadline.as_deref().map(parse_deadline).transpose()?,
        tools,
        approval,
        priority: knobs.priority.as_deref().map(parse_priority).transpose()?,
        long_lived: knobs.long_lived,
        concurrency: knobs.concurrency,
        network: knobs.network.as_deref().map(parse_network).transpose()?,
        backend: knobs.backend.as_deref().map(parse_backend).transpose()?,
    };
    if patch == AgentPatch::default() {
        bail!("give at least one knob to set; `aether set --help` lists them");
    }
    let updated = client.update_agent(id, patch).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&updated)?);
        return Ok(());
    }
    println!("{} updated", updated.id);
    Ok(())
}

/// An RFC 3339 instant, or `none` to remove the deadline entirely.
pub(super) fn parse_deadline(spec: &str) -> anyhow::Result<DeadlineUpdate> {
    if spec.eq_ignore_ascii_case("none") {
        return Ok(DeadlineUpdate::Clear);
    }
    let at = spec
        .parse::<aether_core::time::Timestamp>()
        .map_err(|e| anyhow!("deadline must be RFC 3339, or none: {e}"))?;
    let now = aether_core::time::now();
    if at <= now {
        bail!(
            "deadline {spec} is not in the future (now {}); use a later instant, or none to remove it",
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
    }
    Ok(DeadlineUpdate::Set(at))
}

pub(super) fn parse_priority(spec: &str) -> anyhow::Result<Priority> {
    match spec.to_ascii_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "normal" => Ok(Priority::Normal),
        "high" => Ok(Priority::High),
        "critical" => Ok(Priority::Critical),
        other => Err(anyhow!(
            "priority must be low, normal, high or critical: {other}"
        )),
    }
}

pub(super) fn parse_network(spec: &str) -> anyhow::Result<NetworkPolicy> {
    match spec.to_ascii_lowercase().as_str() {
        "denied" | "deny" | "off" => Ok(NetworkPolicy::Denied),
        "allowed" | "allow" | "on" => Ok(NetworkPolicy::Allowed),
        other => Err(anyhow!("network must be denied or allowed: {other}")),
    }
}

/// `none`, or a comma-separated list of side-effect classes that need a person.
pub(super) fn parse_approval(spec: &str) -> anyhow::Result<aether_core::agent::ApprovalPolicy> {
    use aether_core::tool::SideEffectClass;
    if spec.eq_ignore_ascii_case("none") {
        return Ok(ApprovalPolicy::none());
    }
    let mut require_for = std::collections::BTreeSet::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        require_for.insert(match part.to_ascii_lowercase().as_str() {
            "read_only" => SideEffectClass::ReadOnly,
            "idempotent_mutation" => SideEffectClass::IdempotentMutation,
            "mutation" => SideEffectClass::Mutation,
            "external" => SideEffectClass::External,
            other => bail!(
                "approval class must be read_only, idempotent_mutation, mutation, external \
                 or none: {other}"
            ),
        });
    }
    if require_for.is_empty() {
        bail!("approval requires at least one class; use none to disable approval explicitly");
    }
    Ok(ApprovalPolicy { require_for })
}

/// A gate that leaves file writes open, which `mutation` alone suggests it
/// covers: say so, since Write and Edit are `idempotent_mutation`.
pub(super) fn ungated_writes(policy: &ApprovalPolicy) -> Option<&'static str> {
    use aether_core::tool::SideEffectClass;
    if policy.require_for.is_empty() || policy.requires(SideEffectClass::IdempotentMutation) {
        return None;
    }
    Some(
        "note: file writes and edits (write_file, edit_file, Write, Edit) are \
         idempotent_mutation and still run without asking; add idempotent_mutation to gate them",
    )
}
