//! Anthropic Messages API adapter over raw HTTP with SSE streaming.

pub mod request;
pub mod stream;

use aether_core::model::{ContentBlock, MessageRole, ModelError, ModelErrorClass, ModelRequest};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use crate::adapters::streaming::sse_stream;
use crate::adapters::{AdapterConfig, RequestAuth, shadows_credential};
use crate::client::{ModelClient, ModelStream};
use crate::http::{self, map_http_error, map_transport_error, read_error_body};

pub const ADAPTER: &str = "anthropic-messages/1";

#[derive(Debug)]
pub struct AnthropicClient {
    http: reqwest::Client,
    config: AdapterConfig,
}

impl AnthropicClient {
    pub fn new(config: AdapterConfig) -> Result<Self, ModelError> {
        let http = http::build_client(&config.user_agent)?;
        Ok(Self { http, config })
    }

    pub fn with_http(config: AdapterConfig, http: reqwest::Client) -> Self {
        Self { http, config }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }

    fn headers(&self, betas: &[String]) -> Result<HeaderMap, ModelError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(request::API_VERSION),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        let mut all_betas: Vec<String> = betas.to_vec();
        match &self.config.auth {
            RequestAuth::ApiKey(key) => {
                headers.insert("x-api-key", header_value(key)?);
            }
            RequestAuth::Bearer(token) => {
                headers.insert("authorization", header_value(&format!("Bearer {token}"))?);
                all_betas.push(request::OAUTH_BETA.to_owned());
            }
            RequestAuth::None => {}
        }
        for (k, v) in &self.config.headers {
            if k.eq_ignore_ascii_case("anthropic-beta") {
                all_betas.extend(
                    v.split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty()),
                );
                continue;
            }
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| {
                ModelError::new(
                    aether_core::model::ModelErrorClass::InvalidRequest,
                    format!("bad header {k}: {e}"),
                )
            })?;
            if shadows_credential(&headers, &name) {
                tracing::warn!(header = %name, "ignoring a configured header that would replace the resolved credential");
                continue;
            }
            headers.insert(name, header_value(v)?);
        }
        all_betas.dedup();
        if !all_betas.is_empty() {
            headers.insert("anthropic-beta", header_value(&all_betas.join(","))?);
        }
        Ok(headers)
    }

    async fn send(
        &self,
        built: &request::BuiltRequest,
        headers: HeaderMap,
        cancel: &CancellationToken,
        retry: http::RetryPolicy,
    ) -> Result<reqwest::Response, ModelError> {
        request::validate_thinking(&built.body, &headers)?;
        let url = self.endpoint();
        let body = serde_json::to_vec(&built.body).map_err(|e| {
            ModelError::new(aether_core::model::ModelErrorClass::Unknown, e.to_string())
        })?;
        http::retry(&retry, cancel, |_| {
            let req = self
                .http
                .post(&url)
                .headers(headers.clone())
                .body(body.clone());
            let cancel = cancel.clone();
            async move {
                let resp = tokio::select! {
                    r = req.send() => r.map_err(map_transport_error)?,
                    _ = cancel.cancelled() => return Err(http::cancelled()),
                };
                if resp.status().is_success() {
                    return Ok(resp);
                }
                let code = resp.status().as_u16();
                let headers = resp.headers().clone();
                let text = read_error_body(resp, &cancel).await?;
                Err(map_http_error(code, &headers, &text))
            }
        })
        .await
    }

    /// The one fallback attempt: no thinking blocks, no thinking parameter and
    /// no binding beta. It never retries again, whatever it returns.
    async fn send_without_thinking(
        &self,
        request: &ModelRequest,
        profile: &aether_core::model::CapabilityProfile,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, ModelError> {
        let mut stripped = request.clone();
        strip_thinking(&mut stripped);
        let mut fallback = request::build(&stripped, profile)?;
        let body = fallback.body.as_object_mut().ok_or_else(|| {
            ModelError::new(
                ModelErrorClass::Unknown,
                "Anthropic request body is not an object",
            )
        })?;
        body.remove("thinking");
        fallback
            .betas
            .retain(|beta| beta != request::THINKING_BINDING_BETA);
        debug_assert!(!request::body_replays_thinking(body));
        let mut headers = self.headers(&fallback.betas)?;
        remove_binding_header(&mut headers)?;
        self.send(&fallback, headers, cancel, http::RetryPolicy::none())
            .await
    }
}

