//! OpenAI-compatible Chat Completions adapter. Covers OpenAI, OpenRouter,
//! Groq, xAI, Google's compatibility endpoint, vLLM, Ollama, LM Studio and
//! llama.cpp: anything that speaks `/v1/chat/completions`.

pub mod request;
pub mod stream;

use aether_core::model::{Effort, ModelError, ModelErrorClass, ModelRequest};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use crate::adapters::streaming::sse_stream;
use crate::adapters::{AdapterConfig, RequestAuth, shadows_credential};
use crate::client::{ModelClient, ModelStream};
use crate::http::{self, map_http_error, map_transport_error, read_error_body};

pub const ADAPTER: &str = "openai-chat-completions/1";

#[derive(Debug)]
pub struct OpenAiChatClient {
    http: reqwest::Client,
    config: AdapterConfig,
}

impl OpenAiChatClient {
    pub fn new(config: AdapterConfig) -> Result<Self, ModelError> {
        let http = http::build_client(&config.user_agent)?;
        Ok(Self { http, config })
    }

    pub fn with_http(config: AdapterConfig, http: reqwest::Client) -> Self {
        Self { http, config }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    fn headers(&self) -> Result<HeaderMap, ModelError> {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        match &self.config.auth {
            RequestAuth::ApiKey(key) | RequestAuth::Bearer(key) => {
                headers.insert("authorization", value(&format!("Bearer {key}"))?);
            }
            RequestAuth::None => {}
        }
        for (k, v) in &self.config.headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| {
                ModelError::new(
                    ModelErrorClass::InvalidRequest,
                    format!("bad header {k}: {e}"),
                )
            })?;
            if shadows_credential(&headers, &name) {
                tracing::warn!(header = %name, "ignoring a configured header that would replace the resolved credential");
                continue;
            }
            headers.insert(name, value(v)?);
        }
        Ok(headers)
    }
}

fn value(v: &str) -> Result<HeaderValue, ModelError> {
    HeaderValue::from_str(v).map_err(|e| {
        ModelError::new(
            ModelErrorClass::InvalidRequest,
            format!("invalid header value: {e}"),
        )
    })
}

#[async_trait::async_trait]
impl ModelClient for OpenAiChatClient {
    fn adapter(&self) -> &str {
        ADAPTER
    }

    fn effective_effort(
        &self,
        model: &crate::ResolvedModel,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        self.config.effective_effort(
            crate::Protocol::OpenaiChatCompletions,
            &model.model.id,
            effort,
        )
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let profile = self.config.profile_for(&request.model.model);
        let body = request::build(&request, &profile, &self.config.flags);
        request::validate_tool_compatibility(&body)?;
        let headers = self.headers()?;
        let url = self.endpoint();
        let bytes = serde_json::to_vec(&body)
            .map_err(|e| ModelError::new(ModelErrorClass::Unknown, e.to_string()))?;

        let retry_cancel = cancel.clone();
        let response = http::retry(&self.config.retry, &cancel, |_| {
            let req = self
                .http
                .post(&url)
                .headers(headers.clone())
                .body(bytes.clone());
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

        Ok(sse_stream(
            response,
            stream::OpenAiChatMapper::new(),
            cancel,
        ))
    }
}
