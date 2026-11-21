//! A tool discovered on an MCP server, presented to agents as an ordinary
//! Aether tool.

use std::sync::Arc;

use aether_core::artifact::ArtifactKind;
use aether_core::mcp::McpToolInfo;
use aether_core::tool::{ResourceKey, ToolDescriptor, ToolOutcome};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{MAX_STRUCTURED_CHARS, McpCaller, McpError, McpManager};
use crate::output::OutputPolicy;
use crate::tool::{Tool, ToolContext, ToolError, ToolExecution};

/// One remote tool. Holds the manager rather than a connection, so a
/// reconnect between calls is invisible here.
#[derive(Debug)]
pub struct McpTool {
    manager: Arc<McpManager>,
    server: String,
    info: McpToolInfo,
    description: String,
    input_schema: Value,
    /// A chatty server must not flood the model context any more than a
    /// chatty command does.
    output: OutputPolicy,
}

/// Characters of a call's rendering kept verbatim in its approval rule. The
/// argument becomes a durable rule and a line a person reads on an approval
/// card, and the model chose it, so it is bounded like every other read here.
const MAX_RULE_ARGUMENT_CHARS: usize = 2_000;

impl McpTool {
    pub(super) fn new(
        manager: Arc<McpManager>,
        server: String,
        info: McpToolInfo,
        description: String,
        input_schema: Value,
    ) -> Self {
        let description = match description.trim() {
            "" => format!("[mcp:{server}] {}", info.name),
            text => format!("[mcp:{server}] {text}"),
        };
        Self {
            manager,
            server,
            info,
            description,
            input_schema,
            output: OutputPolicy::default(),
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn descriptor(&self) -> ToolDescriptor {
        crate::builtin::descriptor(
            &self.info.qualified_name,
            &self.description,
            self.input_schema.clone(),
            self.info.side_effect,
            // Every MCP server can reach the network: an HTTP one by
            // definition, a stdio one because it is an unconstrained child of
            // the daemon. The scope's network policy decides both.
            true,
        )
    }

    /// The server runs outside whatever backend the agent is scoped to: an
    /// HTTP one on another machine, a stdio one as a daemon child on the
    /// host. A containerized agent must not reach around its container.
    fn escapes_backend(&self) -> bool {
        true
    }

    /// One key per server - calls to the same server are one shared resource,
    /// which is also how the transport behaves - plus every path the call
    /// names, so a rule about a path still sees it.
    fn resources(&self, input: &Value, ctx: &ToolContext) -> Vec<ResourceKey> {
        let mut keys = vec![ResourceKey(format!("mcp:{}", self.server))];
        keys.extend(named_paths(input, ctx));
        keys
    }

    /// Never: the server runs the call, so what it touches beyond the paths
    /// it was handed is unknowable here. A resource deny then fails it closed
    /// rather than being silently inert.
    fn resources_known(&self, _input: &Value) -> bool {
        false
    }

    /// The server holds the call, not this daemon, so `mcp:<server>` is an
    /// ordering key and never a fleet-wide lease. Only a path the call
    /// declares is a claim on the host worth holding, and only when the call
    /// may change it.
    fn leases_resources(&self, input: &Value) -> bool {
        self.info.side_effect.is_mutating() && names_a_path(input)
    }

    /// The whole call, rendered canonically: a remembered answer is pinned to
    /// the call that was answered, the way a shell's is pinned to its command.
    /// Without one the rule falls back to the narrowest resource prefix, which
    /// for an MCP call is the constant `mcp:<server>` - so "always allow" would
    /// silently cover this tool with any arguments at all, on exactly the tools
    /// Aether knows least about. Serde's object is ordered, so equal inputs
    /// always render equal.
    /// Bounded, because the model chose the input: an oversized call must not
    /// become an oversized durable rule, or an unreadable approval card.
    fn rule_argument(&self, input: &Value) -> Option<String> {
        let rendered = input.to_string();
        if rendered.chars().count() <= MAX_RULE_ARGUMENT_CHARS {
            return Some(rendered);
        }
        // A prefix a person can recognise, plus a digest over the whole call
        // - so the rule is still one value per distinct call, and remembering
        // an answer still covers only the call that was answered.
        let kept: String = rendered.chars().take(MAX_RULE_ARGUMENT_CHARS).collect();
        Some(format!(
            "{kept}… {}",
            aether_core::idempotency::IdempotencyKey::derive(&self.info.qualified_name, input)
        ))
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let caller = McpCaller::of(ctx);
        let replay_safe = self.info.side_effect.safe_to_retry() || carries_idempotency_key(&input);
        let call = self
            .manager
            .call(&self.server, &self.info.name, input, &caller, replay_safe);
        let result = tokio::select! {
            result = call => result,
            _ = cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        let result = match result {
            Ok(result) => result,
            Err(McpError::InvalidArguments(reason)) => return Err(ToolError::InvalidInput(reason)),
            Err(McpError::TransportFailed { retryable, cause }) => {
                return Ok(self.transport_failed(retryable, &cause));
            }
            Err(e) => {
                return Err(ToolError::Internal(format!(
                    "mcp server `{}`: {e}",
                    self.server
                )));
            }
        };
        let mut text = match result.text.trim() {
            "" => "(no content)".to_owned(),
            _ => result.text,
        };
        // The server chose this JSON and it is committed whole to the event
        // log, so it is bounded here the way the text half is; dropping it
        // is said out loud rather than leaving the model a silent gap.
        let (structured, dropped) = bound_structured(result.structured);
        if let Some(note) = dropped {
            text.push('\n');
            text.push_str(&note);
        }
        let bounded = self
            .output
            .bound(
                text,
                &format!("{} output", self.info.qualified_name),
                ArtifactKind::CommandOutput,
                ctx,
            )
            .await?;
        Ok(ToolExecution::Outcome(ToolOutcome {
            content: bounded.text,
            is_error: result.is_error,
            artifacts: bounded.artifacts,
            structured,
            truncated: bounded.truncated,
            side_effect_occurred: Some(self.info.side_effect.is_mutating()),
            ..Default::default()
        }))
    }
}

impl McpTool {
    /// The structured result of a call whose connection dropped: never a bare
    /// "connection closed", so the model and the host can tell what to do.
    fn transport_failed(&self, retryable: bool, cause: &str) -> ToolExecution {
        let failure = aether_core::mcp::tool_transport_failed(
            &self.info.qualified_name,
            retryable,
            &format!("mcp server `{}`: {cause}", self.server),
        );
        debug_assert_eq!(failure["retryable"], retryable);
        ToolExecution::Outcome(ToolOutcome {
            content: failure.to_string(),
            is_error: true,
            structured: Some(failure),
            // A call that cannot be replayed may have reached the server.
            side_effect_occurred: (!retryable && self.info.side_effect.is_mutating())
                .then_some(true),
            ..Default::default()
        })
    }
}

/// Whether the model gave this call an idempotency key, which makes sending it
/// again one effect rather than two.
fn carries_idempotency_key(input: &Value) -> bool {
    input
        .get("idempotency_key")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

/// `structuredContent` if it fits the budget, else nothing plus the line
/// that says why. Unbounded it would be an attacker-chosen write of any size
/// into the store, since the whole outcome becomes one event.
fn bound_structured(structured: Option<Value>) -> (Option<Value>, Option<String>) {
    let Some(value) = structured else {
        return (None, None);
    };
    let rendered = match serde_json::to_string(&value) {
        Ok(text) => text,
        Err(e) => {
            return (
                None,
                Some(format!(
                    "[structuredContent dropped: it cannot be encoded: {e}]"
                )),
            );
        }
    };
    let total = rendered.chars().count();
    if total <= MAX_STRUCTURED_CHARS {
        return (Some(value), None);
    }
    (
        None,
        Some(format!(
            "[structuredContent dropped: {total} chars, over the {MAX_STRUCTURED_CHARS}-char cap; \
             the text above is the whole answer]"
        )),
    )
}

/// The fields the rest of Aether treats as a call's path.
const PATH_FIELDS: [&str; 3] = ["path", "file_path", "notebook_path"];

/// The paths a call names, resolved the way every built-in tool resolves its
/// own, so one rule matches the same path on either path.
fn named_paths(input: &Value, ctx: &ToolContext) -> Vec<ResourceKey> {
    PATH_FIELDS
        .iter()
        .filter_map(|field| input.get(*field).and_then(Value::as_str))
        .flat_map(|named| crate::fs::resource_paths(ctx, named))
        .map(ResourceKey::file)
        .collect()
}

/// Whether the call declares a path of its own.
fn names_a_path(input: &Value) -> bool {
    PATH_FIELDS
        .iter()
        .any(|field| input.get(*field).and_then(Value::as_str).is_some())
}

#[cfg(test)]
mod tests {
    use aether_core::tool::SideEffectClass;

    use super::*;

    #[test]
    fn structured_content_is_bounded_and_its_loss_is_stated() {
        let small = serde_json::json!({"rows": 3});
        assert_eq!(
            bound_structured(Some(small.clone())),
            (Some(small), None),
            "what fits is carried unchanged"
        );
        assert_eq!(bound_structured(None), (None, None));

        // The server chooses this, and the whole outcome becomes one event.
        let flood = serde_json::json!({ "big": "x".repeat(MAX_STRUCTURED_CHARS) });
        let (structured, note) = bound_structured(Some(flood));
        assert_eq!(
            structured, None,
            "an oversized value never reaches an event"
        );
        let note = note.expect("dropping it is said out loud");
        assert!(note.contains("structuredContent dropped"), "{note}");
    }

    #[test]
    fn an_oversized_call_does_not_become_an_oversized_rule() {
        let tool = McpTool::new(
            McpManager::new(Vec::new(), None, Vec::new().into()),
            "s".into(),
            McpToolInfo {
                name: "write".into(),
                qualified_name: "mcp__s__write".into(),
                side_effect: SideEffectClass::External,
                declared: false,
            },
            "writes".into(),
            serde_json::json!({"type": "object"}),
        );

        let small = serde_json::json!({"path": "a.txt"});
        assert_eq!(
            tool.rule_argument(&small),
            Some(small.to_string()),
            "a call inside the budget is remembered verbatim"
        );

        let huge = serde_json::json!({"body": "x".repeat(MAX_RULE_ARGUMENT_CHARS * 2)});
        let rule = tool.rule_argument(&huge).expect("an argument all the same");
        assert!(
            rule.chars().count() < MAX_RULE_ARGUMENT_CHARS + 200,
            "an oversized call must not become an oversized durable rule: {} chars",
            rule.chars().count()
        );
        // Still one value per distinct call, so a remembered answer covers
        // the call that was answered and not the tool.
        let other = serde_json::json!({"body": "y".repeat(MAX_RULE_ARGUMENT_CHARS * 2)});
        assert_ne!(rule, tool.rule_argument(&other).expect("argument"));
    }

    fn tool_with(side_effect: SideEffectClass) -> McpTool {
        McpTool::new(
            McpManager::new(Vec::new(), None, Vec::new().into()),
            "s".into(),
            McpToolInfo {
                name: "write".into(),
                qualified_name: "mcp__s__write".into(),
                side_effect,
                declared: true,
            },
            "writes".into(),
            serde_json::json!({"type": "object"}),
        )
    }

    #[test]
    fn a_call_holds_a_lease_only_on_a_path_it_declares() {
        let mutating = tool_with(SideEffectClass::Mutation);
        assert!(
            !mutating.leases_resources(&serde_json::json!({"note": "hi"})),
            "the server runs the call; `mcp:s` is not a claim on this host"
        );
        assert!(
            mutating.leases_resources(&serde_json::json!({"path": "a.txt"})),
            "a declared path is a real claim and must be held"
        );
        assert!(
            !tool_with(SideEffectClass::ReadOnly)
                .leases_resources(&serde_json::json!({"path": "a.txt"})),
            "a read-only call excludes nobody"
        );
    }

    #[test]
    fn a_declared_path_is_normalized_the_way_every_other_tool_normalizes_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = crate::testing::ctx_in(dir.path());
        let tool = tool_with(SideEffectClass::Mutation);

        let relative = tool.resources(&serde_json::json!({"path": "sub/../a.txt"}), &ctx);
        assert!(
            relative.contains(&ResourceKey::file(dir.path().join("a.txt"))),
            "a relative claim resolves against the agent's tree: {relative:?}"
        );
        // A rule is matched by literal prefix, so a claim that walks out of
        // the workspace must name where it really lands, not keep the dots.
        let outside = dir
            .path()
            .parent()
            .and_then(std::path::Path::parent)
            .expect("a tempdir has two ancestors")
            .join("etc/passwd");
        let escaping = tool.resources(&serde_json::json!({"file_path": "../../etc/passwd"}), &ctx);
        assert!(
            escaping.contains(&ResourceKey::file(&outside)),
            "dot-dot must resolve to what it names, not hide it from a rule: {escaping:?}"
        );
        assert!(
            !escaping.iter().any(|k| k.0.contains("..")),
            "an unnormalized key matches no prefix a person would write: {escaping:?}"
        );
    }
}
