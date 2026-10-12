//! Canonical request → Anthropic Messages API body.

use aether_core::model::{
    CapabilityProfile, ContentBlock, Effort, Message, MessageRole, ModelError, ModelErrorClass,
    ModelRequest, ThinkingMode, ThinkingSupport, ToolChoice, ToolResultBlock,
};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};

use crate::adapters::merge_extra_body;

pub const API_VERSION: &str = "2023-06-01";
/// Beta required when authenticating with an OAuth bearer token.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
const DEFAULT_MAX_TOKENS: u32 = 16_000;
const DEFAULT_THINKING_BUDGET: u32 = 8_192;

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltRequest {
    pub body: Value,
    /// Values for the `anthropic-beta` header, in order.
    pub betas: Vec<String>,
}

pub fn build(req: &ModelRequest, profile: &CapabilityProfile) -> Result<BuiltRequest, ModelError> {
    let max_tokens = req.max_output_tokens.unwrap_or_else(|| {
        if profile.max_output_tokens > 0 {
            DEFAULT_MAX_TOKENS.min(profile.max_output_tokens)
        } else {
            DEFAULT_MAX_TOKENS
        }
    });

    let mut body = json!({
        "model": req.model.model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": build_messages(&req.messages)?,
    });
    let obj = body
        .as_object_mut()
        .ok_or_else(|| internal("body is not an object"))?;

    if !req.system.is_empty() {
        let system: Vec<Value> = req
            .system
            .iter()
            .filter(|b| !b.text.is_empty())
            .map(|b| {
                let mut block = json!({"type": "text", "text": b.text});
                if b.cache && profile.prompt_caching {
                    block["cache_control"] = json!({"type": "ephemeral"});
                }
                block
            })
            .collect();
        if !system.is_empty() {
            obj.insert("system".into(), Value::Array(system));
        }
    }

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut tool = json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                });
                if t.strict && profile.strict_tools {
                    tool["strict"] = Value::Bool(true);
                }
                tool
            })
            .collect();
        obj.insert("tools".into(), Value::Array(tools));
        let mut choice = match req.tool_choice {
            ToolChoice::Auto => json!({"type": "auto"}),
            ToolChoice::None => json!({"type": "none"}),
        };
        if !profile.parallel_tools && req.tool_choice == ToolChoice::Auto {
            choice["disable_parallel_tool_use"] = Value::Bool(true);
        }
        obj.insert("tool_choice".into(), choice);
    }

    if let Some(thinking) = thinking_param(req.thinking, profile, max_tokens) {
        obj.insert("thinking".into(), thinking);
    }

    if profile.effort
        && let Some(effort) = req.effort
    {
        obj.insert(
            "output_config".into(),
            json!({"effort": effort_str(effort)}),
        );
    }

    if let Some(t) = req.temperature {
        obj.insert("temperature".into(), json!(t));
    }
    if !req.stop_sequences.is_empty() {
        obj.insert("stop_sequences".into(), json!(req.stop_sequences));
    }
    if let Some(user) = req.metadata.get("user_id") {
        obj.insert("metadata".into(), json!({"user_id": user}));
    }

    let mut betas = Vec::new();
    for (key, value) in &req.extensions {
        match key.as_str() {
            "anthropic.betas" => {
                if let Some(list) = value.as_array() {
                    betas.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
                } else if let Some(s) = value.as_str() {
                    betas.push(s.to_owned());
                }
            }
            "anthropic.fallbacks" => {
                obj.insert("fallbacks".into(), value.clone());
            }
            "anthropic.thinking_display" => {
                if let Some(thinking) = obj.get_mut("thinking") {
                    thinking["display"] = value.clone();
                }
            }
            "anthropic.extra_body" => {
                if let Some(extra) = value.as_object() {
                    merge_extra_body(obj, extra, &["model", "stream", "messages"]);
                }
            }
            other if other.starts_with("anthropic.") => {
                tracing::warn!(extension = other, "ignoring unknown anthropic extension");
            }
            _ => {}
        }
    }

    Ok(BuiltRequest { body, betas })
}

