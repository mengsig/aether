//! Turn a provider definition plus the stored/ambient credentials into
//! request auth. A stored credential owns its provider; the environment is
//! consulted only when nothing is stored; a failed refresh is an error,
//! never a silent fallback.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::helper::HelperCache;
use super::import;
use super::interaction::{AuthInteraction, AuthPrompt};
use super::types::{AuthError, AuthKind, Credential, CredentialInfo, CredentialStore};
use crate::adapters::RequestAuth;
use crate::registry::ProviderDef;

/// The `source` of a provider that needs no credentials: a local server,
/// ready only when something listens at its address.
pub const NO_AUTH_SOURCE: &str = "no auth required";

/// Refresh when less than this remains.
const REFRESH_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuth {
    pub auth: RequestAuth,
    pub headers: BTreeMap<String, String>,
    /// Human-readable origin: `ANTHROPIC_API_KEY`, `stored credential`, `oauth`.
    pub source: String,
    pub kind: AuthKind,
}

/// How to log in interactively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginMethod {
    ApiKey,
    OAuth {
        method: Option<String>,
    },
    /// Hand off to the provider's own CLI login.
    Cli,
}

type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct AuthResolver {
    store: Arc<dyn CredentialStore>,
    http: reqwest::Client,
    helpers: HelperCache,
    env: EnvFn,
}

impl std::fmt::Debug for AuthResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResolver").finish_non_exhaustive()
    }
}

