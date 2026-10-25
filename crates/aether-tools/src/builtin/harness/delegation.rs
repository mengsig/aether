use super::{harness_descriptor, parse_id};
use crate::tool::{
    DelegateKey, HarnessAction, Tool, ToolContext, ToolError, ToolExecution, WorkspaceMode,
    parse_input,
};
use aether_core::tool::{ResourceKey, SideEffectClass, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use aether_core::{
    agent::{HostContext, NetworkPolicy},
    id::AgentId,
    model::{Effort, ModelRef},
};

// ----- delegate ------------------------------------------------------------

#[derive(Debug)]
pub struct Delegate;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateInput {
    role: String,
    objective: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    model: String,
    // A raw `Value`, not `Effort`: direct enum deserialization loses the
    // field name through `serde::de::Error::custom` (aether-core/src/id.rs
    // and model.rs) - and a `String` field would drop it too, just for
    // wrong-typed values instead of wrong-valued ones. Parsed below through
    // `parse_field` so the rejection names `delegate.effort` either way.
    effort: Value,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    workspace_mode: WorkspaceMode,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    network: Option<bool>,
    #[serde(default = "yes")]
    wait: bool,
    #[serde(default)]
    leaf: bool,
    // A raw `Value` for the same reason as `effort`.
    #[serde(default)]
    host_context: Option<Value>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Longest caller key: the runtime prefixes it with the calling agent, and
/// the whole must fit `MAX_SUBMISSION_KEY_BYTES`.
const MAX_DELEGATE_KEY_BYTES: usize = 128;

/// The caller's arguments as sent, without `wait` or the key: what decides
/// whether a repeat is the same request. Taken before any inheritance from
/// the caller's current scope, which may have changed since the first call.
fn keyed_request(input: &Value) -> Option<DelegateKey> {
    let key = input.get("idempotency_key")?.as_str()?.to_owned();
    let mut request = input.clone();
    let fields = request.as_object_mut()?;
    fields.remove("idempotency_key");
    fields.remove("wait");
    Some(DelegateKey { key, request })
}

fn validate_idempotency_key(key: &str) -> Result<(), ToolError> {
    let printable = key.bytes().all(|b| (0x21..=0x7E).contains(&b));
    if key.is_empty() || key.len() > MAX_DELEGATE_KEY_BYTES || !printable {
        return Err(ToolError::InvalidInput(format!(
            "idempotency_key must be 1-{MAX_DELEGATE_KEY_BYTES} bytes of printable ASCII with no spaces"
        )));
    }
    Ok(())
}

fn yes() -> bool {
    true
}

#[async_trait]
impl Tool for Delegate {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "delegate",
            "Create a child agent with its own task. You must choose both its model and reasoning effort explicitly. With wait=true (default) you sleep until \
             the child finishes and its result arrives in your inbox; you consume no tokens while \
             waiting. With wait=false the child runs in the background and you can wait later \
             with wait_for. The child's scope can only narrow yours. Use workspace_mode=isolated \
             for concurrent editing; that creates a managed Git worktree snapshot and requires \
             deliberate integration later. Omitted workspace_mode keeps the shared workspace. \
             idempotency_key makes a repeat after a lost reply safe.",
            json!({
                "type": "object",
                "properties": {
                    "role": {"type": "string", "description": "Role label for the child, e.g. `worker`, `reviewer`."},
                    "objective": {"type": "string", "description": "What the child must accomplish, with acceptance criteria."},
                    "name": {"type": "string", "description": "Short display name."},
                    "instructions": {"type": "string", "description": "Extra instructions for this child's task. Omitted instructions are empty; your own task instructions are not copied to the child."},
                    "model": {"type": "string", "description": "Required exact `provider/model` connection ID, e.g. `codex/gpt-6.1-sol` or `claude-code/opus`. Choose capability first, then cost within the suitable tier; list_models supplies current preferences and prices. Pass an ID you were already given straight through; an unknown one is refused by name. Resolve a bare model family with list_models rather than guessing a vendor connection."},
                    "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"], "description": "Required reasoning effort for this child's task. Choose explicitly; there is no inheritance default."},
                    "workspace_root": {"type": "string", "description": "Narrower workspace for the child. With isolated mode, select a Git repository root within your scope; omit to use your workspace root."},
                    "workspace_mode": {"type": "string", "enum": ["shared", "isolated"], "default": "shared", "description": "shared inherits or narrows your workspace. isolated snapshots the selected Git repository into a managed worktree; work remains there until integrate_workspace is called."},
                    "allowed_paths": {"type": "array", "items": {"type": "string"}, "description": "Paths the child owns, enforced by the runtime rather than by prose in the objective. Exactly what is enforced: the file tools refuse any path outside the workspace root and these, and Aether's built-in shell refuses git commands that discard working-tree changes outside them (`git stash`, `git checkout -- <paths>`, `git restore`, `git reset --hard`, `git clean`); other shell writes are not sandboxed, and these paths widen the file tools rather than narrowing the workspace - use workspace_root for that. Use it whenever siblings share a workspace so each child owns distinct files; like every scope field it can only narrow yours."},
                    "network": {"type": "boolean", "description": "Omit to inherit your network policy. Read-only work does not require disabling network. Set false when the task requires offline execution; cannot exceed your own policy."},
                    "leaf": {"type": "boolean", "default": false, "description": "Create a child that cannot delegate and receives fewer coordination tools."},
                    "host_context": {"type": "string", "enum": ["full", "lean"], "description": "lean starts a Claude Code child without its native skills and non-Aether MCP servers, for a smaller context; full restores them. Omit to inherit yours."},
                    "wait": {"type": "boolean", "default": true},
                    "idempotency_key": {"type": "string", "maxLength": 128, "description": "Printable ASCII, no spaces. A repeat with the same key and arguments returns the first call's agent_id and task_id, not a second child; other arguments under a used key are refused. `wait` is not compared. Kept up to seven days."}
                },
                "required": ["role", "objective", "model", "effort"],
                "additionalProperties": false
            }),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    /// A decided, in-memory state transition once the model commits to it
    /// - see `Tool::interruptible`.
    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let keyed = keyed_request(&input);
        let input: DelegateInput = parse_input(input)?;
        if input.role.trim().is_empty() {
            return Err(ToolError::InvalidInput("role is empty".into()));
        }
        if input.objective.trim().is_empty() {
            return Err(ToolError::InvalidInput("objective is empty".into()));
        }
        if let Some(key) = &input.idempotency_key {
            validate_idempotency_key(key)?;
        }
        let model = ModelRef::parse(&input.model).ok_or_else(|| {
            ToolError::InvalidInput(format!(
                "model must be `provider/model`, got `{}`",
                input.model
            ))
        })?;
        let effort: Effort = crate::builtin::parse_field(&input.effort, "delegate.effort")?;
        let host_context: Option<HostContext> = input
            .host_context
            .as_ref()
            .map(|value| crate::builtin::parse_field(value, "delegate.host_context"))
            .transpose()?;
        let wants_scope = input.workspace_root.is_some()
            || !input.allowed_paths.is_empty()
            || input.network.is_some();
        let scope = wants_scope.then(|| {
            let mut s = ctx.scope.clone();
            if let Some(root) = &input.workspace_root {
                s.workspace_root = Some(crate::fs::lexical_resolve(ctx, root));
            }
            if !input.allowed_paths.is_empty() {
                s.allowed_paths = input
                    .allowed_paths
                    .iter()
                    .map(|p| crate::fs::lexical_resolve(ctx, p))
                    .collect();
            }
            if let Some(net) = input.network {
                s.network = if net {
                    NetworkPolicy::Allowed
                } else {
                    NetworkPolicy::Denied
                };
            }
            s
        });
        // A keyed call may repeat one that already ran under a since-narrowed
        // scope; its replay comes first, and creation still refuses a widening.
        if keyed.is_none()
            && let Some(s) = &scope
            && !s.is_narrowing_of(&ctx.scope)
        {
            return Err(ToolError::Denied(
                "child scope must narrow the parent's scope".into(),
            ));
        }
        Ok(ToolExecution::Action(HarnessAction::Delegate {
            role: input.role,
            name: input.name,
            objective: input.objective,
            instructions: input.instructions,
            model,
            effort,
            scope,
            workspace_mode: input.workspace_mode,
            wait: input.wait,
            leaf: input.leaf,
            host_context,
            idempotency: keyed,
        }))
    }
}

