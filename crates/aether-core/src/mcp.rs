//! What Aether knows about a configured MCP server: its health and the tools
//! it offers. Shared by the tool runtime that talks to the server, the events
//! that record its transitions, and every client that displays it.

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;
use crate::tool::SideEffectClass;

/// Whether a configured server is usable right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpServerState {
    /// Configured, not contacted yet.
    Unknown,
    /// Connected, tools discovered.
    Ready,
    /// The last attempt failed; the tools are not offered to any agent.
    Failed,
}

/// Whether a server that names a token variable got its token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthState {
    /// The variable is set; the server is reached with its value.
    Token,
    /// The variable is unset and the token is required: the server is not
    /// contacted, so a typo cannot downgrade it to anonymous access.
    AuthFailed,
    /// The variable is unset and the server is configured `auth = "optional"`:
    /// it is reached without a token.
    Anonymous,
}

/// How a server authenticates. Names the variable, never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpAuth {
    /// The environment variable the token is read from.
    pub token_env: String,
    pub state: McpAuthState,
}

/// The name an agent sees for a server's tool. Namespaced by server, so two
/// servers may offer the same tool name, and restricted to the characters
/// providers accept in a tool name.
pub fn qualified_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize_name(server), sanitize_name(tool))
}

/// One part of a qualified name. The single definition of the mapping:
/// configuration checks two servers for collision with it and the tool
/// registry builds the name with it, so the two cannot drift apart.
pub fn sanitize_name(part: &str) -> String {
    part.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// One tool a server offers, as Aether will expose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpToolInfo {
    /// The name on the server.
    pub name: String,
    /// The name agents see: `mcp__<server>__<tool>`.
    pub qualified_name: String,
    /// How the call is classed: declared in config, else conservative.
    pub side_effect: SideEffectClass,
    /// True when the class came from config rather than the conservative default.
    pub declared: bool,
}

/// A server's health as of the last contact attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpServerHealth {
    pub name: String,
    pub state: McpServerState,
    /// Human-readable transport, e.g. `stdio: npx …` or `http://…`.
    pub transport: String,
    /// Empty unless the state is `Ready`.
    #[serde(default)]
    pub tools: Vec<McpToolInfo>,
    /// Why the last attempt failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the state was last established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<Timestamp>,
    /// Present when the server is configured with a token variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpAuth>,
}

impl McpServerHealth {
    /// A configured server nobody has contacted yet.
    pub fn unknown(name: impl Into<String>, transport: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: McpServerState::Unknown,
            transport: transport.into(),
            tools: Vec::new(),
            error: None,
            checked_at: None,
            auth: None,
        }
    }

    /// True when the server was not contacted because its token is missing.
    pub fn auth_failed(&self) -> bool {
        self.auth
            .as_ref()
            .is_some_and(|auth| auth.state == McpAuthState::AuthFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_namespaced_and_safe_for_providers() {
        assert_eq!(
            qualified_name("my-server", "read.file"),
            "mcp__my_server__read_file"
        );
        assert!(
            qualified_name("a b", "c/d")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        );
        // Case is part of the name: `Files` and `files` do not collide.
        assert_ne!(sanitize_name("Files"), sanitize_name("files"));
        // Anything else does: `read.file` and `read_file` do.
        assert_eq!(sanitize_name("read.file"), sanitize_name("read_file"));
    }
}
