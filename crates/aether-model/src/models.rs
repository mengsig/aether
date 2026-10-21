//! The facade the runtime uses: resolve a model, get a client, check auth.

#[cfg(all(test, unix))]
#[path = "models/default_tests.rs"]
mod default_tests;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use aether_core::model::{CapabilityProfile, ModelError, ModelErrorClass, ModelRef};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::adapters::{AdapterConfig, RequestAuth};
use crate::auth::{
    AuthError, AuthInteraction, AuthKind, AuthResolver, CredentialInfo, CredentialStore,
    FileCredentialStore,
};
use crate::client::ModelClient;
use crate::config::{AetherConfig, ConfigError, ModelSection, config_dir, load_config};
use crate::registry::{
    ModelDef, Protocol, ProviderDef, Registry, RegistryError, load_providers_file,
};
use crate::tier::ModelTier;

pub use crate::auth::LoginMethod;
pub use crate::registry::ResolvedModel;

/// Automatic compaction points, whatever the larger window is.
pub const CLAUDE_COMPACTION_TOKENS: u32 = 256_000;
pub const CODEX_COMPACTION_TOKENS: u32 = 272_000;

/// Capability extension set when the provider compacts on the server.
use aether_core::model::SERVER_COMPACTION_KEY;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelsError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("http client: {0}")]
    Http(String),
    /// A caller-composed refusal wrapping a resolve failure, kept in this
    /// class (not `RuntimeError::Invalid`) so it reads as unavailable rather
    /// than a bad argument.
    #[error("{0}")]
    Unbuildable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub tier: Option<ModelTier>,
    pub model: ModelRef,
    pub aliases: Vec<String>,
    pub name: String,
    pub provider_name: String,
    pub capabilities: CapabilityProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub provider: String,
    pub configured: bool,
    pub source: Option<String>,
    pub kind: Option<AuthKind>,
    /// Present when resolution failed rather than merely finding nothing.
    pub error: Option<String>,
}

pub struct Models {
    registry: Registry,
    config: AetherConfig,
    config_dir: PathBuf,
    resolver: AuthResolver,
    http: reqwest::Client,
    clients: Mutex<HashMap<String, Arc<dyn ModelClient>>>,
}

impl std::fmt::Debug for Models {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Models")
            .field("config_dir", &self.config_dir)
            .field("providers", &self.registry.providers().len())
            .finish_non_exhaustive()
    }
}

impl Models {
    /// Load built-ins, `providers.toml` overrides, `config.toml`, and the
    /// file credential store from the config directory.
    pub fn load(explicit_config_dir: Option<PathBuf>) -> Result<Self, ModelsError> {
        let dir = config_dir(explicit_config_dir);
        let mut registry = Registry::builtin();
        if let Some(file) = load_providers_file(&dir.join("providers.toml"))? {
            registry.apply_overrides(file)?;
        }
        let config = load_config(&dir)?;
        let store: Arc<dyn CredentialStore> = Arc::new(FileCredentialStore::in_config_dir(&dir));
        Self::new(registry, config, store, dir)
    }

    pub fn new(
        registry: Registry,
        config: AetherConfig,
        store: Arc<dyn CredentialStore>,
        config_dir: PathBuf,
    ) -> Result<Self, ModelsError> {
        let http = crate::http::build_client(&crate::user_agent())
            .map_err(|e| ModelsError::Http(e.message))?;
        Ok(Self {
            registry,
            config,
            config_dir,
            resolver: AuthResolver::new(store, http.clone()),
            http,
            clients: Mutex::new(HashMap::new()),
        })
    }

