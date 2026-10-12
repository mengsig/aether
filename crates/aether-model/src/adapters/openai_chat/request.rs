//! Canonical request → OpenAI Chat Completions body.

use aether_core::model::{
    CapabilityProfile, ContentBlock, Effort, Message, MessageRole, ModelRequest, ToolChoice,
    ToolResultBlock,
};
use serde_json::{Value, json};

use crate::adapters::merge_extra_body;
use crate::registry::ProviderFlags;

/// Fail before sending an unsupported GPT-6 tool request; never silently drop
/// tools, lower effort, or switch the operator's configured endpoint.
pub(super) fn validate_tool_compatibility(
    body: &Value,
) -> Result<(), aether_core::model::ModelError> {
    let has_tools = body["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty());
    let incompatible = match body["model"].as_str() {
        // GPT-6.1 Sol has no `none` effort, so Chat tools are never allowed.
        Some("gpt-6-astra" | "gpt-6.1-sol") => has_tools,
        Some("gpt-6-sol" | "gpt-6-luna") => has_tools && body["reasoning_effort"] != "none",
        _ => false,
    };
    if incompatible {
        return Err(aether_core::model::ModelError::new(
            aether_core::model::ModelErrorClass::InvalidRequest,
            "GPT-6 reasoning with tools requires Responses: use openai-responses/<model> (or codex/<model> for the native harness)",
        ));
    }
    Ok(())
}

#[test]
fn gpt6_chat_tools_require_the_documented_endpoint_or_effort() {
    for model in ["gpt-6-astra", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"] {
        for effort in [None, Some("medium"), Some("none")] {
            let mut body = json!({"model": model, "tools": [{"type": "function"}]});
            if let Some(effort) = effort {
                body["reasoning_effort"] = json!(effort);
            }
            assert_eq!(
                validate_tool_compatibility(&body).is_ok(),
                matches!(model, "gpt-6-sol" | "gpt-6-luna") && effort == Some("none"),
                "{model} {effort:?}"
            );
        }
        assert!(validate_tool_compatibility(&json!({"model":model})).is_ok());
    }
    assert!(validate_tool_compatibility(&json!({"model":"gpt-5.6-sol", "tools":[{}]})).is_ok());
}

pub fn build(req: &ModelRequest, profile: &CapabilityProfile, flags: &ProviderFlags) -> Value {
    let mut body = json!({
        "model": req.model.model,
        "stream": true,
        "messages": build_messages(req),
    });
    let obj = body
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("json! object"));

    if flags.stream_options {
        obj.insert("stream_options".into(), json!({"include_usage": true}));
    }

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut function = json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                });
                if t.strict && profile.strict_tools {
                    function["strict"] = Value::Bool(true);
                }
                json!({"type": "function", "function": function})
            })
            .collect();
        obj.insert("tools".into(), Value::Array(tools));
        obj.insert(
            "tool_choice".into(),
            match req.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
            },
        );
        if !profile.parallel_tools {
            obj.insert("parallel_tool_calls".into(), Value::Bool(false));
        }
    }

    if let Some(max) = req.max_output_tokens {
        let key = if flags.max_completion_tokens {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        obj.insert(key.into(), json!(max));
    }
    if profile.effort
        && let Some(effort) = req.effort
    {
        obj.insert("reasoning_effort".into(), json!(reasoning_effort(effort)));
    }
    if let Some(t) = req.temperature {
        obj.insert("temperature".into(), json!(t));
    }
    if !req.stop_sequences.is_empty() {
        obj.insert("stop".into(), json!(req.stop_sequences));
    }
    if let Some(user) = req.metadata.get("user_id") {
        obj.insert("user".into(), json!(user));
    }

    for (key, value) in &req.extensions {
        match key.as_str() {
            "openai.extra_body" => {
                if let Some(extra) = value.as_object() {
                    merge_extra_body(obj, extra, &["model", "stream", "messages"]);
                }
            }
            other if other.starts_with("openai.") => {
                tracing::warn!(extension = other, "ignoring unknown openai extension");
            }
            _ => {}
        }
    }

    body
}

/// OpenAI-compatible servers accept low/medium/high; the higher Aether
/// levels collapse to `high` rather than failing on servers that lack them.
fn reasoning_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::XHigh | Effort::Max => "high",
    }
}

