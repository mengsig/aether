//! Canonical request → OpenAI Responses body.

use aether_core::model::{
    CapabilityProfile, ContentBlock, Effort, Message, MessageRole, ModelRequest, ThinkingSupport,
    ToolChoice, ToolResultBlock,
};
use serde_json::{Value, json};

use super::Dialect;
use crate::adapters::merge_extra_body;

pub fn build(req: &ModelRequest, profile: &CapabilityProfile, dialect: Dialect) -> Value {
    let mut body = json!({
        "model": req.model.model,
        "stream": true,
        "input": build_input(req),
        // Aether keeps the transcript; a server-side copy of every turn is
        // not ours to create on the person's account.
        "store": false,
    });
    let obj = body
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("json! object"));

    let instructions: String = req
        .system
        .iter()
        .filter(|b| !b.text.is_empty())
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !instructions.is_empty() {
        obj.insert("instructions".into(), json!(instructions));
    }

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut tool = json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                });
                // Responses defaults strict to true; say so explicitly either way
                // so a schema the model cannot satisfy strictly is not rejected.
                tool["strict"] = Value::Bool(t.strict && profile.strict_tools);
                tool
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
        obj.insert("max_output_tokens".into(), json!(max));
    }
    if profile.thinking != ThinkingSupport::None || profile.effort {
        let mut reasoning = json!({});
        if profile.effort
            && let Some(effort) = req.effort
        {
            reasoning["effort"] = json!(reasoning_effort(effort));
        }
        // Without a summary the reasoning item streams no text at all, and the
        // person sees a silent gap where the model was thinking.
        if profile.thinking != ThinkingSupport::None {
            reasoning["summary"] = json!("auto");
        }
        if reasoning.as_object().is_some_and(|o| !o.is_empty()) {
            obj.insert("reasoning".into(), reasoning);
        }
    }
    // Reasoning models reject an explicit temperature; the profile that carries
    // effort is exactly the one where it is refused.
    if let Some(t) = req.temperature
        && !profile.effort
    {
        obj.insert("temperature".into(), json!(t));
    }
    if !req.stop_sequences.is_empty() {
        tracing::warn!(
            count = req.stop_sequences.len(),
            "openai-responses has no stop sequences; they are not sent"
        );
    }
    if let Some(user) = req.metadata.get("user_id") {
        obj.insert("safety_identifier".into(), json!(user));
    }
    if dialect == Dialect::Codex {
        // The Codex backend answers only for its own account scope and rejects
        // a stored response; both are already set above except this one.
        obj.insert("include".into(), json!([]));
    }

    for (key, value) in &req.extensions {
        match key.as_str() {
            "openai.extra_body" => {
                if let Some(extra) = value.as_object() {
                    merge_extra_body(
                        obj,
                        extra,
                        &[
                            "model",
                            "stream",
                            "store",
                            "input",
                            "instructions",
                            "include",
                        ],
                    );
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

/// Responses accepts minimal/low/medium/high; the higher Aether levels
/// collapse to `high` rather than failing on a server that lacks them.
fn reasoning_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::XHigh | Effort::Max => "high",
    }
}

fn build_input(req: &ModelRequest) -> Vec<Value> {
    let mut out = Vec::with_capacity(req.messages.len());
    for msg in &req.messages {
        match msg.role {
            MessageRole::User => push_user(&mut out, msg),
            MessageRole::Assistant => push_assistant(&mut out, msg),
        }
    }
    out
}

/// Tool results become `function_call_output` items in place; remaining text
/// and images become one `user` message after them.
fn push_user(out: &mut Vec<Value>, msg: &Message) {
    let mut parts: Vec<Value> = Vec::new();
    for block in &msg.content {
        match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => out.push(tool_output_item(tool_use_id, content, *is_error)),
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    parts.push(json!({"type": "input_text", "text": text}));
                }
            }
            ContentBlock::Image { media_type, data } => parts.push(json!({
                "type": "input_image",
                "image_url": format!("data:{media_type};base64,{data}"),
            })),
            // Inlined by the runtime before a request; see the Anthropic adapter.
            ContentBlock::ImageRef(image) => {
                parts.push(json!({"type": "input_text", "text": image.label()}));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if !parts.is_empty() {
        out.push(json!({"role": "user", "content": parts}));
    }
}

/// A tool result as a Responses input item, which carries text only.
fn tool_output_item(tool_use_id: &str, content: &[ToolResultBlock], is_error: bool) -> Value {
    let images = content
        .iter()
        .filter(|c| matches!(c, ToolResultBlock::Image { .. }))
        .count();
    if images > 0 {
        // A screenshot-returning tool must not lose its result in silence.
        tracing::warn!(
            images,
            tool_use_id,
            "openai-responses tool results carry no images; they are not sent"
        );
    }
    let mut text = content
        .iter()
        .filter_map(|c| match c {
            ToolResultBlock::Text { text } => Some(text.as_str()),
            ToolResultBlock::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if is_error
        && !text
            .get(..5)
            .is_some_and(|p| p.eq_ignore_ascii_case("error"))
    {
        text = format!("Error: {text}");
    }
    json!({
        "type": "function_call_output",
        "call_id": tool_use_id,
        "output": text,
    })
}

fn push_assistant(out: &mut Vec<Value>, msg: &Message) {
    let mut text = String::new();
    let mut calls: Vec<Value> = Vec::new();
    for block in &msg.content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => calls.push(json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                "arguments": input.to_string(),
            })),
            // A reasoning item cannot be replayed without its provider-side
            // encrypted payload, which Aether does not store.
            ContentBlock::Thinking { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ImageRef(_) => {}
        }
    }
    if !text.is_empty() {
        out.push(json!({
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}],
        }));
    }
    out.extend(calls);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::{ModelRef, SystemBlock, ToolSpec};
    use pretty_assertions::assert_eq;

    #[test]
    fn request_shape_with_tools_results_and_reasoning() {
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
            strict: true,
        }];
        req.effort = Some(Effort::Max);
        req.max_output_tokens = Some(500);
        req.temperature = Some(0.5);
        req.extensions
            .insert("openai.extra_body".into(), json!({"top_p": 0.9}));
        let profile = CapabilityProfile {
            effort: true,
            strict_tools: true,
            thinking: ThinkingSupport::Adaptive,
            ..Default::default()
        };
        let body = build(&req, &profile, Dialect::OpenAi);
        let expected = json!({
            "model": "gpt-5",
            "stream": true,
            "store": false,
            "instructions": "sys",
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"role": "assistant", "content": [{"type": "output_text", "text": "checking"}]},
                {"type": "function_call", "call_id": "call_1", "name": "echo", "arguments": "{\"x\":1}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "Error: boom"},
                {"role": "user", "content": [{"type": "input_text", "text": "continue"}]}
            ],
            "tools": [{"type": "function", "name": "echo", "description": "Echo", "parameters": {"type": "object"}, "strict": true}],
            "tool_choice": "auto",
            "max_output_tokens": 500,
            "reasoning": {"effort": "high", "summary": "auto"},
            "top_p": 0.9
        });
        assert_eq!(
            body, expected,
            "a reasoning model must not carry temperature"
        );
    }

    #[test]
    fn images_and_plain_models_keep_temperature() {
        let mut req = ModelRequest::new(
            ModelRef::new("openai", "gpt-4o"),
            vec![Message::user_blocks(vec![
                ContentBlock::text("what is this"),
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "AAA".into(),
                },
            ])],
        );
        req.temperature = Some(0.2);
        let body = build(&req, &CapabilityProfile::default(), Dialect::OpenAi);
        assert_eq!(body["temperature"], json!(0.2f32));
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            json!("data:image/png;base64,AAA")
        );
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn codex_dialect_asks_for_no_extra_output() {
        let req = ModelRequest::new(
            ModelRef::new("openai-codex", "gpt-5.3-codex"),
            vec![Message::user_text("hi")],
        );
        let body = build(&req, &CapabilityProfile::default(), Dialect::Codex);
        assert_eq!(body["include"], json!([]));
        assert_eq!(body["store"], json!(false));
    }
}
