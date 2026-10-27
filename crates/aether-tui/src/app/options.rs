//! Application options.
use super::{AgentId, ModelRef, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct TuiOptions {
    pub view_modes: crate::view_modes::Catalog,
    pub view_mode: Option<String>,
    pub keys: std::collections::BTreeMap<String, toml::Value>,
    /// Agent to select first.
    pub focus: Option<AgentId>,
    /// Model for agents the TUI creates, and the label shown when nothing is selected.
    pub default_model: Option<ModelRef>,
    /// Models the first agent falls back to, in order.
    pub default_fallbacks: Vec<ModelRef>,
    /// Models chosen recently, newest first, from `ui.toml`.
    pub recent_models: Vec<String>,
    /// Start with cleared work shown, from `ui.toml`.
    pub show_archived: bool,
    /// Create a root agent when the daemon has no live one.
    pub auto_create_root: bool,
    /// Socket used for reconnection after the daemon restarts.
    pub socket_path: Option<PathBuf>,
    pub endpoint: Option<aether_client::Endpoint>,
    pub instance_id: Option<String>,
    pub resume_seq: Option<u64>,
    pub draft: Option<String>,
    /// Preserve an explicitly selected socket in copyable command hints.
    pub explicit_socket: bool,
    /// What the mouse does: off (native selection), or capture for scrolling and selection.
    pub mouse: crate::prefs::MouseMode,
    /// Start agents with approvals off, or on. `None` leaves it to the
    /// daemon's `[approval] default`, which ships as yolo.
    pub yolo: Option<bool>,
    /// Directory the TUI was launched from: root agents work there until
    /// the operator picks another workspace.
    pub cwd: Option<PathBuf>,
    /// Home directory, for `~` and the picker's project discovery. Injected
    /// so tests never read the real one.
    pub home: Option<PathBuf>,
    /// Workspaces agents were started in, newest first, from `ui.toml`.
    pub recent_workspaces: Vec<String>,
    /// Workspaces the picker lists first, in pin order, from `ui.toml`.
    pub pinned_workspaces: Vec<String>,
    /// Where the picker discovers projects, as written in `ui.toml`; `None` is home.
    pub workspace_roots: Option<Vec<String>>,
    /// Levels below each root discovery searches, from `ui.toml`.
    pub workspace_depth: Option<usize>,
    /// The theme before the palette probe; ascii and motion come from their
    /// own fields.
    pub theme_spec: crate::ui::theme::ThemeSpec,
    /// The named theme `theme_spec` was built from.
    pub theme: crate::ui::theme::ThemeChoice,
    /// `NO_COLOR` chose `theme`, so `/theme` swatches stay colourless too.
    pub no_color_theme: bool,
    /// Shown once in the flash row on start (an override file only partly applied).
    pub startup_flash: Option<String>,
    /// Reported once on start, e.g. a theme file that could not be used.
    pub startup_error: Option<String>,
    /// Where `/theme` and `/motion` save what was chosen. `None` keeps the
    /// choice to this run.
    pub prefs_dir: Option<PathBuf>,
    /// Shared custom themes and view modes, independent of endpoint session state.
    pub definitions_dir: Option<PathBuf>,
    /// Spinner static and scrolling instant (§9.7).
    pub reduced_motion: bool,
    pub ascii: bool,
    /// Saved board state from `ui.toml`; `None` opens it on terminals ≥140 cols.
    pub board_open: Option<bool>,
    /// Slash commands the host program behind the agents runs, from `--host-commands`.
    pub host_commands: crate::commands::HostCommands,
    /// Tab-queued messages restored from `ui.toml`, per agent.
    pub queued: std::collections::BTreeMap<AgentId, Vec<String>>,
}

impl TuiOptions {
    pub fn is_remote(&self) -> bool {
        self.endpoint
            .as_ref()
            .is_some_and(aether_client::Endpoint::is_remote)
    }
    pub(crate) fn command_hint(&self) -> String {
        if let Some(endpoint) = &self.endpoint {
            return endpoint.command_hint(self.explicit_socket);
        }
        let socket = self
            .socket_path
            .clone()
            .unwrap_or_else(aether_protocol::paths::socket_path);
        aether_protocol::command_hint::command_hint(&socket, self.explicit_socket)
    }
}
