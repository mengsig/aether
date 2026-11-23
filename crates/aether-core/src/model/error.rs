use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Normalized failure class. Every adapter maps its provider's errors here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorClass {
    /// Missing, invalid or expired credentials.
    Auth,
    /// Credentials lack permission for this model or feature.
    Permission,
    RateLimit,
    /// Provider credits or spending allowance exhausted; fallback may recover.
    QuotaExhausted,
    /// Provider subscription/session/day/week allowance exhausted.
    UsageLimit,
    Overloaded,
    ContextLimit,
    InvalidRequest,
    NotFound,
    Network,
    Timeout,
    Cancelled,
    Server,
    Unknown,
}

impl ModelErrorClass {
    /// The wire name, so a projection prints the class the log carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Permission => "permission",
            Self::RateLimit => "rate_limit",
            Self::QuotaExhausted => "quota_exhausted",
            Self::UsageLimit => "usage_limit",
            Self::Overloaded => "overloaded",
            Self::ContextLimit => "context_limit",
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Server => "server",
            Self::Unknown => "unknown",
        }
    }
}

/// Recognize explicit provider exhaustion evidence, not generic "quota" or 429.
/// Call only on error codes/messages, never assistant prose or usage telemetry.
pub fn provider_exhaustion(message: &str) -> Option<ModelErrorClass> {
    let lower = message.to_lowercase();
    if [
        "insufficient_quota",
        "quotaexhausted:",
        "credit balance is too low",
        "credit balance too low",
        "insufficient credits",
        "credits exhausted",
        "billing_hard_limit_reached",
        "billing_limit_reached",
        "organization_usage_limit_exceeded",
        "spend limit exceeded",
        "spend limit has been reached",
        "reached your specified api usage limits",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
    {
        return Some(ModelErrorClass::QuotaExhausted);
    }
    if [
        "usage_limit_reached",
        "you've hit your usage limit",
        "you have hit your usage limit",
        "usagelimit:",
        "usage limit exceeded",
        "usage limit reached",
        "you've hit your limit",
        "you have hit your limit",
        "weekly limit reached",
        "daily limit reached",
        "weekly limit exceeded",
        "daily limit exceeded",
        "weekly limit resets",
        "daily limit resets",
        "5-hour limit resets",
        "5-hour limit reached",
        "session limit reached",
        "you have reached your usage limit",
        "you've reached your usage limit",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
        || named_limit_hit(&lower)
    {
        return Some(ModelErrorClass::UsageLimit);
    }
    None
}

fn named_limit_hit(message: &str) -> bool {
    message.match_indices("hit your ").any(|(start, _)| {
        if message[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        {
            return false;
        }
        let mut words = message[start + "hit your ".len()..].split_whitespace();
        let Some(name) = words.next() else {
            return false;
        };
        let Some(limit) = words.next() else {
            return false;
        };
        name != "rate"
            && name.chars().any(char::is_alphanumeric)
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && limit.trim_end_matches(|c: char| !c.is_alphanumeric()) == "limit"
    })
}

/// A model call failed.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error, schemars::JsonSchema,
)]
#[error("{class:?}: {message}")]
pub struct ModelError {
    pub class: ModelErrorClass,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    // Milliseconds on the wire; `with` names a serde module, which is not a
    // type a schema can be taken from.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_millis"
    )]
    #[schemars(with = "Option<u64>")]
    pub retry_after: Option<Duration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ModelError {
    pub fn new(class: ModelErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            status: None,
            retry_after: None,
            provider_code: None,
            request_id: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_retry_after(mut self, after: Duration) -> Self {
        self.retry_after = Some(after);
        self
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    pub fn with_provider_code(mut self, code: impl Into<String>) -> Self {
        self.provider_code = Some(code.into());
        self
    }

    /// Whether an automatic retry with backoff is reasonable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.class,
            ModelErrorClass::RateLimit
                | ModelErrorClass::Overloaded
                | ModelErrorClass::Network
                | ModelErrorClass::Timeout
                | ModelErrorClass::Server
        )
    }
}

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        value.map(|d| d.as_millis() as u64).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_millis))
    }
}