fn strip_thinking(request: &mut ModelRequest) {
    for message in &mut request.messages {
        message
            .content
            .retain(|block| !matches!(block, ContentBlock::Thinking { .. }));
    }
    // Drop emptied assistant turns: the body builder merges the user turns around them, so roles still alternate.
    request
        .messages
        .retain(|message| message.role != MessageRole::Assistant || !message.content.is_empty());
    debug_assert!(request.messages.iter().all(|message| {
        !message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Thinking { .. }))
    }));
}

/// The 400s the API returns for a replayed thinking block it can no longer
/// verify: a bad or foreign signature, or an edit to a bound block.
fn is_thinking_mismatch(error: &ModelError) -> bool {
    let typed = error
        .provider_code
        .as_deref()
        .is_none_or(|code| code == "invalid_request_error");
    let message = error.message.as_str();
    let mismatch = message.contains("Invalid `signature` in `thinking` block")
        || (message.contains("`thinking`") && message.contains("cannot be modified"));
    typed && mismatch
}

/// Whether a failed request earns its single retry without preserved thinking.
fn should_strip_thinking(error: &ModelError, built: &request::BuiltRequest) -> bool {
    if error.status != Some(400) || error.class != ModelErrorClass::InvalidRequest {
        return false;
    }
    let sent_beta = built
        .betas
        .iter()
        .any(|beta| beta == request::THINKING_BINDING_BETA);
    let beta_rejected = sent_beta && error.message.contains(request::THINKING_BINDING_BETA);
    beta_rejected
        || (is_thinking_mismatch(error)
            && built
                .body
                .as_object()
                .is_some_and(request::body_replays_thinking))
}

fn remove_binding_header(headers: &mut HeaderMap) -> Result<(), ModelError> {
    let Some(value) = headers.get("anthropic-beta") else {
        return Ok(());
    };
    let value = value.to_str().map_err(|e| {
        ModelError::new(
            aether_core::model::ModelErrorClass::InvalidRequest,
            format!("invalid beta header: {e}"),
        )
    })?;
    let remaining = value
        .split(',')
        .map(str::trim)
        .filter(|beta| *beta != request::THINKING_BINDING_BETA)
        .collect::<Vec<_>>()
        .join(",");
    if remaining.is_empty() {
        headers.remove("anthropic-beta");
    } else {
        headers.insert("anthropic-beta", header_value(&remaining)?);
    }
    Ok(())
}

fn header_value(v: &str) -> Result<HeaderValue, ModelError> {
    HeaderValue::from_str(v).map_err(|e| {
        ModelError::new(
            aether_core::model::ModelErrorClass::InvalidRequest,
            format!("invalid header value: {e}"),
        )
    })
}