/// Choose the thinking parameter from the request and the profile. Never
/// sends `budget_tokens` to an adaptive-only model and never sends
/// `disabled`, which some models reject; omission is the universal "off".
fn thinking_param(
    mode: Option<ThinkingMode>,
    profile: &CapabilityProfile,
    max_tokens: u32,
) -> Option<Value> {
    match (profile.thinking, mode) {
        (ThinkingSupport::None, _) => None,
        (_, Some(ThinkingMode::Off)) => None,
        (ThinkingSupport::Adaptive, _) => Some(json!({"type": "adaptive"})),
        (ThinkingSupport::Budget, Some(ThinkingMode::Budget { tokens })) => {
            Some(json!({"type": "enabled", "budget_tokens": budget_within(tokens, max_tokens)}))
        }
        (ThinkingSupport::Budget, Some(ThinkingMode::Adaptive)) => Some(
            json!({"type": "enabled", "budget_tokens": budget_within(DEFAULT_THINKING_BUDGET, max_tokens)}),
        ),
        (ThinkingSupport::Budget, None) => None,
    }
}

/// Preserve the 1024 floor; final validation accounts for overrides and betas.
fn budget_within(budget: u32, max_tokens: u32) -> u32 {
    budget.max(1024).min(max_tokens.saturating_sub(1).max(1024))
}

/// Validate after body overrides and configured beta headers have been merged.
pub(super) fn validate_thinking(body: &Value, headers: &HeaderMap) -> Result<(), ModelError> {
    let thinking = &body["thinking"];
    if thinking["type"] != "enabled" {
        return Ok(());
    }
    let (Some(budget), Some(max_tokens)) = (
        thinking["budget_tokens"].as_u64(),
        body["max_tokens"].as_u64(),
    ) else {
        return Ok(());
    };
    let interleaved = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|betas| {
            betas
                .split(',')
                .any(|beta| beta.trim() == "interleaved-thinking-2025-05-14")
        });
    if budget >= max_tokens && !interleaved {
        return Err(ModelError::new(
            ModelErrorClass::InvalidRequest,
            "Anthropic budget_tokens must be below max_tokens; budget thinking requires more than 1024 output tokens",
        ));
    }
    Ok(())
}

fn effort_str(effort: Effort) -> &'static str {
    effort.as_str()
}

fn build_messages(messages: &[Message]) -> Result<Vec<Value>, ModelError> {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        let blocks: Vec<Value> = msg.content.iter().filter_map(block_to_json).collect();
        if blocks.is_empty() {
            match msg.role {
                // An assistant turn made only of unsigned thinking has nothing to replay.
                MessageRole::Assistant => continue,
                MessageRole::User => {
                    return Err(ModelError::new(
                        ModelErrorClass::InvalidRequest,
                        format!("message {i} (user) has no content after normalization"),
                    ));
                }
            }
        }
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        // The API requires alternating roles; merge adjacent same-role turns.
        if let Some(last) = out.last_mut()
            && last["role"] == role
            && let Some(arr) = last["content"].as_array_mut()
        {
            arr.extend(blocks);
            continue;
        }
        out.push(json!({"role": role, "content": blocks}));
    }
    Ok(out)
}