fn build_messages(req: &ModelRequest) -> Vec<Value> {
    let mut out = Vec::with_capacity(req.messages.len() + 1);
    let system: String = req
        .system
        .iter()
        .filter(|b| !b.text.is_empty())
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    for msg in &req.messages {
        match msg.role {
            MessageRole::User => push_user(&mut out, msg),
            MessageRole::Assistant => push_assistant(&mut out, msg),
        }
    }
    out
}

/// Tool results become `tool` messages; remaining text and images become
/// one `user` message.
fn push_user(out: &mut Vec<Value>, msg: &Message) {
    let mut parts: Vec<Value> = Vec::new();
    let mut only_text = true;
    for block in &msg.content {
        match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut text = content
                    .iter()
                    .filter_map(|c| match c {
                        ToolResultBlock::Text { text } => Some(text.as_str()),
                        ToolResultBlock::Image { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if *is_error && !text.to_lowercase().starts_with("error") {
                    text = format!("Error: {text}");
                }
                out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": text}));
            }
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    parts.push(json!({"type": "text", "text": text}));
                }
            }
            ContentBlock::Image { media_type, data } => {
                only_text = false;
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{media_type};base64,{data}")}
                }));
            }
            // Inlined by the runtime before a request; see the Anthropic adapter.
            ContentBlock::ImageRef(image) => {
                parts.push(json!({"type": "text", "text": image.label()}));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if parts.is_empty() {
        return;
    }
    let content = if only_text {
        let joined = parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        json!(joined)
    } else {
        Value::Array(parts)
    };
    out.push(json!({"role": "user", "content": content}));
}

fn push_assistant(out: &mut Vec<Value>, msg: &Message) {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in &msg.content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": input.to_string()}
            })),
            // Reasoning cannot be replayed portably across OpenAI-compatible servers.
            ContentBlock::Thinking { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ImageRef(_) => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return;
    }
    let mut m = json!({"role": "assistant"});
    m["content"] = if text.is_empty() {
        Value::Null
    } else {
        json!(text)
    };
    if !tool_calls.is_empty() {
        m["tool_calls"] = Value::Array(tool_calls);
    }
    out.push(m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::{ModelRef, SystemBlock, ToolSpec};
    use pretty_assertions::assert_eq;

    #[test]
    fn request_shape_with_tools_and_results() {
        let mut req = ModelRequest::new(
            ModelRef::new("openai", "gpt-5"),
            vec![
                Message::user_text("hi"),
                Message::assistant_blocks(vec![
                    ContentBlock::text("checking"),
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "echo".into(),
                        input: json!({"x": 1}),
                    },
                ]),
                Message::user_blocks(vec![
                    ContentBlock::tool_result_text("call_1", "boom", true),
                    ContentBlock::text("continue"),
                ]),
            ],
        );
        req.system = vec![SystemBlock {
            text: "sys".into(),
            cache: true,
        }];
        req.tools = vec![ToolSpec {
            name: "echo".into(),
            description: "Echo".into(),
            input_schema: json!({"type": "object"}),
            strict: false,
        }];
        req.effort = Some(Effort::Max);
        req.max_output_tokens = Some(500);
        req.extensions
            .insert("openai.extra_body".into(), json!({"top_p": 0.9}));
        let profile = CapabilityProfile {
            effort: true,
            ..Default::default()
        };
        let flags = ProviderFlags {
            max_completion_tokens: true,
            ..Default::default()
        };
        let body = build(&req, &profile, &flags);
        let expected = json!({
            "model": "gpt-5",
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "checking", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "echo", "arguments": "{\"x\":1}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "Error: boom"},
                {"role": "user", "content": "continue"}
            ],
            "tools": [{"type": "function", "function": {"name": "echo", "description": "Echo", "parameters": {"type": "object"}}}],
            "tool_choice": "auto",
            "max_completion_tokens": 500,
            "reasoning_effort": "high",
            "top_p": 0.9
        });
        assert_eq!(body, expected);
    }

    #[test]
    fn images_become_parts_and_local_flags_omit_stream_options() {
        let req = ModelRequest::new(
            ModelRef::new("ollama", "llava"),
            vec![Message::user_blocks(vec![
                ContentBlock::text("what is this"),
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "AAA".into(),
                },
            ])],
        );
        let flags = ProviderFlags {
            stream_options: false,
            ..Default::default()
        };
        let body = build(&req, &CapabilityProfile::default(), &flags);
        assert!(body.get("stream_options").is_none());
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            json!("data:image/png;base64,AAA")
        );
    }
}