#[async_trait::async_trait]
impl ModelClient for AnthropicClient {
    fn adapter(&self) -> &str {
        ADAPTER
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let profile = self.config.profile_for(&request.model.model);
        let built = request::build(&request, &profile)?;
        let headers = self.headers(&built.betas)?;
        let retry = if built.body.get("compaction").is_some() {
            http::RetryPolicy::none()
        } else {
            self.config.retry
        };
        let response = match self.send(&built, headers, &cancel, retry).await {
            Err(error) if should_strip_thinking(&error, &built) => {
                tracing::warn!(error = %error.message, "retrying once without preserved thinking");
                self.send_without_thinking(&request, &profile, &cancel)
                    .await?
            }
            other => other?,
        };

        Ok(sse_stream(response, stream::AnthropicMapper::new(), cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::{CapabilityProfile, Message, ModelRef, ThinkingSupport};
    use serde_json::json;

    const SIGNATURE_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation."},"request_id":"req_011CVsigmismatch"}"#;
    const MODIFIED_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.1.content.0: `thinking` or `redacted_thinking` blocks in the latest assistant message cannot be modified. These blocks must remain as they were in the original response."},"request_id":"req_011CVmodified"}"#;
    const BETA_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Unexpected value(s) `thinking-binding-controls-2026-08-01` for the `anthropic-beta` header."}}"#;
    const UNRELATED_400: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.0.content.0.text: text content blocks must be non-empty"}}"#;

    fn http_400(body: &str) -> ModelError {
        crate::http::map_http_error(400, &HeaderMap::new(), body)
    }

    fn fable_request(thinking_signature: Option<&str>) -> ModelRequest {
        ModelRequest::new(
            ModelRef::new("anthropic", "claude-fable-5-2"),
            vec![
                Message::user_text("hi"),
                Message::assistant_blocks(vec![
                    ContentBlock::Thinking {
                        text: "thought".into(),
                        signature: thinking_signature.map(str::to_owned),
                        redacted: false,
                    },
                    ContentBlock::text("answer"),
                ]),
                Message::user_text("continue"),
            ],
        )
    }

    fn adaptive() -> CapabilityProfile {
        CapabilityProfile {
            thinking: ThinkingSupport::Adaptive,
            ..Default::default()
        }
    }

    #[test]
    fn compaction_request_sends_no_binding_body_or_header() {
        let client = AnthropicClient::new(AdapterConfig::new(
            "http://127.0.0.1:9",
            RequestAuth::ApiKey("key".into()),
        ))
        .unwrap();
        let mut req = fable_request(None);
        req.extensions
            .insert("anthropic.compaction".into(), json!({"type": "summarize"}));
        let built = request::build(&req, &adaptive()).unwrap();
        let headers = client.headers(&built.betas).unwrap();
        assert_eq!(headers["anthropic-beta"], request::SUMMARIZE_BETA);
        assert_eq!(built.body["compaction"], json!({"type": "summarize"}));
        assert_eq!(built.body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(
            built.body["messages"][1]["content"],
            json!([{"type": "text", "text": "answer"}]),
            "the unsigned thinking block is not replayed"
        );
    }

    #[test]
    fn thinking_mismatch_400s_trigger_the_retry_only_when_thinking_is_replayed() {
        let replaying = request::build(&fable_request(Some("sig")), &adaptive()).unwrap();
        let mut without_thinking = fable_request(Some("sig"));
        strip_thinking(&mut without_thinking);
        let plain = request::build(&without_thinking, &adaptive()).unwrap();
        for body in [SIGNATURE_400, MODIFIED_400] {
            let error = http_400(body);
            assert!(is_thinking_mismatch(&error), "{body}");
            assert!(should_strip_thinking(&error, &replaying), "{body}");
            assert!(!should_strip_thinking(&error, &plain), "{body}");
        }
        // A rejected beta retries whether or not thinking is replayed, since the beta rode on both.
        assert!(should_strip_thinking(&http_400(BETA_400), &replaying));
        assert!(should_strip_thinking(&http_400(BETA_400), &plain));
        let unrelated = http_400(UNRELATED_400);
        assert!(!is_thinking_mismatch(&unrelated));
        assert!(!should_strip_thinking(&unrelated, &replaying));
        let server = crate::http::map_http_error(500, &HeaderMap::new(), SIGNATURE_400);
        assert!(!should_strip_thinking(&server, &replaying));
    }

    #[test]
    fn stripping_drops_an_assistant_turn_left_empty() {
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "claude-fable-5-2"),
            vec![
                Message::user_text("hi"),
                Message::assistant_blocks(vec![ContentBlock::Thinking {
                    text: String::new(),
                    signature: Some("sig".into()),
                    redacted: false,
                }]),
                Message::user_text("continue"),
            ],
        );
        strip_thinking(&mut req);
        assert_eq!(req.messages.len(), 2);
        assert!(req.messages.iter().all(|m| m.role == MessageRole::User));
        let built = request::build(&req, &adaptive()).unwrap();
        assert_eq!(
            built.body["messages"],
            json!([{"role": "user", "content": [
                {"type": "text", "text": "hi"},
                {"type": "text", "text": "continue"}
            ]}])
        );
    }

    #[test]
    fn fallback_strips_thinking_and_only_the_binding_beta() {
        let mut req = ModelRequest::new(
            ModelRef::new("anthropic", "claude-fable-5-2"),
            vec![
                Message::assistant_blocks(vec![
                    ContentBlock::Thinking {
                        text: "private".into(),
                        signature: Some("signature".into()),
                        redacted: false,
                    },
                    ContentBlock::text("answer"),
                ]),
                Message::user_text("continue"),
            ],
        );
        strip_thinking(&mut req);
        assert_eq!(req.messages[0].content, vec![ContentBlock::text("answer")]);
        assert_eq!(
            req.messages[1].content,
            vec![ContentBlock::text("continue")]
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("other,thinking-binding-controls-2026-08-01,third"),
        );
        remove_binding_header(&mut headers).unwrap();
        assert_eq!(headers["anthropic-beta"], "other,third");
        headers.insert(
            "anthropic-beta",
            HeaderValue::from_static("thinking-binding-controls-2026-08-01"),
        );
        remove_binding_header(&mut headers).unwrap();
        assert!(!headers.contains_key("anthropic-beta"));
    }
}
