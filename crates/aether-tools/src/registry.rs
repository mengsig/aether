//! Registry of available tools, filtered per agent by exposure and scope.

use std::collections::BTreeMap;
use std::sync::Arc;

use aether_core::agent::{ExecutionBackend, NetworkPolicy, Scope};
use aether_core::model::ToolSpec;
use aether_core::tool::{ToolDescriptor, ToolExposure};

use crate::background::BackgroundProcesses;
use crate::builtin::web::SearchProvider;
use crate::mcp::McpManager;
use crate::tool::Tool;

/// Variables that would let a child reach the daemon that spawned it.
/// Stripped from every tool child, so an agent cannot drive its own daemon
/// (rule changes and approvals are person-level operations); the container
/// backend, not this list, is the isolation boundary. The daemon adds the
/// secrets its own config names on top, through [`ToolRegistry::hiding`].
pub const DAEMON_ENV: [&str; 3] = ["AETHER_SOCKET", "AETHER_CONFIG_DIR", "AETHER_DATA_DIR"];

#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    /// The registry the built-in shell tools share, when they are registered.
    /// The runtime needs it to end an agent's processes with the agent.
    background: Option<Arc<BackgroundProcesses>>,
    /// Variables no tool child may see; see `hidden_env`.
    hidden_env: Arc<[String]>,
    /// Engines may need provider credentials, but never daemon credentials.
    engine_hidden_env: Arc<[String]>,
    /// Configured MCP servers. Their tools are not stored here: the manager
    /// is asked each time, so a rediscovery takes effect at once and there is
    /// no second copy of the tool list to drift.
    mcp: Option<Arc<McpManager>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: BTreeMap::new(),
            background: None,
            hidden_env: DAEMON_ENV.iter().map(|v| (*v).to_owned()).collect(),
            engine_hidden_env: DAEMON_ENV.iter().map(|v| (*v).to_owned()).collect(),
            mcp: None,
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The built-in world tools and harness tools, sharing one background
    /// process registry.
    pub fn with_builtins() -> Self {
        Self::with_builtins_and_search(None)
    }

    /// The built-ins plus `web_search`, when a provider is configured.
    /// Without one the tool is absent rather than present and failing, so a
    /// model is never offered a search it cannot run.
    pub fn with_builtins_and_search(search: Option<SearchProvider>) -> Self {
        let background = Arc::new(BackgroundProcesses::new());
        let mut r = Self::new();
        for tool in crate::builtin::builtin_tools(background.clone()) {
            r.register(tool);
        }
        if let Some(provider) = search {
            if let Some(var) = provider.api_key_env.clone() {
                r = r.hiding([var]);
            }
            r.register(Arc::new(crate::builtin::web::WebSearchTool::new(provider)));
        }
        r.background = Some(background);
        r
    }

    /// Add variables hidden from tool, MCP and engine children.
    /// Additive: daemon reach and previously hidden credentials stay hidden.
    pub fn hiding(mut self, names: impl IntoIterator<Item = String>) -> Self {
        let names: Vec<_> = names.into_iter().collect();
        self.engine_hidden_env = self
            .engine_hidden_env
            .iter()
            .cloned()
            .chain(names.clone())
            .collect();
        self.hidden_env = self.hidden_env.iter().cloned().chain(names).collect();
        self
    }

    /// Hide provider credentials from tools and MCP while engines retain their auth.
    pub fn hiding_from_tools(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.hidden_env = self.hidden_env.iter().cloned().chain(names).collect();
        self
    }

    /// Variable names that must be stripped from every tool child's
    /// environment: the daemon's own reach plus any configured credential.
    /// Goes on each `ToolContext` and MCP server the runtime builds.
    pub fn hidden_env(&self) -> Arc<[String]> {
        self.hidden_env.clone()
    }

    /// Engine children strip daemon and search secrets while retaining provider auth.
    pub fn engine_hidden_env(&self) -> Arc<[String]> {
        self.engine_hidden_env.clone()
    }

    /// Serve the tools of these MCP servers alongside the built-ins.
    pub fn with_mcp(mut self, mcp: Arc<McpManager>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Configured MCP servers, when any were registered.
    pub fn mcp(&self) -> Option<&Arc<McpManager>> {
        self.mcp.as_ref()
    }

    /// Background shell processes, when the built-in shell tools are present.
    pub fn background(&self) -> Option<&Arc<BackgroundProcesses>> {
        self.background.as_ref()
    }

    /// Register a tool; a later registration with the same name replaces the earlier.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.descriptor().spec.name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        match self.tools.get(name) {
            Some(tool) => Some(tool.clone()),
            None => self.mcp.as_ref().and_then(|m| m.tool(name)),
        }
    }

    /// Every tool that exists right now, built-ins plus the MCP servers',
    /// in name order.
    fn all(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = self.tools.values().cloned().collect();
        if let Some(mcp) = &self.mcp {
            tools.extend(mcp.tools());
            tools.sort_by_key(|t| t.descriptor().spec.name);
        }
        tools
    }

    pub fn names(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .map(|t| t.descriptor().spec.name)
            .collect()
    }

    /// Tools visible to an agent: allowed by its exposure and usable in its scope.
    pub fn visible(
        &self,
        exposure: &ToolExposure,
        scope: &Scope,
    ) -> Vec<(Arc<dyn Tool>, ToolDescriptor)> {
        self.all()
            .into_iter()
            .map(|t| {
                let d = t.descriptor();
                (t, d)
            })
            .filter(|(t, d)| withheld(t.as_ref(), d, exposure, scope).is_none())
            .collect()
    }

    /// Why a tool that exists is not offered to this agent. `None` when it is
    /// offered, or when no such tool exists at all.
    pub fn withheld_reason(
        &self,
        name: &str,
        exposure: &ToolExposure,
        scope: &Scope,
    ) -> Option<String> {
        let tool = self.get(name)?;
        let descriptor = tool.descriptor();
        withheld(tool.as_ref(), &descriptor, exposure, scope)
    }

    pub fn descriptors_for(&self, exposure: &ToolExposure, scope: &Scope) -> Vec<ToolDescriptor> {
        self.visible(exposure, scope)
            .into_iter()
            .map(|(_, d)| d)
            .collect()
    }

    /// Model-facing specs for an agent, in stable name order.
    pub fn specs_for(&self, exposure: &ToolExposure, scope: &Scope) -> Vec<ToolSpec> {
        self.visible(exposure, scope)
            .into_iter()
            .map(|(_, d)| d.spec)
            .collect()
    }
}