impl AuthResolver {
    pub fn new(store: Arc<dyn CredentialStore>, http: reqwest::Client) -> Self {
        Self {
            store,
            http,
            helpers: HelperCache::new(),
            env: Arc::new(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())),
        }
    }

    /// Replace environment lookup (tests).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Arc::new(env);
        self
    }

    pub fn store(&self) -> &Arc<dyn CredentialStore> {
        &self.store
    }

    pub async fn resolve(&self, provider: &ProviderDef) -> Result<Option<ResolvedAuth>, AuthError> {
        let methods = &provider.auth;
        let stored = match self.store.read(&provider.id).await {
            Ok(stored) => stored,
            // CLI/no-auth providers can work without this store; doctor reports its failure.
            Err(_) if methods.cli.is_some() || methods.none => None,
            Err(error) => return Err(error),
        };
        if let Some(stored) = stored {
            return self.resolve_stored(provider, stored).await.map(Some);
        }
        if let Some(auth) = self.resolve_ambient(provider).await? {
            return Ok(Some(auth));
        }
        if let Some(cli) = &methods.cli {
            let status = cli.status().await;
            if !status.available {
                return Err(AuthError::Flow(status.detail));
            }
            return Ok(status.logged_in.then(|| ResolvedAuth {
                auth: RequestAuth::None,
                headers: BTreeMap::new(),
                source: format!("{} login", cli.program),
                kind: AuthKind::Cli,
            }));
        }
        if methods.none {
            return Ok(Some(ResolvedAuth {
                auth: RequestAuth::None,
                headers: BTreeMap::new(),
                source: NO_AUTH_SOURCE.into(),
                kind: AuthKind::None,
            }));
        }
        Ok(None)
    }

    async fn resolve_ambient(
        &self,
        provider: &ProviderDef,
    ) -> Result<Option<ResolvedAuth>, AuthError> {
        let methods = &provider.auth;
        if let Some(helper) = &methods.helper {
            let token = self.helpers.token(helper).await?;
            return Ok(Some(ResolvedAuth {
                auth: RequestAuth::Bearer(token),
                headers: BTreeMap::new(),
                source: format!("helper `{}`", helper.display()),
                kind: AuthKind::Helper,
            }));
        }
        if let Some(kind) = &methods.import
            && let Some(imported) = import::import(kind, &self.helpers, &*self.env).await?
        {
            return Ok(Some(ResolvedAuth {
                auth: RequestAuth::Bearer(imported.token),
                headers: BTreeMap::new(),
                source: imported.source,
                kind: AuthKind::Import,
            }));
        }
        for var in &methods.bearer_env {
            if let Some(token) = (self.env)(var) {
                return Ok(Some(ResolvedAuth {
                    auth: RequestAuth::Bearer(token),
                    headers: BTreeMap::new(),
                    source: var.clone(),
                    kind: AuthKind::Bearer,
                }));
            }
        }
        for var in &methods.api_key_env {
            if let Some(key) = (self.env)(var) {
                return Ok(Some(ResolvedAuth {
                    auth: RequestAuth::ApiKey(key),
                    headers: BTreeMap::new(),
                    source: var.clone(),
                    kind: AuthKind::ApiKey,
                }));
            }
        }
        Ok(None)
    }

    async fn resolve_stored(
        &self,
        provider: &ProviderDef,
        stored: Credential,
    ) -> Result<ResolvedAuth, AuthError> {
        match stored {
            Credential::ApiKey { key, .. } => Ok(ResolvedAuth {
                auth: RequestAuth::ApiKey(key),
                headers: BTreeMap::new(),
                source: "stored credential".into(),
                kind: AuthKind::ApiKey,
            }),
            oauth @ Credential::OAuth { .. } => {
                let now = aether_core::time::now();
                let credential = if oauth.expires_within(REFRESH_WINDOW, now) {
                    self.refresh_locked(provider).await?
                } else {
                    oauth
                };
                let Credential::OAuth { access, .. } = &credential else {
                    return Err(AuthError::Refresh {
                        provider: provider.id.clone(),
                        reason: "credential changed type".into(),
                    });
                };
                let headers = provider
                    .auth
                    .oauth
                    .as_ref()
                    .map(|k| k.headers(&credential))
                    .unwrap_or_default();
                Ok(ResolvedAuth {
                    auth: RequestAuth::Bearer(access.clone()),
                    headers,
                    source: "oauth".into(),
                    kind: AuthKind::OAuth,
                })
            }
        }
    }

    /// Refresh under the store lock; re-checks expiry inside so a concurrent
    /// refresh (another request or process) is not repeated.
    async fn refresh_locked(&self, provider: &ProviderDef) -> Result<Credential, AuthError> {
        let kind = provider
            .auth
            .oauth
            .clone()
            .ok_or_else(|| AuthError::Refresh {
                provider: provider.id.clone(),
                reason: "stored token expired and this provider has no refresh flow; sign in again"
                    .into(),
            })?;
        let http = self.http.clone();
        let provider_id = provider.id.clone();
        let updated = self
            .store
            .modify(
                &provider.id,
                Box::new(move |current| {
                    Box::pin(async move {
                        let Some(current) = current else {
                            return Err(AuthError::NotConfigured {
                                provider: provider_id,
                                hint: "logged out during refresh".into(),
                            });
                        };
                        if !current.expires_within(REFRESH_WINDOW, aether_core::time::now()) {
                            return Ok(None);
                        }
                        let refreshed = kind.refresh(&http, &current).await.map_err(|e| {
                            AuthError::Refresh {
                                provider: provider_id.clone(),
                                reason: e.to_string(),
                            }
                        })?;
                        Ok(Some(refreshed))
                    })
                }),
            )
            .await?;
        updated.ok_or_else(|| AuthError::NotConfigured {
            provider: provider.id.clone(),
            hint: "credential disappeared".into(),
        })
    }

    pub async fn login(
        &self,
        provider: &ProviderDef,
        method: LoginMethod,
        interaction: &dyn AuthInteraction,
        cancel: &CancellationToken,
    ) -> Result<CredentialInfo, AuthError> {
        if let Some(cli) = &provider.auth.cli {
            cli.login().await?;
            return Ok(CredentialInfo {
                provider: provider.id.clone(),
                kind: AuthKind::Cli,
                expires_at: None,
            });
        }
        let credential = match method {
            LoginMethod::Cli => {
                return Err(AuthError::Unsupported {
                    provider: provider.id.clone(),
                    what: "CLI login".into(),
                });
            }
            LoginMethod::ApiKey => {
                if !provider.auth.accepts_api_key() {
                    return Err(AuthError::Unsupported {
                        provider: provider.id.clone(),
                        what: "API keys".into(),
                    });
                }
                let key = interaction
                    .prompt(AuthPrompt::Secret {
                        message: format!("Enter {} API key", provider.name),
                    })
                    .await?;
                if key.trim().is_empty() {
                    return Err(AuthError::Flow("empty API key".into()));
                }
                Credential::ApiKey {
                    key: key.trim().to_owned(),
                    env: BTreeMap::new(),
                }
            }
            LoginMethod::OAuth { method } => {
                let kind = provider
                    .auth
                    .oauth
                    .clone()
                    .ok_or_else(|| AuthError::Unsupported {
                        provider: provider.id.clone(),
                        what: "OAuth login".into(),
                    })?;
                kind.login(&self.http, interaction, cancel, method.as_deref())
                    .await?
            }
        };
        let info = CredentialInfo {
            provider: provider.id.clone(),
            kind: credential.kind(),
            expires_at: credential.expires_at(),
        };
        self.store
            .modify(
                &provider.id,
                Box::new(move |_| Box::pin(async move { Ok(Some(credential)) })),
            )
            .await?;
        Ok(info)
    }

    pub async fn logout(&self, provider: &str) -> Result<(), AuthError> {
        self.store.delete(provider).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::methods::AuthMethods;
    use crate::auth::store::MemoryCredentialStore;
    use crate::registry::Protocol;

    fn provider(auth: AuthMethods) -> ProviderDef {
        ProviderDef {
            id: "p".into(),
            name: "P".into(),
            protocol: Protocol::OpenaiChatCompletions,
            base_url: "http://localhost".into(),
            auth,
            headers: BTreeMap::new(),
            models: vec![],
            default_capabilities: None,
            flags: Default::default(),
            max_concurrent: None,
            family: None,
            enabled: true,
        }
    }

    fn resolver(env: Vec<(&'static str, &'static str)>) -> AuthResolver {
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::new());
        AuthResolver::new(store, reqwest::Client::new()).with_env(move |k| {
            env.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unsafe_store_does_not_block_cli_or_no_auth_providers() {
        use crate::auth::FileCredentialStore;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("config");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o775))
            .expect("shared directory");
        let store = Arc::new(FileCredentialStore::in_config_dir(dir.path()));
        assert!(store.read("anthropic").await.is_err());
        let resolver = AuthResolver::new(store, reqwest::Client::new()).with_env(|_| None);
        let registry = crate::Registry::builtin();
        for id in ["claude-code", "codex"] {
            let mut provider = registry.provider(id).expect("CLI provider").clone();
            let cli = provider.auth.cli.as_mut().expect("CLI auth");
            cli.program = "/bin/sh".into();
            cli.status = vec![
                "-c".into(),
                format!("printf '%s' '{}'", cli.logged_in_markers[0]),
            ];
            let auth = resolver
                .resolve(&provider)
                .await
                .expect("resolve")
                .expect("ready");
            assert_eq!(auth.kind, AuthKind::Cli);
            assert_eq!(auth.auth, RequestAuth::None);
        }
        let ollama = registry.provider("ollama").expect("ollama");
        let auth = resolver
            .resolve(ollama)
            .await
            .expect("resolve")
            .expect("ready");
        assert_eq!(auth.kind, AuthKind::None);
        assert_eq!(auth.source, "no auth required");
        assert!(!dir.path().join("credentials.json.lock").exists());
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o775
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn api_key_provider_preserves_unsafe_store_error_with_repair_command() {
        use crate::auth::FileCredentialStore;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("config");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o775)).unwrap();
        let resolver = AuthResolver::new(
            Arc::new(FileCredentialStore::in_config_dir(dir.path())),
            reqwest::Client::new(),
        )
        .with_env(|_| Some("env-key".into()));
        let error = resolver
            .resolve(&provider(AuthMethods::api_key(["P_KEY"])))
            .await
            .expect_err("unsafe store must fail");
        assert!(matches!(error, AuthError::Store(_)));
        let detail = error.to_string();
        assert!(detail.contains("group- or world-writable"), "{detail}");
        assert!(
            detail.contains(&super::super::store::credentials_dir_fix(dir.path())),
            "{detail}"
        );
        assert!(!dir.path().join("credentials.json.lock").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn optional_auth_uses_env_key_with_healthy_or_unsafe_store() {
        use crate::auth::FileCredentialStore;
        use std::os::unix::fs::PermissionsExt;

        let p = provider(AuthMethods {
            none: true,
            ..AuthMethods::api_key(["P_KEY"])
        });
        for mode in [0o700, 0o775] {
            let dir = tempfile::tempdir().expect("config");
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode))
                .expect("directory permissions");
            let r = AuthResolver::new(
                Arc::new(FileCredentialStore::in_config_dir(dir.path())),
                reqwest::Client::new(),
            )
            .with_env(|key| (key == "P_KEY").then(|| "env-key".into()));
            let auth = r.resolve(&p).await.expect("resolve").expect("ready");
            assert_eq!(auth.auth, RequestAuth::ApiKey("env-key".into()));
            assert_eq!(auth.source, "P_KEY");
            assert_eq!(auth.kind, AuthKind::ApiKey);
            assert_eq!(
                dir.path().join("credentials.json.lock").exists(),
                mode == 0o700
            );
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }

    #[tokio::test]
    async fn optional_auth_uses_stored_key_before_env_or_none() {
        let r = resolver(vec![("P_KEY", "env-key")]);
        let p = provider(AuthMethods {
            none: true,
            ..AuthMethods::api_key(["P_KEY"])
        });
        r.store()
            .modify(
                "p",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey {
                            key: "stored".into(),
                            env: BTreeMap::new(),
                        }))
                    })
                }),
            )
            .await
            .expect("store");
        let auth = r.resolve(&p).await.expect("resolve").expect("ready");
        assert_eq!(auth.auth, RequestAuth::ApiKey("stored".into()));
        assert_eq!(auth.source, "stored credential");
        assert_eq!(auth.kind, AuthKind::ApiKey);
    }

    #[tokio::test]
    async fn cli_auth_uses_stored_or_env_key_before_cli_status() {
        let mut p = crate::Registry::builtin()
            .provider("codex")
            .expect("CLI provider")
            .clone();
        p.auth.api_key_env = vec!["P_KEY".into()];
        p.auth.cli.as_mut().expect("CLI auth").program = "/missing-cli".into();
        let r = resolver(vec![("P_KEY", "env-key")]);
        let auth = r.resolve(&p).await.expect("resolve").expect("ready");
        assert_eq!(auth.auth, RequestAuth::ApiKey("env-key".into()));
        assert_eq!(auth.source, "P_KEY");
        r.store()
            .modify(
                &p.id,
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey {
                            key: "stored".into(),
                            env: BTreeMap::new(),
                        }))
                    })
                }),
            )
            .await
            .expect("store");
        let auth = r.resolve(&p).await.expect("resolve").expect("ready");
        assert_eq!(auth.auth, RequestAuth::ApiKey("stored".into()));
        assert_eq!(auth.source, "stored credential");
    }

    #[tokio::test]
    async fn env_order_and_none() {
        let r = resolver(vec![("P_KEY", "k1")]);
        let p = provider(AuthMethods::api_key(["P_MISSING", "P_KEY"]));
        let a = r.resolve(&p).await.expect("resolve").expect("some");
        assert_eq!(a.auth, RequestAuth::ApiKey("k1".into()));
        assert_eq!(a.source, "P_KEY");

        let r = resolver(vec![("P_TOKEN", "t"), ("P_KEY", "k")]);
        let p = provider(AuthMethods::api_key(["P_KEY"]).with_bearer_env(["P_TOKEN"]));
        let a = r.resolve(&p).await.expect("resolve").expect("some");
        assert_eq!(a.auth, RequestAuth::Bearer("t".into()));

        let r = resolver(vec![]);
        assert!(
            r.resolve(&provider(AuthMethods::api_key(["P_KEY"])))
                .await
                .expect("resolve")
                .is_none()
        );
        let a = r
            .resolve(&provider(AuthMethods::none()))
            .await
            .expect("resolve")
            .expect("some");
        assert_eq!(a.auth, RequestAuth::None);
    }

    #[tokio::test]
    async fn stored_credential_owns_the_provider() {
        let r = resolver(vec![("P_KEY", "from-env")]);
        let p = provider(AuthMethods::api_key(["P_KEY"]));
        r.store()
            .modify(
                "p",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::ApiKey {
                            key: "stored".into(),
                            env: BTreeMap::new(),
                        }))
                    })
                }),
            )
            .await
            .expect("store");
        let a = r.resolve(&p).await.expect("resolve").expect("some");
        assert_eq!(a.auth, RequestAuth::ApiKey("stored".into()));
        assert_eq!(a.source, "stored credential");
    }

    #[tokio::test]
    async fn expired_oauth_without_refresh_flow_is_an_error_not_a_fallback() {
        let r = resolver(vec![("P_KEY", "from-env")]);
        let p = provider(AuthMethods::api_key(["P_KEY"]));
        r.store()
            .modify(
                "p",
                Box::new(|_| {
                    Box::pin(async {
                        Ok(Some(Credential::OAuth {
                            access: "old".into(),
                            refresh: Some("r".into()),
                            expires_at: Some(
                                aether_core::time::now() - chrono::Duration::minutes(1),
                            ),
                            extra: BTreeMap::new(),
                        }))
                    })
                }),
            )
            .await
            .expect("store");
        let err = r.resolve(&p).await.unwrap_err();
        assert!(matches!(err, AuthError::Refresh { .. }));
    }

    #[tokio::test]
    async fn api_key_login_stores_and_logout_clears() {
        let r = resolver(vec![]);
        let p = provider(AuthMethods::api_key(["P_KEY"]));
        let interaction = crate::auth::interaction::ScriptedInteraction::new(["  sk-new "]);
        let info = r
            .login(
                &p,
                LoginMethod::ApiKey,
                &interaction,
                &CancellationToken::new(),
            )
            .await
            .expect("login");
        assert_eq!(info.kind, AuthKind::ApiKey);
        let a = r.resolve(&p).await.expect("resolve").expect("some");
        assert_eq!(a.auth, RequestAuth::ApiKey("sk-new".into()));
        r.logout("p").await.expect("logout");
        assert!(r.resolve(&p).await.expect("resolve").is_none());

        let err = r
            .login(
                &provider(AuthMethods::none()),
                LoginMethod::ApiKey,
                &interaction,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Unsupported { .. }));
    }
}
