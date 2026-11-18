//! OpenAI Responses adapter. Covers `POST {base_url}/responses` on OpenAI
//! itself and on any server speaking the same protocol.

pub mod request;
pub mod stream;

use aether_core::model::{Effort, ModelError, ModelErrorClass, ModelRequest};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use crate::adapters::streaming::sse_stream;
use crate::adapters::{AdapterConfig, RequestAuth, shadows_credential};
use crate::client::{ModelClient, ModelStream};
use crate::http::{self, map_http_error, map_transport_error, read_error_body};

pub const ADAPTER: &str = "openai-responses/1";
pub const CODEX_ADAPTER: &str = "openai-codex-responses/1";

/// Which flavour of the Responses protocol a provider speaks. The wire shape
/// is the same; the Codex backend only accepts a narrower request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    OpenAi,
    Codex,
}

impl Dialect {
    fn protocol(self) -> crate::Protocol {
        match self {
            Self::OpenAi => crate::Protocol::OpenaiResponses,
            Self::Codex => crate::Protocol::OpenaiCodexResponses,
        }
    }
}

#[derive(Debug)]
pub struct OpenAiResponsesClient {
    http: reqwest::Client,
    config: AdapterConfig,
    dialect: Dialect,
}

impl OpenAiResponsesClient {
    pub fn new(config: AdapterConfig, dialect: Dialect) -> Result<Self, ModelError> {
        let http = http::build_client(&config.user_agent)?;
        Ok(Self {
            http,
            config,
            dialect,
        })
    }

    pub fn with_http(config: AdapterConfig, dialect: Dialect, http: reqwest::Client) -> Self {
        Self {
            http,
            config,
            dialect,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/responses", self.config.base_url.trim_end_matches('/'))
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
impl ModelClient for OpenAiResponsesClient {
    fn adapter(&self) -> &str {
        match self.dialect {
            Dialect::OpenAi => ADAPTER,
            Dialect::Codex => CODEX_ADAPTER,
        }
    }

    fn effective_effort(
        &self,
        model: &crate::ResolvedModel,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        self.config
            .effective_effort(self.dialect.protocol(), &model.model.id, effort)
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let profile = self.config.profile_for(&request.model.model);
        let body = request::build(&request, &profile, self.dialect);
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
            stream::OpenAiResponsesMapper::new(),
            cancel,
        ))
    }
}
