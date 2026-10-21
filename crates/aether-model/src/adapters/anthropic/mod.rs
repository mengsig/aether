//! Anthropic Messages API adapter over raw HTTP with SSE streaming.

pub mod request;
pub mod stream;

use aether_core::model::{ModelError, ModelRequest};
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
        request::validate_thinking(&built.body, &headers)?;
        let url = self.endpoint();
        let body = serde_json::to_vec(&built.body).map_err(|e| {
            ModelError::new(aether_core::model::ModelErrorClass::Unknown, e.to_string())
        })?;

        // Retries cover the phase before any content flows; once the status
        // is 2xx the stream is handed over and never restarted.
        let retry_cancel = cancel.clone();
        let retry = if built.body.get("compaction").is_some() {
            http::RetryPolicy::none()
        } else {
            self.config.retry
        };
        let response = http::retry(&retry, &cancel, |_| {
            let req = self
                .http
                .post(&url)
                .headers(headers.clone())
                .body(body.clone());
            let cancel = retry_cancel.clone();
            async move {
                let resp = tokio::select! {
                    r = req.send() => r.map_err(map_transport_error)?,
                    _ = cancel.cancelled() => return Err(http::cancelled()),
                };
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp);
                }
                let code = status.as_u16();
                let headers = resp.headers().clone();
                let text = read_error_body(resp, &cancel).await?;
                Err(map_http_error(code, &headers, &text))
            }
        })
        .await?;

        Ok(sse_stream(response, stream::AnthropicMapper::new(), cancel))
    }
}
