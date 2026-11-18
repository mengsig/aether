//! Built-in tools: world tools that touch the filesystem or shell, and
//! harness tools that propose changes to authoritative state.

use std::sync::Arc;

use aether_core::model::ToolSpec;
use aether_core::tool::{SideEffectClass, ToolDescriptor};
use serde_json::Value;

use crate::tool::{Tool, ToolError};

pub mod bash;
pub mod edit_file;
pub mod glob;
pub mod grep;
pub mod harness;
pub mod inspection;
pub mod list_dir;
pub mod read_file;
mod read_only;
pub mod schedules;
pub mod shell_bg;
pub mod web;
pub mod write_file;

pub(crate) fn descriptor(
    name: &str,
    description: &str,
    input_schema: Value,
    side_effect: SideEffectClass,
    needs_network: bool,
) -> ToolDescriptor {
    ToolDescriptor {
        spec: ToolSpec {
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema,
            strict: false,
        },
        side_effect,
        needs_network,
    }
}

/// Parse a schema field that is a typed id or a closed, string-tagged enum,
/// naming the exact `tool.field` so a rejection tells a model which argument
/// to fix. Takes the raw `Value` (not a pre-extracted `String`) so a
/// wrong-typed value - not just a wrong-valued one - also gets the field
/// name attached: routing a `String`-typed struct field through serde first
/// would reject a non-string value before this ever runs, the same
/// field-dropping failure mode this exists to fix. Reuses the target type's
/// own `Deserialize` (both typed ids and their enums have one), so aliases,
/// `rename_all` and the id's accepted prefix stay defined in one place.
pub(crate) fn parse_field<T: serde::de::DeserializeOwned>(
    value: &Value,
    tool_field: &str,
) -> Result<T, ToolError> {
    serde_json::from_value(value.clone())
        .map_err(|e| ToolError::InvalidInput(format!("{tool_field}: {e}")))
}

/// The scope a caller asked a run to start in: its own, narrowed by the
/// fields it set, or `None` when it set none and the run inherits.
pub(crate) fn requested_scope(
    ctx: &crate::tool::ToolContext,
    workspace_root: Option<&str>,
    allowed_paths: &[String],
    network: Option<bool>,
) -> Option<aether_core::agent::Scope> {
    use aether_core::agent::NetworkPolicy;
    if workspace_root.is_none() && allowed_paths.is_empty() && network.is_none() {
        return None;
    }
    let mut scope = ctx.scope.clone();
    if let Some(root) = workspace_root {
        scope.workspace_root = Some(crate::fs::lexical_resolve(ctx, root));
    }
    if !allowed_paths.is_empty() {
        scope.allowed_paths = allowed_paths
            .iter()
            .map(|p| crate::fs::lexical_resolve(ctx, p))
            .collect();
    }
    if let Some(allowed) = network {
        scope.network = if allowed {
            NetworkPolicy::Allowed
        } else {
            NetworkPolicy::Denied
        };
    }
    Some(scope)
}

/// Tools that act on the workspace and shell. The shell tools share one
/// background-process registry, so a process started by `bash` is the same
/// one `shell_output` and `shell_stop` see.
pub fn world_tools(processes: Arc<crate::background::BackgroundProcesses>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(web::WebFetchTool::default()),
        Arc::new(bash::BashTool::new(processes.clone())),
        Arc::new(shell_bg::ShellOutputTool::new(processes.clone())),
        Arc::new(shell_bg::ShellListTool::new(processes.clone())),
        Arc::new(shell_bg::WriteStdinTool::new(processes.clone())),
        Arc::new(shell_bg::ShellStopTool::new(processes)),
        Arc::new(read_file::ReadFileTool::default()),
        Arc::new(write_file::WriteFileTool),
        Arc::new(edit_file::EditFileTool::default()),
        Arc::new(list_dir::ListDirTool),
        Arc::new(glob::GlobTool),
        Arc::new(grep::GrepTool),
    ]
}

/// Tools that propose harness actions for the runtime to record.
pub fn harness_tools() -> Vec<Arc<dyn Tool>> {
    harness::all()
}

pub fn builtin_tools(processes: Arc<crate::background::BackgroundProcesses>) -> Vec<Arc<dyn Tool>> {
    let mut all = world_tools(processes);
    all.extend(harness_tools());
    all
}

/// Path relative to the workspace for display, falling back to the full path.
pub(crate) fn display_path(ctx: &crate::tool::ToolContext, path: &std::path::Path) -> String {
    let root = ctx.scope.workspace_root.as_deref().unwrap_or(&ctx.cwd);
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// True when the first bytes look like binary content.
pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
mod special_files;
