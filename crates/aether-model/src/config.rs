//! Configuration directory, `config.toml`, and the commented templates
//! written on first start so users can see what is configurable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aether_core::agent::{ApprovalDefault, NetworkPolicy};
use aether_core::tool::SideEffectClass;
use serde::{Deserialize, Serialize};

use crate::profile::ProfileSetting;

mod agents;
mod build_cache;
mod providers;
mod remote;
mod retention;
mod scheduler;
mod storage;
mod templates;

pub use agents::AgentsSection;
pub use build_cache::{BuildCacheSection, BuildCacheVar};
pub use providers::ProvidersSection;
pub use remote::RemoteSection;
pub use retention::RetentionSection;
pub use storage::StorageSection;

pub use scheduler::{
    DEFAULT_MAX_CONCURRENT_TURNS, MAX_ENGINE_PARK_GRACE_SECS, MAX_WAIT_TIMEOUT_MINUTES,
    SchedulerSection,
};
pub use templates::{
    CONFIG_TEMPLATE, PROVIDERS_CLAUDE_1M_EXAMPLE, PROVIDERS_EXAMPLE, ensure_templates,
    providers_template,
};

/// `$AETHER_CONFIG_DIR`, else `~/.config/aether`.
pub fn config_dir(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    if let Some(dir) = std::env::var_os("AETHER_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aether")
}

/// Create missing configuration directories private to their owner on Unix.
/// Existing directories keep their permissions; the credential store validates them.
pub fn create_config_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        builder.recursive(true).create(parent)?;
    }
    match builder.recursive(false).create(dir) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::metadata(dir)?.is_dir() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

// The TUI reads only [theme] from this file; ui.toml and voice.toml are separate.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AetherConfig {
    #[serde(skip_serializing)]
    pub theme: toml::Table,
    pub providers: ProvidersSection,
    pub model: ModelSection,
    pub models: ModelsSection,
    pub approval: ApprovalSection,
    pub network: NetworkSection,
    pub scheduler: SchedulerSection,
    pub web: WebSection,
    pub artifacts: ArtifactsSection,
    pub retention: RetentionSection,
    pub storage: StorageSection,
    pub agents: AgentsSection,
    pub build_cache: BuildCacheSection,
    pub mcp: McpSection,
    pub memory: MemorySection,
    pub listen: ListenSection,
    pub remote: RemoteSection,
    pub serve: ServeSection,
}

/// Default scope for new roots; explicit scopes and existing agents are unchanged.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSection {
    pub default: NetworkPolicy,
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            default: NetworkPolicy::Allowed,
        }
    }
}

/// `aether serve`: webhooks in, event subscriptions out. Both doors are off
/// until this section names them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServeSection {
    /// `host:port` for the inbound listener. Required when a webhook is
    /// configured; unset serves the outbound half only.
    pub address: Option<String>,
    /// Bind outside loopback, acknowledging that Aether terminates no TLS.
    pub allow_remote: bool,
    #[serde(rename = "webhook")]
    pub webhooks: Vec<WebhookSection>,
    #[serde(rename = "subscription")]
    pub subscriptions: Vec<SubscriptionSection>,
}

/// One inbound hook. The secret never appears here: name the environment
/// variable holding it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSection {
    pub path: String,
    /// An agent id, or the name of exactly one live agent.
    pub agent: String,
    /// Recorded on the inbox item so an agent can tell its callers apart.
    pub source: String,
    pub secret_env: String,
}

/// One outbound subscription.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionSection {
    pub url: String,
    /// Event kinds to post, by their `type` tag. Empty posts all of them.
    pub events: Vec<String>,
    /// Environment variable holding the signing secret. Required: an
    /// unsigned delivery tells the receiver nothing about who sent it, and
    /// an event body carries model content.
    pub secret_env: Option<String>,
    /// Attempts per event, including the first.
    pub max_attempts: u32,
    /// Where an event that never landed is appended, one JSON line each.
    pub dead_letter: Option<PathBuf>,
}

impl Default for SubscriptionSection {
    fn default() -> Self {
        Self {
            url: String::new(),
            events: Vec::new(),
            secret_env: None,
            max_attempts: 5,
            dead_letter: None,
        }
    }
}