/// How an agent asks a question and waits for the answer. No tool exposure
/// withholds them: nothing smaller than a budget or a cancel stops a question.
pub const ASKING_TOOLS: &[&str] = &["send_message", "wait_for", "ask_user"];

/// Why an existing tool is kept from an agent, in words a person and a model
/// can both act on. The single definition of "offered": [`ToolRegistry::visible`]
/// is this filter, so nothing can be reachable that was never advertised.
fn withheld(
    tool: &dyn Tool,
    descriptor: &ToolDescriptor,
    exposure: &ToolExposure,
    scope: &Scope,
) -> Option<String> {
    let name = descriptor.spec.name.as_str();
    if !exposure.allows(name) && !ASKING_TOOLS.contains(&name) {
        return Some("this agent's tool exposure does not include it".to_owned());
    }
    if descriptor.needs_network && scope.network != NetworkPolicy::Allowed {
        return Some("it reaches the network and this agent's scope denies network".to_owned());
    }
    if tool.escapes_backend() && scope.backend != ExecutionBackend::Host {
        return Some(
            "it runs outside this agent's container backend, on the daemon's host".to_owned(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn the_daemons_own_reach_is_hidden_from_every_child_alongside_the_credential() {
        let plain = ToolRegistry::with_builtins();
        for var in DAEMON_ENV {
            assert!(
                plain.hidden_env().contains(&var.to_owned()),
                "{var} must never reach a tool child"
            );
        }
        let with_key = ToolRegistry::with_builtins_and_search(Some(SearchProvider {
            endpoint: "https://example.com/s".into(),
            query_param: "q".into(),
            results_pointer: "/results".into(),
            title_field: "title".into(),
            url_field: "url".into(),
            snippet_field: "description".into(),
            api_key_env: Some("SEARCH_KEY".into()),
            api_key_header: "Authorization".into(),
        }));
        // Configuring a credential adds to the list, never replaces it.
        assert!(with_key.hidden_env().contains(&"SEARCH_KEY".to_owned()));
        assert!(
            with_key
                .engine_hidden_env()
                .contains(&"SEARCH_KEY".to_owned())
        );
        for var in DAEMON_ENV {
            assert!(with_key.hidden_env().contains(&var.to_owned()));
        }
    }

    #[test]
    fn daemon_secrets_stay_hidden_from_engines_when_also_named_by_a_provider() {
        for provider_first in [false, true] {
            let registry = ToolRegistry::new();
            let registry = if provider_first {
                registry
                    .hiding_from_tools(["SHARED".into()])
                    .hiding(["SHARED".into()])
            } else {
                registry
                    .hiding(["SHARED".into()])
                    .hiding_from_tools(["SHARED".into()])
            };
            assert!(registry.hidden_env().contains(&"SHARED".to_owned()));
            assert!(registry.engine_hidden_env().contains(&"SHARED".to_owned()));
        }
    }

    #[test]
    fn network_scope_decides_whether_the_web_tools_exist_for_an_agent() {
        use aether_core::agent::NetworkPolicy;
        let r = ToolRegistry::with_builtins_and_search(Some(SearchProvider {
            endpoint: "https://example.com/s".into(),
            query_param: "q".into(),
            results_pointer: "/results".into(),
            title_field: "title".into(),
            url_field: "url".into(),
            snippet_field: "description".into(),
            api_key_env: None,
            api_key_header: "Authorization".into(),
        }));
        let names = |scope: &Scope| -> BTreeSet<String> {
            r.specs_for(&ToolExposure::All, scope)
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        let denied = names(&Scope::default());
        assert!(!denied.contains("web_fetch"));
        assert!(!denied.contains("web_search"));
        let allowed = names(&Scope {
            network: NetworkPolicy::Allowed,
            ..Default::default()
        });
        assert!(allowed.contains("web_fetch"));
        assert!(allowed.contains("web_search"));
        // Without a configured provider the search tool does not exist at all.
        assert!(
            !ToolRegistry::with_builtins()
                .specs_for(
                    &ToolExposure::All,
                    &Scope {
                        network: NetworkPolicy::Allowed,
                        ..Default::default()
                    }
                )
                .iter()
                .any(|s| s.name == "web_search")
        );
    }

    #[test]
    fn builtins_register_and_filter() {
        let r = ToolRegistry::with_builtins();
        assert!(r.get("bash").is_some());
        assert!(r.get("delegate").is_some());
        let all = r.specs_for(&ToolExposure::All, &Scope::default());
        assert!(all.iter().any(|s| s.name == "read_file"));
        // The background shell tools need no network, so they are there even
        // for the default, network-denied scope.
        for name in ["shell_output", "shell_processes", "shell_stop"] {
            assert!(r.get(name).is_some(), "{name} is not registered");
            assert!(
                all.iter().any(|s| s.name == name),
                "{name} is not offered to a network-denied agent"
            );
        }
        let only = ToolExposure::Only {
            names: BTreeSet::from(["read_file".to_owned()]),
        };
        let names: Vec<_> = r
            .specs_for(&only, &Scope::default())
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["ask_user", "read_file", "send_message", "wait_for"]);
        let except = ToolExposure::Except {
            names: BTreeSet::from(["bash".to_owned()]),
        };
        assert!(
            !r.specs_for(&except, &Scope::default())
                .iter()
                .any(|s| s.name == "bash")
        );
    }

    #[test]
    fn no_exposure_withholds_a_way_to_ask() {
        let r = ToolRegistry::with_builtins();
        let except = ToolExposure::Except {
            names: ASKING_TOOLS.iter().map(|n| (*n).to_owned()).collect(),
        };
        let denied = Scope::default();
        assert_ne!(denied.network, NetworkPolicy::Allowed);
        for name in ASKING_TOOLS {
            assert!(r.get(name).is_some(), "{name} is not registered");
            assert_eq!(r.withheld_reason(name, &except, &denied), None, "{name}");
        }
    }
}