    /// Replace environment lookup for auth resolution (tests).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.resolver =
            AuthResolver::new(self.resolver.store().clone(), self.http.clone()).with_env(env);
        self
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn config(&self) -> &AetherConfig {
        &self.config
    }

    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }

    pub fn credential_store(&self) -> &Arc<dyn CredentialStore> {
        self.resolver.store()
    }

    pub fn resolve(&self, r: &ModelRef) -> Result<ResolvedModel, ModelsError> {
        let mut resolved = self.registry.resolve(r)?;
        resolved.model.capabilities =
            self.effective_capabilities(&resolved.provider, &resolved.model);
        Ok(resolved)
    }

    /// The catalogue profile plus where automatic compaction starts: the
    /// operator's `[models.compaction_tokens]` (model, then provider), else 256K
    /// for Claude and 272K for Codex, clamped to the window. Without a value the
    /// proportional default applies. The direct Anthropic API compacts on the
    /// server, which the profile records so Aether does not fold the same context.
    fn effective_capabilities(
        &self,
        provider: &ProviderDef,
        model: &ModelDef,
    ) -> CapabilityProfile {
        let mut profile = model.capabilities.clone();
        let configured = &self.config.models.compaction_tokens;
        let default = match provider.protocol {
            Protocol::AnthropicMessages | Protocol::ClaudeCode => Some(CLAUDE_COMPACTION_TOKENS),
            Protocol::Codex | Protocol::OpenaiCodexResponses => Some(CODEX_COMPACTION_TOKENS),
            _ => None,
        };
        profile.compaction_tokens = configured
            .get(&format!("{}/{}", provider.id, model.id))
            .or_else(|| configured.get(&provider.id))
            .copied()
            .or(default)
            .map(|tokens| tokens.min(profile.context_window));
        let listed = provider.models.iter().any(|m| m.id == model.id);
        if listed && provider.id == "anthropic" && provider.protocol == Protocol::AnthropicMessages
        {
            profile
                .extensions
                .insert(SERVER_COMPACTION_KEY.into(), serde_json::Value::Bool(true));
        }
        profile
    }

    /// The capability band of a model: the operator's `[models.tiers]`
    /// override (by canonical id or alias), else the catalog's.
    pub fn tier(&self, r: &ModelRef) -> Option<ModelTier> {
        let canonical = self.canonical(r);
        self.config
            .models
            .tiers
            .iter()
            .find(|(key, _)| {
                ModelRef::parse(key).is_some_and(|key| self.canonical(&key) == canonical)
            })
            .map(|(_, tier)| *tier)
            .or_else(|| {
                self.registry
                    .catalog_tier(&canonical.provider, &canonical.model)
            })
    }

    /// Every model of every enabled provider, with canonical ids and aliases.
    pub fn list(&self) -> Vec<ModelInfo> {
        self.registry
            .providers()
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.models.iter().map(move |m| ModelInfo {
                    tier: self.tier(&ModelRef::new(&p.id, &m.id)),
                    model: ModelRef::new(&p.id, &m.id),
                    aliases: m.aliases.clone(),
                    name: if m.name.is_empty() {
                        m.id.clone()
                    } else {
                        m.name.clone()
                    },
                    provider_name: p.name.clone(),
                    capabilities: {
                        let mut capabilities = m.capabilities.clone();
                        // A native engine's list price does not imply enforceable cost.
                        capabilities.extensions.insert(
                            "aether.cost_budget".into(),
                            serde_json::Value::Bool(p.protocol.reports_cost(&m.capabilities)),
                        );
                        capabilities
                    },
                })
            })
            .collect()
    }

    pub async fn auth_status(&self, provider: &str) -> AuthStatus {
        let Some(def) = self.registry.provider(provider) else {
            return AuthStatus {
                provider: provider.into(),
                configured: false,
                source: None,
                kind: None,
                error: Some(format!("unknown provider `{provider}`")),
            };
        };
        match self.resolver.resolve(def).await {
            Ok(Some(auth)) => AuthStatus {
                provider: provider.into(),
                configured: true,
                source: Some(auth.source),
                kind: Some(auth.kind),
                error: None,
            },
            Ok(None) => AuthStatus {
                provider: provider.into(),
                configured: false,
                source: None,
                kind: None,
                error: None,
            },
            Err(e) => AuthStatus {
                provider: provider.into(),
                configured: false,
                source: None,
                kind: None,
                error: Some(e.to_string()),
            },
        }
    }

    /// The configured default, or the first usable provider's own default:
    /// an engine's `default` model (the CLI picks, never the priciest catalog
    /// entry), else the provider's first model.
    pub async fn default_model(&self) -> Option<ModelRef> {
        if let Some(text) = &self.config.model.default {
            let parsed = ModelRef::parse(text);
            match parsed.as_ref().map(|r| self.registry.resolve(r)) {
                Some(Ok(_)) => return parsed,
                // Kept so creation refuses it with the successor; never swapped silently.
                Some(Err(e)) if e.is_retirement() => return parsed,
                _ => tracing::warn!(default = %text, "configured default model is not resolvable"),
            }
        }
        for provider in self.registry.providers().iter().filter(|p| p.enabled) {
            let Some(first) = provider_default(provider) else {
                continue;
            };
            if matches!(self.resolver.resolve(provider).await, Ok(Some(_))) {
                return Some(ModelRef::new(&provider.id, &first.id));
            }
        }
        None
    }

    /// `[model]` settings naming a retired model, each with its successor.
    pub fn retired_settings(&self) -> Vec<String> {
        retired_model_settings(&self.registry, &self.config.model)
    }

    /// Model configured for compaction, if any.
    pub fn compactor_model(&self) -> Option<ModelRef> {
        self.config
            .model
            .compactor
            .as_deref()
            .and_then(ModelRef::parse)
    }

    /// Automatic defaults stay on the agent's provider and reuse its credentials.
    pub fn compactor_for(&self, agent: &ModelRef) -> ModelRef {
        if let Some(configured) = self.compactor_model() {
            return configured;
        }
        let id = match agent.provider.as_str() {
            "codex" | "openai" | "openai-responses" => "gpt-6-luna",
            "claude-code" => "sonnet",
            "anthropic" => "claude-sonnet-5-5",
            _ => return agent.clone(),
        };
        let candidate = ModelRef::new(&agent.provider, id);
        if self.resolve(&candidate).is_ok() {
            candidate
        } else {
            agent.clone()
        }
    }

    /// Resolve auth and build (or reuse) the adapter for a model.
    pub async fn client(&self, r: &ModelRef) -> Result<Arc<dyn ModelClient>, ModelError> {
        let resolved = self
            .resolve(r)
            .map_err(|e| ModelError::new(ModelErrorClass::NotFound, e.to_string()))?;
        let provider = &resolved.provider;
        let auth = self.resolver.resolve(provider).await?.ok_or_else(|| {
            ModelError::new(
                ModelErrorClass::Auth,
                format!(
                    "provider `{}` is not configured: {}",
                    provider.id,
                    provider.auth.hint(&provider.id)
                ),
            )
        })?;
        require_private_endpoint(&provider.base_url, &auth.auth)?;
        let key = format!("{}:{}", provider.id, auth.auth.fingerprint());
        let mut clients = self.clients.lock().await;
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }
        let mut headers: BTreeMap<String, String> = provider.headers.clone();
        headers.extend(auth.headers);
        let mut capabilities = HashMap::new();
        for m in &provider.models {
            let profile = self.effective_capabilities(provider, m);
            capabilities.insert(m.id.clone(), profile.clone());
            for alias in &m.aliases {
                capabilities.insert(alias.clone(), profile.clone());
            }
        }
        if !capabilities.contains_key(&resolved.model.id) {
            capabilities.insert(
                resolved.model.id.clone(),
                resolved.model.capabilities.clone(),
            );
        }
        let config = AdapterConfig {
            base_url: provider.base_url.clone(),
            auth: auth.auth,
            headers,
            capabilities,
            flags: provider.flags.clone(),
            retry: crate::http::RetryPolicy::default(),
            user_agent: crate::user_agent(),
        };
        let client: Arc<dyn ModelClient> = match provider.protocol {
            Protocol::AnthropicMessages => Arc::new(
                crate::adapters::anthropic::AnthropicClient::with_http(config, self.http.clone()),
            ),
            Protocol::OpenaiChatCompletions => {
                Arc::new(crate::adapters::openai_chat::OpenAiChatClient::with_http(
                    config,
                    self.http.clone(),
                ))
            }
            Protocol::OpenaiResponses | Protocol::OpenaiCodexResponses => {
                let dialect = if provider.protocol == Protocol::OpenaiCodexResponses {
                    crate::adapters::openai_responses::Dialect::Codex
                } else {
                    crate::adapters::openai_responses::Dialect::OpenAi
                };
                Arc::new(
                    crate::adapters::openai_responses::OpenAiResponsesClient::with_http(
                        config,
                        dialect,
                        self.http.clone(),
                    ),
                )
            }
            Protocol::ClaudeCode | Protocol::Codex => {
                return Err(ModelError::new(
                    ModelErrorClass::InvalidRequest,
                    format!(
                        "provider `{}` is an engine; the runtime drives it directly",
                        provider.id
                    ),
                ));
            }
        };
        clients.insert(key, client.clone());
        Ok(client)
    }

    /// Canonical model reference (aliases resolved): what a provider is sent.
    pub fn canonical(&self, r: &ModelRef) -> ModelRef {
        match self.registry.resolve(r) {
            Ok(resolved) => ModelRef::new(&resolved.provider.id, &resolved.model.id),
            Err(_) => r.clone(),
        }
    }

    pub async fn login(
        &self,
        provider: &str,
        method: LoginMethod,
        interaction: &dyn AuthInteraction,
        cancel: &CancellationToken,
    ) -> Result<CredentialInfo, AuthError> {
        let def = self
            .registry
            .provider(provider)
            .ok_or_else(|| AuthError::UnknownProvider(provider.into()))?;
        let info = self
            .resolver
            .login(def, method, interaction, cancel)
            .await?;
        self.clients
            .lock()
            .await
            .retain(|k, _| !k.starts_with(&format!("{provider}:")));
        Ok(info)
    }

    pub async fn logout(&self, provider: &str) -> Result<(), AuthError> {
        self.resolver.logout(provider).await?;
        self.clients
            .lock()
            .await
            .retain(|k, _| !k.starts_with(&format!("{provider}:")));
        Ok(())
    }

    pub async fn credentials(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        self.resolver.store().list().await
    }

    /// Whether a provider offers an interactive OAuth login.
    pub fn oauth_label(&self, provider: &str) -> Option<String> {
        self.registry
            .provider(provider)
            .and_then(|p| p.auth.oauth.as_ref())
            .map(|k| k.label())
    }
}