impl ServeSection {
    /// Where the inbound listener binds, and whether the section is coherent.
    pub fn address(&self) -> Result<Option<std::net::SocketAddr>, String> {
        let parsed = match self
            .address
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            Some(address) => Some(address.parse().map_err(|_| {
                format!("[serve] address `{address}` is not a host:port, e.g. \"127.0.0.1:8787\"")
            })?),
            None => None,
        };
        if parsed.is_none() && !self.webhooks.is_empty() {
            return Err("[serve] configures webhooks but no address to receive them on".into());
        }
        for hook in &self.webhooks {
            if !hook.path.starts_with('/') {
                return Err(format!(
                    "[serve] webhook path `{}` must start with /",
                    hook.path
                ));
            }
        }
        for subscription in &self.subscriptions {
            if subscription.url.is_empty() {
                return Err("[serve] a subscription has no url".into());
            }
            if subscription.max_attempts == 0 {
                return Err(format!(
                    "[serve] subscription {} must allow at least one attempt",
                    subscription.url
                ));
            }
            if subscription
                .secret_env
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
            {
                return Err(format!(
                    "[serve] subscription {} needs secret_env: an unsigned delivery tells the \
                     receiver nothing about who sent it",
                    subscription.url
                ));
            }
        }
        Ok(parsed)
    }
}

/// How durable memory is retrieved. Structural and lexical retrieval always
/// run; `index` adds an optional semantic pass on top.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySection {
    /// `none` (the default) or `local`: a deterministic on-machine embedder
    /// with no network and no key. A provider-backed index plugs in behind
    /// the same `MemoryIndex` trait.
    pub index: MemoryIndexChoice,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryIndexChoice {
    #[default]
    None,
    Local,
}

/// Shortest idle timeout a client can hold a connection open under. Every
/// first-party client pings at a third of the timeout and skips the keepalive
/// entirely below this, so the config floor and the clients' floor are one
/// number.
pub const MIN_IDLE_TIMEOUT_SECS: u64 = 3;

/// The daemon's optional second transport: the protocol over TCP, behind a
/// token, so a TUI or an orchestrator on another machine can attach through
/// an SSH tunnel. Absent `address`, the daemon serves its Unix socket only.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ListenSection {
    /// `host:port` to bind. Unset means no TCP listener at all.
    pub address: Option<String>,
    /// Environment variable holding the token. Aether stores no secret, so
    /// the token itself never appears in this file.
    pub token_env: String,
    /// Bind outside loopback, acknowledging that Aether terminates no TLS.
    pub allow_remote: bool,
    /// Connections served at once; more wait in the kernel's backlog.
    pub max_connections: usize,
    /// How long a connection may go without sending a frame. Clients hold a
    /// quiet connection open by pinging.
    pub idle_timeout_secs: u64,
}

/// Defaults chosen to be safe rather than convenient: no listener, and a
/// bind that must be acknowledged before it leaves this machine.
impl Default for ListenSection {
    fn default() -> Self {
        Self {
            address: None,
            token_env: "AETHER_TOKEN".into(),
            allow_remote: false,
            max_connections: 64,
            idle_timeout_secs: 300,
        }
    }
}

impl ListenSection {
    /// Where to bind, when this daemon listens on TCP at all. A value that
    /// cannot mean what it says is reported, not silently dropped.
    pub fn address(&self) -> Result<Option<std::net::SocketAddr>, String> {
        let Some(address) = self
            .address
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        else {
            return Ok(None);
        };
        let parsed = address.parse().map_err(|_| {
            format!("[listen] address `{address}` is not a host:port, e.g. \"127.0.0.1:7777\"")
        })?;
        // Not 1: every first-party client skips its keepalive below three
        // seconds, so a shorter timeout drops each of them on a timer with
        // nothing they can do about it.
        if self.idle_timeout_secs < MIN_IDLE_TIMEOUT_SECS {
            return Err(format!(
                "[listen] idle_timeout_secs must be at least {MIN_IDLE_TIMEOUT_SECS}; below that \
                 the clients' keepalive does not run and every connection is dropped on the timer"
            ));
        }
        Ok(Some(parsed))
    }

    /// The token, from the environment variable this section names.
    pub fn token(&self) -> Result<String, String> {
        std::env::var(&self.token_env)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                format!(
                    "[listen] address is set but {} is unset; the TCP transport is the whole \
                     fleet behind one token and will not open without it",
                    self.token_env
                )
            })
    }
}

/// MCP servers Aether connects to as a client.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpSection {
    pub servers: Vec<McpServerSection>,
}

