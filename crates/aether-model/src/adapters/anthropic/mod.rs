//! Anthropic Messages API adapter over raw HTTP with SSE streaming.

pub mod request;
pub mod stream;

use aether_core::model::{ContentBlock, ModelError, ModelRequest};
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
}

fn strip_thinking(request: &mut ModelRequest) {
    for message in &mut request.messages {
        message
            .content
            .retain(|block| !matches!(block, ContentBlock::Thinking { .. }));
    }
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
        let result = self.send(&built, headers, &cancel, retry).await;
        let response = match result {
            Err(error)
                if error.status == Some(400)
                    && error.message.contains(request::THINKING_BINDING_BETA)
                    && built
                        .betas
                        .iter()
                        .any(|beta| beta == request::THINKING_BINDING_BETA) =>
            {
                let mut stripped = request.clone();
                strip_thinking(&mut stripped);
                let mut fallback = request::build(&stripped, &profile)?;
                let body = fallback.body.as_object_mut().ok_or_else(|| {
                    ModelError::new(
                        aether_core::model::ModelErrorClass::Unknown,
                        "Anthropic request body is not an object",
                    )
                })?;
                body.remove("thinking");
                fallback
                    .betas
                    .retain(|beta| beta != request::THINKING_BINDING_BETA);
                let mut headers = self.headers(&fallback.betas)?;
                remove_binding_header(&mut headers)?;
                self.send(&fallback, headers, &cancel, http::RetryPolicy::none())
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
    use aether_core::model::{Message, ModelRef};

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