/// A resolved credential must not leave the machine in the clear.
///
/// `http://` stays allowed - it is how a local server is reached - but only
/// on loopback, where the bearer never touches a network. A plaintext base
/// URL pointing anywhere else is refused rather than warned about: the
/// operator can set `https`, or use the loopback address the server is
/// actually on.
fn require_private_endpoint(base_url: &str, auth: &RequestAuth) -> Result<(), ModelError> {
    if matches!(auth, RequestAuth::None) {
        return Ok(());
    }
    // An unparseable or empty base URL is an engine or a broken config; the
    // request itself reports that, and more usefully.
    let Ok(url) = url::Url::parse(base_url) else {
        return Ok(());
    };
    if url.scheme() != "http" {
        return Ok(());
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        None => false,
    };
    if loopback {
        return Ok(());
    }
    Err(ModelError::new(
        ModelErrorClass::InvalidRequest,
        format!(
            "refusing to send a credential in the clear to `{}` over http; \
             use https, or a loopback address for a local server",
            url.host_str().unwrap_or("that host")
        ),
    ))
}

/// Convenience for callers that only have an API key in hand.
pub fn request_auth_for_key(key: &str) -> RequestAuth {
    RequestAuth::ApiKey(key.to_owned())
}

/// `[model]` settings naming a retired model on `registry`, overrides
/// included, each with its successor.
pub fn retired_model_settings(registry: &Registry, section: &ModelSection) -> Vec<String> {
    [
        ("default", &section.default),
        ("compactor", &section.compactor),
    ]
    .into_iter()
    .filter_map(|(key, text)| {
        let error = registry
            .resolve(&ModelRef::parse(text.as_deref()?)?)
            .err()?;
        error
            .is_retirement()
            .then(|| format!("[model] {key}: {error}"))
    })
    .collect()
}