impl McpSection {
    /// Two servers whose names collapse to the same tool prefix would shadow
    /// each other silently, so they are refused while the person is looking.
    /// Collapsed with the same mapping the tool name is built from, so this
    /// refuses exactly the pairs that would collide - no more, no less.
    pub fn check_names_are_distinct(&self) -> Result<(), String> {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for server in &self.servers {
            let key = aether_core::mcp::sanitize_name(&server.name);
            if let Some(first) = seen.insert(key, &server.name) {
                return Err(format!(
                    "mcp servers `{first}` and `{}` name the same tools",
                    server.name
                ));
            }
        }
        Ok(())
    }
}

/// Server names Aether uses for its own surface. A server called `aether`
/// would have the calls an engine made itself recorded with a built-in's
/// side-effect class, because that is the prefix the engine contract puts on
/// Aether's own tools.
const RESERVED_SERVER_NAMES: [&str; 1] = ["aether"];

/// Whether an MCP server's token is required.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthMode {
    #[default]
    Required,
    Optional,
}

/// One configured server. Exactly one transport: `command` or `url`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerSection {
    pub name: String,
    /// stdio transport: the program to run.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment for a stdio server, merged over the daemon's own.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory for a stdio server.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Streamable HTTP transport: the endpoint.
    #[serde(default)]
    pub url: Option<String>,
    /// Environment variable holding the bearer token for an HTTP server.
    /// Aether stores no credential; the value is read at daemon start.
    #[serde(default)]
    pub token_env: Option<String>,
    /// Whether the server may be reached without its token when `token_env`
    /// is unset. Required by default: the server is then not contacted.
    #[serde(default)]
    pub auth: McpAuthMode,
    /// Side-effect class per tool name. Anything not named here is treated as
    /// a non-idempotent mutation and needs approval.
    #[serde(default)]
    pub classes: BTreeMap<String, SideEffectClass>,
    /// Name the calling agent in each `tools/call`'s `params._meta`. Opt-in:
    /// some servers reject params they do not know.
    #[serde(default)]
    pub send_caller_meta: bool,
}

impl McpServerSection {
    /// Refuse a server that names no transport or names both: the ambiguity
    /// would otherwise be resolved silently. Also refuses a reserved name and
    /// a url that would put the conversation, and any bearer token with it,
    /// in cleartext across a network.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("an mcp server needs a name".into());
        }
        let collapsed = aether_core::mcp::sanitize_name(self.name.trim());
        if RESERVED_SERVER_NAMES
            .iter()
            .any(|reserved| collapsed.eq_ignore_ascii_case(reserved))
        {
            return Err(format!(
                "`{}` is reserved: it is how Aether's own tools reach an engine, and a server \
                 with that name would have its calls recorded as theirs",
                self.name
            ));
        }
        if self
            .token_env
            .as_deref()
            .is_some_and(|var| var.trim().is_empty())
        {
            return Err(format!(
                "mcp server `{}` names an empty `token_env`",
                self.name
            ));
        }
        if self.auth == McpAuthMode::Optional && self.token_env.is_none() {
            return Err(format!(
                "mcp server `{}` sets `auth = \"optional\"` but names no `token_env`",
                self.name
            ));
        }
        match (&self.command, &self.url) {
            (Some(_), None) => Ok(()),
            (None, Some(url)) => check_url(&self.name, url),
            (Some(_), Some(_)) => Err(format!(
                "mcp server `{}` names both a command and a url; pick one",
                self.name
            )),
            (None, None) => Err(format!(
                "mcp server `{}` names neither a command nor a url",
                self.name
            )),
        }
    }
}

/// An MCP endpoint is reached with a bearer token and carries the agent's
/// data both ways, so plain HTTP is accepted only where it cannot leave the
/// machine.
fn check_url(server: &str, url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("mcp server `{server}` has an unusable url: {e}"))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&parsed) => Ok(()),
        "http" => Err(format!(
            "mcp server `{server}` is plain http to `{}`; the conversation and any bearer \
             token would cross the network in cleartext - use https, or a loopback address",
            parsed.host_str().unwrap_or("?")
        )),
        other => Err(format!(
            "mcp server `{server}` speaks `{other}`; only https, or http to loopback, is served"
        )),
    }
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost")
                || name.to_ascii_lowercase().ends_with(".localhost")
        }
        None => false,
    }
}

/// How long artifact bytes are kept. Records are never removed.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactsSection {
    /// Days of bytes to keep; 0 keeps them forever.
    pub retain_days: u32,
    /// Total bytes to keep; 0 means no size limit.
    pub retain_bytes: u64,
}

impl Default for ArtifactsSection {
    fn default() -> Self {
        Self {
            retain_days: 30,
            retain_bytes: 2 << 30,
        }
    }
}

