//! Protocol adapters: canonical request in, wire protocol out, normalized
//! stream events back.

pub mod anthropic;
pub mod openai_chat;
pub mod openai_responses;
pub mod streaming;

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use aether_core::model::{CapabilityProfile, Effort};
use reqwest::header::{HeaderMap, HeaderName};
use serde_json::{Map, Value};

use crate::http::RetryPolicy;
use crate::registry::{Protocol, ProviderFlags};

/// How a resolved credential is presented on the wire. Adapters decide the
/// header: Anthropic sends API keys as `x-api-key`, OpenAI as a bearer.
#[derive(Clone, PartialEq, Eq)]
pub enum RequestAuth {
    None,
    ApiKey(String),
    Bearer(String),
}

/// Hand-written so no secret can be printed. `AdapterConfig` derives `Debug`
/// and owns one of these, so a single `tracing::error!(?config, ..)` anywhere
/// would otherwise put a live credential in the log.
impl fmt::Debug for RequestAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::ApiKey(_) => write!(f, "ApiKey({})", self.fingerprint()),
            Self::Bearer(_) => write!(f, "Bearer({})", self.fingerprint()),
        }
    }
}

impl RequestAuth {
    /// Stable fingerprint for client caching. Never the secret itself.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let (tag, secret) = match self {
            Self::None => ("none", ""),
            Self::ApiKey(k) => ("key", k.as_str()),
            Self::Bearer(t) => ("bearer", t.as_str()),
        };
        let digest = Sha256::digest(secret.as_bytes());
        format!("{tag}:{}", hex::encode(&digest[..8]))
    }
}

/// Everything an adapter needs besides the request itself.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    pub base_url: String,
    pub auth: RequestAuth,
    /// Extra headers from the provider definition or the auth method.
    pub headers: BTreeMap<String, String>,
    /// Capability profiles by canonical model id.
    pub capabilities: HashMap<String, CapabilityProfile>,
    pub flags: ProviderFlags,
    pub retry: RetryPolicy,
    pub user_agent: String,
}

impl AdapterConfig {
    pub fn new(base_url: impl Into<String>, auth: RequestAuth) -> Self {
        Self {
            base_url: base_url.into(),
            auth,
            headers: BTreeMap::new(),
            capabilities: HashMap::new(),
            flags: ProviderFlags::default(),
            retry: RetryPolicy::default(),
            user_agent: crate::user_agent(),
        }
    }

    /// Profile for a model; unknown models get a conservative default so
    /// optional features are never sent to a server that may reject them.
    pub fn profile_for(&self, model: &str) -> CapabilityProfile {
        self.capabilities.get(model).cloned().unwrap_or_default()
    }

    // A cached client may not hold a profile for an unlisted model.
    fn effective_effort(
        &self,
        protocol: Protocol,
        model: &str,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        let profile = self.capabilities.get(model)?;
        protocol.effective_effort(profile, effort)
    }

    pub fn with_capability(mut self, model: impl Into<String>, profile: CapabilityProfile) -> Self {
        self.capabilities.insert(model.into(), profile);
        self
    }
}

/// Whether a configured header would replace a credential the adapter has
/// already resolved. `HeaderMap::insert` overwrites, so without this a
/// provider definition or an auth method could silently swap the credential
/// the runtime chose for a different one.
pub fn shadows_credential(headers: &HeaderMap, name: &HeaderName) -> bool {
    const CREDENTIAL_HEADERS: [&str; 2] = ["authorization", "x-api-key"];
    CREDENTIAL_HEADERS.iter().any(|h| name == h) && headers.contains_key(name)
}

/// Merge a caller's `extra_body` extension into a finished request body,
/// refusing the keys the adapter owns.
///
/// The merge runs last, so without this an extension decides what the request
/// actually is: `store: false` is a privacy promise about the person's
/// account, `stream` is the parsing contract, `model` is the link that gets
/// billed. A refused key is warned about rather than dropped in silence.
pub fn merge_extra_body(body: &mut Map<String, Value>, extra: &Map<String, Value>, owned: &[&str]) {
    for (key, value) in extra {
        if owned.contains(&key.as_str()) {
            tracing::warn!(key, "extra_body may not override a key the adapter owns");
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_effort_uses_the_active_clients_dialect_and_cached_profile() {
        use crate::ModelClient;
        use openai_chat::OpenAiChatClient;

        let model = crate::Registry::builtin()
            .resolve(&aether_core::model::ModelRef::new("anthropic", "opus"))
            .unwrap();
        let empty = AdapterConfig::new("https://example.test", RequestAuth::None);
        let cached = empty
            .clone()
            .with_capability(&model.model.id, model.model.capabilities.clone());
        let requested = Some(Effort::Max);
        assert_eq!(
            model
                .provider
                .protocol
                .effective_effort(&model.model.capabilities, requested),
            requested
        );
        assert_eq!(
            OpenAiChatClient::new(empty)
                .unwrap()
                .effective_effort(&model, requested),
            None
        );
        assert_eq!(
            OpenAiChatClient::new(cached)
                .unwrap()
                .effective_effort(&model, requested),
            Some(Effort::High)
        );
    }

    #[test]
    fn debug_prints_a_fingerprint_never_the_secret() {
        let secret = "sk-proj-NOTAREALKEYbutkeyshaped0123456789";
        for auth in [
            RequestAuth::ApiKey(secret.into()),
            RequestAuth::Bearer(secret.into()),
        ] {
            let shown = format!("{auth:?}");
            assert!(!shown.contains(secret), "{shown}");
            assert!(shown.contains(&auth.fingerprint()), "{shown}");
            // And through every type that encloses it.
            let config = AdapterConfig::new("https://example.test", auth);
            assert!(!format!("{config:?}").contains(secret));
        }
        assert_eq!(format!("{:?}", RequestAuth::None), "None");
    }

    #[test]
    fn extra_body_cannot_take_over_the_keys_the_adapter_owns() {
        let mut body = serde_json::json!({"model": "m", "stream": true, "store": false});
        let extra =
            serde_json::json!({"store": true, "model": "other", "stream": false, "top_p": 0.9});
        merge_extra_body(
            body.as_object_mut().expect("object"),
            extra.as_object().expect("object"),
            &["model", "stream", "store"],
        );
        assert_eq!(
            body,
            serde_json::json!({"model": "m", "stream": true, "store": false, "top_p": 0.9})
        );
    }
}