/// The model a provider is used on when nothing names one. An engine CLI
/// chooses for itself through its `default` entry; the catalogue order puts
/// the priciest model first, so it must never decide.
fn provider_default(provider: &ProviderDef) -> Option<&ModelDef> {
    let engine_choice = provider
        .models
        .iter()
        .find(|m| provider.protocol.is_engine() && m.id == "default");
    engine_choice.or(provider.models.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MemoryCredentialStore;
    use crate::config::AetherConfig;

    fn builtin_models(config: AetherConfig) -> Models {
        Models::new(
            Registry::builtin(),
            config,
            Arc::new(MemoryCredentialStore::new()),
            PathBuf::from("/unused"),
        )
        .expect("models")
    }

    fn resolved_compaction(models: &Models, provider: &str, model: &str) -> (u32, Option<u32>) {
        let resolved = models
            .resolve(&ModelRef::new(provider, model))
            .expect("builtin model");
        (
            resolved.model.capabilities.context_window,
            resolved.model.capabilities.compaction_tokens,
        )
    }

    #[test]
    fn server_compaction_is_marked_only_for_listed_direct_anthropic_models() {
        let models = builtin_models(AetherConfig::default());
        let provider = models
            .registry()
            .providers()
            .iter()
            .find(|p| p.id == "anthropic")
            .expect("anthropic provider")
            .clone();
        let listed = provider.models.first().expect("a listed model");
        assert!(
            models
                .effective_capabilities(&provider, listed)
                .compacts_on_server()
        );
        let mut unlisted = listed.clone();
        unlisted.id = "not-in-the-list".into();
        assert!(
            !models
                .effective_capabilities(&provider, &unlisted)
                .compacts_on_server()
        );
    }

    #[test]
    fn compaction_starts_at_256k_for_claude_and_272k_for_codex() {
        let models = builtin_models(AetherConfig::default());
        for (provider, model, tokens) in [
            ("claude-code", "opus", Some(256_000)),
            ("claude-code", "default", Some(256_000)),
            ("anthropic", "sonnet", Some(256_000)),
            ("codex", "gpt-6.1-sol", Some(272_000)),
            ("groq", "openai/gpt-oss-120b", None),
        ] {
            let (window, resolved) = resolved_compaction(&models, provider, model);
            assert_eq!(resolved, tokens, "{provider}/{model}");
            assert!(resolved.is_none_or(|t| t <= window), "{provider}/{model}");
        }
        assert_eq!(
            resolved_compaction(&models, "claude-code", "opus").0,
            1_000_000
        );
    }

    #[test]
    fn configured_compaction_tokens_prefer_the_model_and_stay_inside_the_window() {
        let mut config = AetherConfig::default();
        config
            .models
            .compaction_tokens
            .insert("claude-code".into(), 100_000);
        config
            .models
            .compaction_tokens
            .insert("claude-code/claude-haiku-5-5".into(), 5_000_000);
        config
            .models
            .compaction_tokens
            .insert("groq".into(), 50_000);
        let models = builtin_models(config);
        assert_eq!(
            resolved_compaction(&models, "claude-code", "opus").1,
            Some(100_000)
        );
        assert_eq!(
            resolved_compaction(&models, "claude-code", "haiku"),
            (1_000_000, Some(1_000_000)),
            "an alias resolves to its model entry, clamped to the window"
        );
        assert_eq!(
            resolved_compaction(&models, "groq", "openai/gpt-oss-120b").1,
            Some(50_000)
        );
    }

    #[test]
    fn catalogue_preserves_builtin_engine_aliases() {
        let models = Models::new(
            Registry::builtin(),
            AetherConfig::default(),
            Arc::new(MemoryCredentialStore::new()),
            PathBuf::from("/unused"),
        )
        .expect("models");
        let catalogue = models.list();
        for info in &catalogue {
            assert_eq!(
                info.capabilities.extensions["aether.cost_budget"],
                models.resolve(&info.model).unwrap().reports_cost(),
            );
        }
        let opus = catalogue
            .iter()
            .find(|info| info.model == ModelRef::new("claude-code", "claude-opus-5-5"))
            .expect("builtin opus");
        assert_eq!(opus.aliases, ["opus", "opus-5-5", "opus-5.5"]);
        assert_eq!(opus.capabilities.context_window, 1_000_000);
        for alias in &opus.aliases {
            assert_eq!(
                models.canonical(&ModelRef::new("claude-code", alias)),
                opus.model
            );
        }
    }

    fn models(env: Vec<(&'static str, &'static str)>, default: Option<&str>) -> Models {
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::new());
        let config = AetherConfig {
            providers: Default::default(),
            theme: Default::default(),
            model: crate::config::ModelSection {
                default: default.map(str::to_owned),
                ..Default::default()
            },
            models: Default::default(),
            approval: Default::default(),
            network: Default::default(),
            scheduler: Default::default(),
            web: Default::default(),
            artifacts: Default::default(),
            retention: Default::default(),
            storage: Default::default(),
            agents: Default::default(),
            build_cache: Default::default(),
            mcp: Default::default(),
            memory: Default::default(),
            listen: Default::default(),
            remote: Default::default(),
            serve: Default::default(),
            update: Default::default(),
        };
        // Engine providers consult the real CLIs on this machine; keep the
        // test independent of what is installed or logged in here.
        let mut registry = Registry::builtin();
        registry.retain(|p| !p.protocol.is_engine());
        Models::new(registry, config, store, PathBuf::from("/nonexistent"))
            .expect("models")
            .with_env(move |k| {
                env.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            })
    }

    #[test]
    fn compaction_defaults_stay_on_the_same_provider_and_allow_override() {
        let mut config = AetherConfig::default();
        for override_model in [None, Some("anthropic/claude-haiku-5-5")] {
            config.model.compactor = override_model.map(str::to_owned);
            let dir = tempfile::tempdir().unwrap();
            let models = Models::new(
                Registry::builtin(),
                config.clone(),
                Arc::new(MemoryCredentialStore::default()),
                dir.path().into(),
            )
            .unwrap();
            for (provider, active, expected) in [
                ("codex", "gpt-6-astra", "gpt-6-luna"),
                ("claude-code", "opus", "sonnet"),
                ("anthropic", "opus", "claude-sonnet-5-5"),
                ("openai", "gpt-6-astra", "gpt-6-luna"),
                ("openai-responses", "gpt-6.1-sol", "gpt-6-luna"),
            ] {
                assert_eq!(
                    models.compactor_for(&ModelRef::new(provider, active)),
                    override_model
                        .and_then(ModelRef::parse)
                        .unwrap_or_else(|| ModelRef::new(provider, expected))
                );
            }
        }
    }

    #[tokio::test]
    async fn client_requires_auth_and_caches() {
        let m = models(vec![], None);
        let err = m
            .client(&ModelRef::new("anthropic", "opus"))
            .await
            .err()
            .expect("expected an error");
        assert_eq!(err.class, ModelErrorClass::Auth);
        assert!(err.message.contains("ANTHROPIC_API_KEY"));

        let m = models(vec![("ANTHROPIC_API_KEY", "sk")], None);
        let a = m
            .client(&ModelRef::new("anthropic", "opus"))
            .await
            .expect("client");
        let b = m
            .client(&ModelRef::new("anthropic", "claude-sonnet-5-5"))
            .await
            .expect("client");
        assert_eq!(a.adapter(), "anthropic-messages/1");
        assert!(Arc::ptr_eq(&a, &b));
        let local = m
            .client(&ModelRef::new("ollama", "qwen3"))
            .await
            .expect("client");
        assert_eq!(local.adapter(), "openai-chat-completions/1");
        let unknown = m
            .client(&ModelRef::new("nope", "x"))
            .await
            .err()
            .expect("expected an error");
        assert_eq!(unknown.class, ModelErrorClass::NotFound);
    }

    #[test]
    fn engine_providers_default_to_their_own_choice_not_the_priciest_model() {
        let registry = Registry::builtin();
        for id in ["claude-code", "codex"] {
            let provider = registry.provider(id).expect("builtin engine");
            assert_eq!(
                provider_default(provider).map(|m| m.id.as_str()),
                Some("default")
            );
        }
        let anthropic = registry.provider("anthropic").expect("builtin");
        assert_eq!(
            provider_default(anthropic).map(|m| m.id.as_str()),
            anthropic.models.first().map(|m| m.id.as_str())
        );
    }

    #[tokio::test]
    async fn default_model_prefers_config_then_configured_provider() {
        let m = models(vec![("GROQ_API_KEY", "g")], None);
        assert_eq!(
            m.default_model().await,
            Some(ModelRef::new("groq", "openai/gpt-oss-120b"))
        );
        let m = models(vec![("GROQ_API_KEY", "g")], Some("anthropic/sonnet"));
        assert_eq!(
            m.default_model().await,
            Some(ModelRef::new("anthropic", "sonnet"))
        );
        assert_eq!(
            m.canonical(&ModelRef::new("anthropic", "sonnet")),
            ModelRef::new("anthropic", "claude-sonnet-5-5")
        );
        let m = models(vec![], Some("bogus"));
        assert_eq!(m.default_model().await, None);
    }

    #[tokio::test]
    async fn a_credential_never_goes_out_over_plaintext_off_loopback() {
        let key = RequestAuth::ApiKey("sk-not-a-real-key".into());
        // Loopback is how a local server is reached, so it stays allowed.
        for allowed in [
            "http://127.0.0.1:11434/v1",
            "http://localhost:1234/v1",
            "http://[::1]:8080",
            "https://api.example.test/v1",
        ] {
            require_private_endpoint(allowed, &key)
                .unwrap_or_else(|e| panic!("{allowed} must be allowed: {e}"));
        }
        for refused in [
            "http://10.0.0.7:8000/v1",
            "http://models.example.test/v1",
            "http://192.168.1.4/v1",
        ] {
            let err = require_private_endpoint(refused, &key)
                .expect_err("a credential must not go out in the clear");
            assert_eq!(err.class, ModelErrorClass::InvalidRequest);
            assert!(err.message.contains("https"), "{}", err.message);
        }
        // Without a credential there is nothing to leak.
        require_private_endpoint("http://models.example.test/v1", &RequestAuth::None)
            .expect("no credential, no refusal");
    }

    #[tokio::test]
    async fn auth_status_reports_source_and_unknown() {
        let m = models(vec![("XAI_API_KEY", "x")], None);
        let s = m.auth_status("xai").await;
        assert!(s.configured);
        assert_eq!(s.source.as_deref(), Some("XAI_API_KEY"));
        assert_eq!(s.kind, Some(AuthKind::ApiKey));
        let s = m.auth_status("openai").await;
        assert!(!s.configured && s.error.is_none());
        let s = m.auth_status("nope").await;
        assert!(s.error.is_some());
        assert!(
            m.list()
                .iter()
                .any(|i| i.model == ModelRef::new("anthropic", "claude-opus-5-5"))
        );
        // openrouter is the built-in that still ships an OAuth login; the
        // Codex dialect deliberately offers none (VISION §1.6).
        assert!(m.oauth_label("openrouter").is_some());
        assert!(m.oauth_label("openai-codex").is_none());
    }
}