#[derive(Debug)]
pub struct IntegrateWorkspace;

#[async_trait]
impl Tool for IntegrateWorkspace {
    fn descriptor(&self) -> ToolDescriptor {
        super::super::descriptor(
            "integrate_workspace",
            "Copy a completed direct child's isolated workspace changes into your workspace after checking every changed path against the delegation snapshot. Conflicts refuse integration and leave both workspaces intact. No merge or cleanup is automatic.",
            json!({
                "type": "object",
                "properties": {"child_agent_id": {"type": "string", "description": "The direct child agent ID returned by delegate."}},
                "required": ["child_agent_id"],
                "additionalProperties": false
            }),
            SideEffectClass::Mutation,
            false,
        )
    }

    fn resources(&self, _input: &Value, ctx: &ToolContext) -> Vec<ResourceKey> {
        vec![ResourceKey::repo(&ctx.cwd)]
    }

    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            child_agent_id: String,
        }
        let input: Input = parse_input(input)?;
        Ok(ToolExecution::Action(HarnessAction::IntegrateWorkspace {
            child: parse_id(&input.child_agent_id, "child_agent_id")?,
        }))
    }
}

// ----- send_message --------------------------------------------------------

#[derive(Debug)]
pub struct SendMessage;

#[async_trait]
impl Tool for SendMessage {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "send_message",
            "Send a message to another agent's inbox. It wakes the recipient if it is waiting for messages. \
             Set `subtree` to broadcast the same message to the recipient and every live agent beneath it. \
             A message to your own child also interrupts any wait it is holding (except an approval wait); \
             a message to any other agent, including your parent, waits until that agent's condition is satisfied.",
            json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "Recipient agent ID (`agt_...`) from delegate.agent_id, list_peers, inspect_state, or an Aether inbox sender. Display names and native paths such as `/root` are not IDs."},
                    "content": {"type": "string"},
                    "subtree": {"type": "boolean", "default": false, "description": "Deliver to the recipient and every live descendant instead of the recipient alone."}
                },
                "required": ["to", "content"],
                "additionalProperties": false
            }),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    /// A decided, in-memory state transition once the model commits to it
    /// - see `Tool::interruptible`.
    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            to: String,
            content: String,
            #[serde(default)]
            subtree: bool,
        }
        let Input {
            to,
            content,
            subtree,
        } = parse_input(input)?;
        if content.trim().is_empty() {
            return Err(ToolError::InvalidInput("content is empty".into()));
        }
        let to: AgentId = parse_id(&to, "agent id")?;
        Ok(ToolExecution::Action(HarnessAction::SendMessage {
            to,
            content,
            subtree,
        }))
    }
}

#[derive(Debug)]
pub struct CancelAgent;

#[async_trait]
impl Tool for CancelAgent {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "cancel_agent",
            "Cancel a delegated descendant and its entire subtree, including live children beneath completed agents. You may only cancel your descendants, not yourself, ancestors, or other trees. Cancellation is durable; running turns stop cooperatively. Use complete_task or fail_task to finish your own task.",
            json!({
                "type": "object",
                "properties": {
                    "agent": {"type": "string", "description": "The descendant agent ID (agt_...) returned by delegation."},
                    "reason": {"type": "string", "description": "Why this work should stop."}
                },
                "required": ["agent", "reason"],
                "additionalProperties": false
            }),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            agent: String,
            reason: String,
        }
        let Input { agent, reason } = parse_input(input)?;
        if reason.trim().is_empty() {
            return Err(ToolError::InvalidInput("reason must not be empty".into()));
        }
        Ok(ToolExecution::Action(HarnessAction::CancelAgent {
            agent: parse_id(&agent, "agent id")?,
            reason,
        }))
    }
}