fn block_to_json(block: &ContentBlock) -> Option<Value> {
    match block {
        ContentBlock::Text { text } => {
            if text.is_empty() {
                None
            } else {
                Some(json!({"type": "text", "text": text}))
            }
        }
        ContentBlock::Thinking {
            text,
            signature,
            redacted,
        } => {
            if *redacted {
                Some(
                    json!({"type": "redacted_thinking", "data": signature.clone().unwrap_or_default()}),
                )
            } else {
                signature
                    .as_ref()
                    .map(|sig| json!({"type": "thinking", "thinking": text, "signature": sig}))
            }
        }
        ContentBlock::ToolUse { id, name, input } => {
            Some(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(tool_result_json(tool_use_id, content, *is_error)),
        ContentBlock::Image { media_type, data } => Some(image_json(media_type, data)),
        // The runtime inlines stored images before a request; one that
        // reaches here has no bytes, so the model is told what was there.
        ContentBlock::ImageRef(image) => Some(json!({"type": "text", "text": image.label()})),
    }
}

fn image_json(media_type: &str, data: &str) -> Value {
    json!({
        "type": "image",
        "source": {"type": "base64", "media_type": media_type, "data": data}
    })
}

fn tool_result_json(tool_use_id: &str, content: &[ToolResultBlock], is_error: bool) -> Value {
    let parts: Vec<Value> = content
        .iter()
        .map(|c| match c {
            ToolResultBlock::Text { text } => json!({"type": "text", "text": text}),
            ToolResultBlock::Image { media_type, data } => image_json(media_type, data),
        })
        .collect();
    let mut v = json!({"type": "tool_result", "tool_use_id": tool_use_id});
    v["content"] = if parts.is_empty() {
        json!("")
    } else {
        Value::Array(parts)
    };
    if is_error {
        v["is_error"] = Value::Bool(true);
    }
    v
}

fn internal(msg: &str) -> ModelError {
    ModelError::new(ModelErrorClass::Unknown, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::{ModelRef, SystemBlock, ToolSpec};
    use pretty_assertions::assert_eq;

    fn adaptive_profile() -> CapabilityProfile {
        CapabilityProfile {
            thinking: ThinkingSupport::Adaptive,
            effort: true,
            prompt_caching: true,
            strict_tools: true,
            max_output_tokens: 128_000,
            ..Default::default()
        }
    }

    #[test]
    fn full_request_shape() {
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "claude-opus-5-5"),
            vec![
                Message::user_text("hi"),
                Message::assistant_blocks(vec![
                    ContentBlock::Thinking {
                        text: "hmm".into(),
                        signature: Some("sig".into()),
                        redacted: false,
                    },
                    ContentBlock::Thinking {
                        text: "unsigned".into(),
                        signature: None,
                        redacted: false,
                    },
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "echo".into(),
                        input: json!({"x": 1}),
                    },
                ]),
                Message::user_blocks(vec![ContentBlock::tool_result_text("t1", "ok", false)]),
            ],
        );
        req.system = vec![
            SystemBlock {
                text: "policy".into(),
                cache: true,
            },
            SystemBlock {
                text: "task".into(),
                cache: false,
            },
        ];
        req.tools = vec![ToolSpec {
            name: "echo".into(),
            description: "Echo".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false, "required": []}),
            strict: true,
        }];
        req.effort = Some(Effort::XHigh);
        req.thinking = Some(ThinkingMode::Adaptive);
        req.extensions
            .insert("anthropic.betas".into(), json!(["beta-a", "beta-b"]));
        req.extensions
            .insert("anthropic.fallbacks".into(), json!("default"));
        req.metadata.insert("user_id".into(), "u1".into());

        let built = build(&req, &adaptive_profile()).expect("build");
        assert_eq!(built.betas, vec!["beta-a", "beta-b"]);
        let expected = json!({
            "model": "claude-opus-5-5",
            "max_tokens": 16000,
            "stream": true,
            "system": [
                {"type": "text", "text": "policy", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "task"}
            ],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "tool_use", "id": "t1", "name": "echo", "input": {"x": 1}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "ok"}]}
                ]}
            ],
            "tools": [{
                "name": "echo", "description": "Echo",
                "input_schema": {"type": "object", "properties": {}, "additionalProperties": false, "required": []},
                "strict": true
            }],
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "xhigh"},
            "metadata": {"user_id": "u1"},
            "fallbacks": "default"
        });
        assert_eq!(built.body, expected);
    }

    #[test]
    fn budget_model_gets_enabled_thinking_and_no_effort() {
        let profile = CapabilityProfile {
            thinking: ThinkingSupport::Budget,
            max_output_tokens: 64_000,
            ..Default::default()
        };
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "budget-model"),
            vec![Message::user_text("x")],
        );
        req.thinking = Some(ThinkingMode::Budget { tokens: 4000 });
        req.effort = Some(Effort::High);
        let body = build(&req, &profile).expect("build").body;
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 4000})
        );
        assert!(body.get("output_config").is_none());

        req.thinking = Some(ThinkingMode::Budget { tokens: 100_000 });
        req.max_output_tokens = Some(2048);
        let body = build(&req, &profile).expect("build").body;
        assert_eq!(body["thinking"]["budget_tokens"], json!(2047));
    }

    #[test]
    fn thinking_off_and_unsupported_models_omit_the_parameter() {
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![Message::user_text("x")],
        );
        req.thinking = Some(ThinkingMode::Off);
        assert!(
            build(&req, &adaptive_profile())
                .expect("build")
                .body
                .get("thinking")
                .is_none()
        );
        req.thinking = Some(ThinkingMode::Adaptive);
        assert!(
            build(&req, &CapabilityProfile::default())
                .expect("build")
                .body
                .get("thinking")
                .is_none()
        );
        // Unspecified on an adaptive model defaults to adaptive.
        req.thinking = None;
        assert_eq!(
            build(&req, &adaptive_profile()).expect("build").body["thinking"],
            json!({"type": "adaptive"})
        );
    }

    #[test]
    fn budget_thinking_rejects_output_limits_that_cannot_fit_the_minimum() {
        let profile = CapabilityProfile {
            thinking: ThinkingSupport::Budget,
            ..Default::default()
        };
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![Message::user_text("x")],
        );
        for mode in [ThinkingMode::Budget { tokens: 1 }, ThinkingMode::Adaptive] {
            req.thinking = Some(mode);
            for max_tokens in [0, 1, 1023, 1024] {
                req.max_output_tokens = Some(max_tokens);
                let body = build(&req, &profile).unwrap().body;
                let err = validate_thinking(&body, &HeaderMap::new()).unwrap_err();
                assert_eq!(err.class, ModelErrorClass::InvalidRequest);
                assert!(err.message.contains("1024"));
            }
            req.max_output_tokens = Some(1025);
            let body = build(&req, &profile).expect("minimum valid budget").body;
            assert_eq!(body["thinking"]["budget_tokens"], json!(1024));
            validate_thinking(&body, &HeaderMap::new()).unwrap();
        }

        req.max_output_tokens = Some(1024);
        for mode in [None, Some(ThinkingMode::Off)] {
            req.thinking = mode;
            assert!(
                build(&req, &profile)
                    .unwrap()
                    .body
                    .get("thinking")
                    .is_none()
            );
        }
        req.thinking = Some(ThinkingMode::Adaptive);
        assert_eq!(
            build(&req, &adaptive_profile()).unwrap().body["thinking"],
            json!({"type": "adaptive"})
        );
    }

    #[test]
    fn thinking_validation_uses_final_body_overrides() {
        let profile = CapabilityProfile {
            thinking: ThinkingSupport::Budget,
            ..Default::default()
        };
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![Message::user_text("x")],
        );
        req.thinking = Some(ThinkingMode::Budget { tokens: 1024 });
        req.max_output_tokens = Some(1024);
        req.extensions
            .insert("anthropic.extra_body".into(), json!({"max_tokens": 2048}));
        let body = build(&req, &profile).unwrap().body;
        assert_eq!(body["max_tokens"], json!(2048));
        validate_thinking(&body, &HeaderMap::new()).unwrap();

        req.extensions.insert(
            "anthropic.extra_body".into(),
            json!({"thinking": {"type": "disabled"}}),
        );
        let body = build(&req, &profile).unwrap().body;
        validate_thinking(&body, &HeaderMap::new()).unwrap();

        req.max_output_tokens = Some(2048);
        req.extensions
            .insert("anthropic.extra_body".into(), json!({"max_tokens": 512}));
        let body = build(&req, &profile).unwrap().body;
        assert_eq!(
            validate_thinking(&body, &HeaderMap::new())
                .unwrap_err()
                .class,
            ModelErrorClass::InvalidRequest
        );
    }

    #[test]
    fn adjacent_same_role_messages_merge_and_empty_user_is_an_error() {
        let req = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![Message::user_text("a"), Message::user_text("b")],
        );
        let body = build(&req, &CapabilityProfile::default())
            .expect("build")
            .body;
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body["messages"][0]["content"].as_array().map(Vec::len),
            Some(2)
        );

        let bad = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![Message::user_text("")],
        );
        assert_eq!(
            build(&bad, &CapabilityProfile::default())
                .unwrap_err()
                .class,
            ModelErrorClass::InvalidRequest
        );
    }

    #[test]
    fn redacted_thinking_is_replayed_as_data() {
        let req = ModelRequest::new(
            ModelRef::new("anthropic", "m"),
            vec![
                Message::user_text("a"),
                Message::assistant_blocks(vec![
                    ContentBlock::Thinking {
                        text: String::new(),
                        signature: Some("enc".into()),
                        redacted: true,
                    },
                    ContentBlock::text("done"),
                ]),
            ],
        );
        let body = build(&req, &CapabilityProfile::default())
            .expect("build")
            .body;
        assert_eq!(
            body["messages"][1]["content"][0],
            json!({"type": "redacted_thinking", "data": "enc"})
        );
    }
}
