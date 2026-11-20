//! The `spawn` argument of `create_schedule`: the `delegate` call each
//! occurrence makes for the calling agent. The runtime admits it as one.

use aether_core::agent::{AgentConfig, HostContext};
use aether_core::id::AgentId;
use aether_core::model::{Effort, ModelRef};
use aether_core::schedule::{ScheduleTarget, SpawnSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tool::{ToolContext, ToolError};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnInput {
    role: String,
    objective: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    model: String,
    // Raw `Value`s so a rejection names the `create_schedule.spawn.*` field.
    effort: Value,
    #[serde(default)]
    host_context: Option<Value>,
    #[serde(default)]
    leaf: bool,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    network: Option<bool>,
}

/// The JSON schema of [`SpawnInput`]: `delegate`'s arguments, less the ones
/// that belong to one call (`wait`, `reason`, references, isolation, the key).
pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "description": "As in delegate. Omitted host_context inherits yours.",
        "properties": {
            "role": {"type": "string"},
            "objective": {"type": "string"},
            "name": {"type": "string"},
            "instructions": {"type": "string"},
            "model": {"type": "string"},
            "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"]},
            "host_context": {"type": "string", "enum": ["lean"]},
            "leaf": {"type": "boolean"},
            "workspace_root": {"type": "string"},
            "allowed_paths": {"type": "array", "items": {"type": "string"}},
            "network": {"type": "boolean"}
        },
        "required": ["role", "objective", "model", "effort"],
        "additionalProperties": false
    })
}

/// A template can ask for a lean child; a full one is whatever the creator is.
fn lean_or_inherited(value: &Value) -> Result<HostContext, ToolError> {
    let field = "create_schedule.spawn.host_context";
    match super::super::parse_field::<HostContext>(value, field)? {
        HostContext::Lean => Ok(HostContext::Lean),
        HostContext::Full => Err(ToolError::InvalidInput(format!(
            "{field}: only `lean` can be asked for; omit it to inherit yours"
        ))),
    }
}

/// What the schedule acts on: the agent `target` names, a child from `spawn`,
/// or the caller when it gave neither.
pub(super) fn target(
    target: Option<AgentId>,
    spawn: Option<Value>,
    ctx: &ToolContext,
) -> Result<ScheduleTarget, ToolError> {
    let Some(spawn) = spawn else {
        return Ok(ScheduleTarget::Agent(target.unwrap_or(ctx.agent_id)));
    };
    if target.is_some() {
        return Err(ToolError::InvalidInput(
            "choose target OR spawn, not both".into(),
        ));
    }
    let input: SpawnInput = super::super::parse_field(&spawn, "create_schedule.spawn")?;
    template(input, ctx).map(ScheduleTarget::Spawn)
}

/// The template the runtime checks against the calling agent.
fn template(input: SpawnInput, ctx: &ToolContext) -> Result<SpawnSpec, ToolError> {
    if input.role.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "create_schedule.spawn.role is empty".into(),
        ));
    }
    if input.objective.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "create_schedule.spawn.objective is empty".into(),
        ));
    }
    let model = ModelRef::parse(&input.model).ok_or_else(|| {
        ToolError::InvalidInput(format!(
            "create_schedule.spawn.model must be `provider/model`, got `{}`",
            input.model
        ))
    })?;
    let effort: Effort = super::super::parse_field(&input.effort, "create_schedule.spawn.effort")?;
    let host_context = input
        .host_context
        .as_ref()
        .map(lean_or_inherited)
        .transpose()?;
    let scope = super::super::requested_scope(
        ctx,
        input.workspace_root.as_deref(),
        &input.allowed_paths,
        input.network,
    );
    if let Some(scope) = &scope
        && !scope.is_narrowing_of(&ctx.scope)
    {
        return Err(ToolError::Denied(
            "a spawn template's scope must narrow yours".into(),
        ));
    }
    let config = AgentConfig {
        instructions: input.instructions,
        effort: Some(effort),
        leaf: input.leaf,
        host_context: host_context.unwrap_or_default(),
        ..AgentConfig::default()
    };
    Ok(SpawnSpec {
        role: input.role,
        name: input.name,
        objective: input.objective,
        model: Some(model),
        fallbacks: Vec::new(),
        scope,
        config: Some(config),
        approval: None,
    })
}
