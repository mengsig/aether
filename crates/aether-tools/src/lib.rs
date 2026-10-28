//! Tool runtime for Aether: the tool contract, a registry, conflict-aware
//! batch execution with timeouts and cancellation, scoped filesystem access,
//! output bounding and redaction, and the built-in tool set.
//!
//! Tools never touch authoritative harness state. World tools return a
//! [`ToolOutcome`]; harness tools return a [`HarnessAction`] that the agent
//! runtime validates and records.

pub mod background;
pub mod builtin;
mod capture;
pub mod exec;
pub mod fs;
mod git_guard;
pub mod mcp;
pub mod output;
pub mod registry;
pub mod runtime;
mod terminal;
#[cfg(test)]
pub(crate) mod testing;
pub mod tool;
mod walk;

pub use aether_core::tool::ToolOutcome;
pub use background::{BackgroundProcesses, ProcessOutput, ProcessStatus};
pub use builtin::bash::shell_resources;
pub use builtin::web::SearchProvider;
pub use exec::{PreparedCommand, Teardown};
pub use mcp::{McpCaller, McpError, McpManager, McpServerSpec, McpTransportSpec};
pub use output::{BoundedOutput, OutputPolicy, Redactor};
pub use registry::ToolRegistry;
pub use runtime::{
    BatchResult, ExecutionPolicy, ToolRuntime, redact_json, redact_outcome, requires_approval,
};
pub use tool::{
    ArtifactSink, DelegateKey, HarnessAction, Tool, ToolContext, ToolError, ToolExecution,
    WorkspaceMode, parse_input,
};
