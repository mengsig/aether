//! The `spawn` argument of `create_schedule`: a template for the fresh agent
//! each occurrence starts, bounded by the runtime as `delegate` bounds a child.

use aether_core::agent::{AgentConfig, HostContext};
use aether_core::model::{Effort, ModelRef};
use aether_core::schedule::SpawnSpec;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tool::{ToolContext, ToolError};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpawnInput {
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
    max_turns: Option<u32>,
    #[serde(default)]
    cost_budget: Option<f64>,
    #[serde(default)]
    token_budget: Option<u64>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    network: Option<bool>,
}

/// The JSON schema of [`SpawnInput`].
pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "description": "Spawn a fresh agent on each occurrence instead of delivering to one. Bounded like delegate: model and effort within your delegation policy, scope and approvals no wider than yours. Each run is its own root run, so you cannot schedule one while you or an ancestor has a cost or token budget; an occurrence skips while the previous run is live, and the timer fails once you finish.",
        "properties": {
            "role": {"type": "string"},
            "objective": {"type": "string", "description": "Each run's task; the due time and any payload are appended."},
            "name": {"type": "string"},
            "instructions": {"type": "string"},
            "model": {"type": "string", "description": "`provider/model`; choose a cheap tier your policy allows."},
            "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"]},
            "host_context": {"type": "string", "enum": ["full", "lean"]},
            "leaf": {"type": "boolean"},
            "max_turns": {"type": "integer", "minimum": 1},
            "cost_budget": {"type": "number", "exclusiveMinimum": 0},
            "token_budget": {"type": "integer", "minimum": 1},
            "workspace_root": {"type": "string"},
            "allowed_paths": {"type": "array", "items": {"type": "string"}},
            "network": {"type": "boolean"}
        },
        "required": ["role", "objective", "model", "effort"],
        "additionalProperties": false
    })
}

/// The template the runtime checks against the calling agent.
pub(super) fn template(input: SpawnInput, ctx: &ToolContext) -> Result<SpawnSpec, ToolError> {
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
    let host_context: Option<HostContext> = input
        .host_context
        .as_ref()
        .map(|value| super::super::parse_field(value, "create_schedule.spawn.host_context"))
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
        max_turns: input.max_turns,
        cost_budget: input.cost_budget,
        token_budget: input.token_budget,
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
