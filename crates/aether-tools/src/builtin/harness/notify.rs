use super::harness_descriptor;
use crate::tool::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, parse_input};
use aether_core::notification::{NotificationAction, NotificationKind, NotificationPost};
use aether_core::tool::{ResourceKey, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

// ----- notify --------------------------------------------------------------

#[derive(Debug)]
pub struct Notify;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyInput {
    #[serde(default)]
    title: Option<String>,
    // A raw `Value` so the rejection names `notify.kind`.
    #[serde(default)]
    kind: Option<Value>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    actions: Vec<NotificationAction>,
    #[serde(default)]
    free_text: bool,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    resolve: bool,
}

#[async_trait]
impl Tool for Notify {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "notify",
            "Post to the operator's notification center without waiting. An answer reaches \
             your inbox later. The same key updates it; resolve with the key closes it.",
            json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "kind": {"type": "string", "enum": ["question", "blocker", "decision", "outcome", "info"]},
                    "body": {"type": "string", "description": "Short Markdown."},
                    "key": {"type": "string"},
                    "actions": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "label": {"type": "string"}}}},
                    "free_text": {"type": "boolean"},
                    "link": {"type": "string"},
                    "resolve": {"type": "boolean"}
                },
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
        let input: NotifyInput = parse_input(input)?;
        if input.resolve {
            return resolution(input);
        }
        let title = input.title.unwrap_or_default();
        let kind = match &input.kind {
            Some(raw) => crate::builtin::parse_field(raw, "notify.kind")?,
            None => NotificationKind::Info,
        };
        let post = NotificationPost {
            kind,
            title,
            body: input.body,
            key: input.key,
            link: input.link,
            actions: input.actions,
            free_text: input.free_text,
        };
        post.check().map_err(ToolError::InvalidInput)?;
        Ok(ToolExecution::Action(HarnessAction::Notify { post }))
    }
}

/// `resolve` names a keyed notification and nothing else.
fn resolution(input: NotifyInput) -> Result<ToolExecution, ToolError> {
    let Some(key) = input.key.filter(|key| !key.trim().is_empty()) else {
        return Err(ToolError::InvalidInput(
            "notify.resolve needs the key the notification was posted under".into(),
        ));
    };
    let posts = input.title.is_some() || input.kind.is_some() || !input.body.is_empty();
    if posts || !input.actions.is_empty() || input.free_text || input.link.is_some() {
        return Err(ToolError::InvalidInput(
            "notify.resolve takes only the key; post again under it to change the content".into(),
        ));
    }
    Ok(ToolExecution::Action(HarnessAction::ResolveNotification {
        key,
    }))
}