/// Web tools. `web_fetch` always exists; `web_search` needs a provider, and
/// without one the tool is not offered at all.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSection {
    pub search: Option<WebSearchSection>,
}

/// Where a search goes and how to read the answer. Aether ships no provider
/// and stores no credential: `api_key_env` names the environment variable
/// holding it, read at call time.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchSection {
    pub endpoint: String,
    #[serde(default = "default_query_param")]
    pub query_param: String,
    /// JSON pointer to the results array, e.g. `/web/results`.
    pub results_pointer: String,
    #[serde(default = "default_title_field")]
    pub title_field: String,
    #[serde(default = "default_url_field")]
    pub url_field: String,
    #[serde(default = "default_snippet_field")]
    pub snippet_field: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_key_header")]
    pub api_key_header: String,
}

fn default_query_param() -> String {
    "q".into()
}
fn default_title_field() -> String {
    "title".into()
}
fn default_url_field() -> String {
    "url".into()
}
fn default_snippet_field() -> String {
    "description".into()
}
fn default_key_header() -> String {
    "Authorization".into()
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalSection {
    /// Skip every approval for every agent, with no per-agent override:
    /// the same as `--dangerously-skip-permissions` elsewhere.
    pub yolo: bool,
    /// The policy a new agent gets when it inherits none. Ships as `yolo`;
    /// `/yolo` and `--no-yolo` still move an individual agent either way.
    pub default: ApprovalDefault,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSection {
    /// `provider/model` used when an agent does not name one.
    pub default: Option<String>,
    /// Model used by services that prefer a cheaper model (compaction).
    pub compactor: Option<String>,
    /// Defaults to medium independently of the active agent effort.
    pub compactor_effort: Option<aether_core::model::Effort>,
    /// Which families' authored profiles are sent. Defaults are per family and
    /// earned by a committed eval; an explicit setting here wins over them.
    pub profiles: ProfileSetting,
    /// Optional Claude Code cache-write tier; unset preserves native settings.
    pub prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
}

/// `[models]`: per-model catalog overrides.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelsSection {
    /// `"provider/model" = "fast" | "balanced" | "strong" | "frontier"`
    /// (`strongest` is read as `strong`); beats the catalog's own tier.
    pub tiers: BTreeMap<String, crate::tier::ModelTier>,
}

impl ModelsSection {
    fn validate(&self) -> Result<(), String> {
        match self
            .tiers
            .keys()
            .find(|key| aether_core::model::ModelRef::parse(key).is_none())
        {
            Some(key) => Err(format!("models.tiers key `{key}` must be provider/model")),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct ConfigError {
    pub path: String,
    pub reason: String,
}

pub fn load_config(dir: &Path) -> Result<AetherConfig, ConfigError> {
    let path = dir.join("config.toml");
    let bad = |reason: String| ConfigError {
        path: path.display().to_string(),
        reason,
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let config: AetherConfig =
                toml::from_str(&text).map_err(|mut e: toml::de::Error| {
                    // Source excerpts hide the section path on unknown-field errors.
                    if e.message().starts_with("unknown field `") {
                        e.set_input(None);
                    }
                    bad(e.to_string())
                })?;
            if let Some(model) = &config.model.compactor
                && aether_core::model::ModelRef::parse(model).is_none()
            {
                return Err(bad("model.compactor must be provider/model".into()));
            }
            config.models.validate().map_err(&bad)?;
            config.providers.validate().map_err(&bad)?;
            config.scheduler.wait_timeout().map_err(&bad)?;
            config.scheduler.engine_park_grace().map_err(&bad)?;
            config.scheduler.engine_transient_retry().map_err(&bad)?;
            config.scheduler.concurrency().map_err(&bad)?;
            config.scheduler.engine_concurrency().map_err(&bad)?;
            config.scheduler.run_budget().map_err(&bad)?;
            config.scheduler.disk_floor().map_err(&bad)?;
            config.listen.address().map_err(&bad)?;
            config.remote.address().map_err(&bad)?;
            config.serve.address().map_err(&bad)?;
            config.agents.validate().map_err(&bad)?;
            config.storage.validate().map_err(&bad)?;
            config.build_cache.validate().map_err(&bad)?;
            for server in &config.mcp.servers {
                server.validate().map_err(&bad)?;
            }
            config.mcp.check_names_are_distinct().map_err(&bad)?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AetherConfig::default()),
        Err(e) => Err(ConfigError {
            path: path.display().to_string(),
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
