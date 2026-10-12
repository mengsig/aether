//! Application state and the pure reducer.
//!
//! `App::reduce` turns one [`AppEvent`] into state changes plus a list of
//! [`Effect`]s. It never touches the socket or the terminal, so every
//! interaction can be tested by feeding frames and keys.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use std::path::PathBuf;

use aether_client::{ClientError, SubscriptionFrame};

use aether_core::agent::{
    Agent, AgentConfig, AgentStatusKind, ApprovalDefault, ApprovalPolicy, ArchiveTarget, Role,
    Scope,
};

use aether_core::approval::ApprovalRule;

use aether_core::artifact::Artifact;

use aether_core::event::{Event, EventKind};

use aether_core::id::{AgentId, ApprovalId, ArtifactId};

use aether_core::memory::MemoryRecord;

use aether_core::model::{Effort, ModelChain, ModelRef};

use aether_core::time::{Timestamp, now};

use aether_core::tool::{ApprovalRequest, ApprovalStatus};

use aether_protocol::{
    AgentPatch, AgentTree, ArtifactQuery, AuthInfo, Command, CreateAgent, DaemonStatus, EventQuery,
    MemoryQuerySpec, ModelInfo, Reply,
};

use crossterm::event::KeyEvent;

use crate::commands::{self, InputAction, ShowFilter, SlashCommand};

use crate::motion::{self, Motion};

use crate::prefs::UiPrefs;

use crate::ui::theme::Theme;

/// Frame ticks per second; the loop redraws at most this often.
pub const TICKS_PER_SECOND: u64 = motion::FRAME_HZ;

const STATUS_REFRESH_TICKS: u64 = TICKS_PER_SECOND * 2;

/// How often `--plain` wakes to refresh the status; the TUI instead wakes
/// only when [`App::next_tick_in`] asks it to.
pub const IDLE_PERIOD_MS: u64 = 250;

/// How much of an artifact the viewer asks for. Enough that a bounded tool
/// output (30k chars) arrives whole, bounded so a gigabyte file cannot be
/// pulled through the socket to fill one screen.
pub(crate) const ARTIFACT_VIEW_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct App {
    pub view_modes: crate::view_modes::Catalog,
    pub view_mode: String,
    pub presentation_revision: u32,
    modes_reloading: bool,
    pub keys: crate::keys::KeyBindings,
    pub voice: crate::voice::ui::State,
    pub options: TuiOptions,
    pub tree: AgentTree,
    /// The agent whose transcript is on screen.
    pub selected: Option<AgentId>,
    /// Per-agent cell transcripts.
    pub transcripts: transcript::Transcripts,
    pub approvals: Vec<ApprovalRequest>,
    pub(super) approvals_revision: u64,
    /// Answers sent but not yet acknowledged. The card stays until the
    /// daemon confirms: dropping it early would leave the agent waiting on
    /// an approval with nothing left to answer it.
    pub resolving: BTreeSet<ApprovalId>,
    /// Remembered decisions, refreshed when `/approvals` opens.
    pub approval_rules: Vec<ApprovalRule>,
    /// MCP server health, refreshed when `/mcp` opens.
    pub mcp_servers: Vec<aether_core::mcp::McpServerHealth>,
    /// A reconnect is in flight, so the view says so instead of looking idle.
    pub mcp_refreshing: bool,
    /// Loaded memories, refreshed when `/memory` opens and after every
    /// pin/unpin or forget.
    pub memories: Vec<MemoryRecord>,
    pub status: Option<DaemonStatus>,
    pub models: Vec<ModelInfo>,
    /// Models chosen recently, newest first: what lifts them in the picker.
    pub recent_models: Vec<String>,
    /// Show work cleared out of the view. Off is the point of `/clear`;
    /// the board's `.` reveals it again, but does not un-archive it.
    pub show_archived: bool,
    /// Provider rows, refreshed when `/usage` opens.
    pub providers: Vec<aether_protocol::ProviderInfo>,
    pub schedules: ScheduleView,
    pub report: ReportView,
    /// `/artifacts` and the pager: the viewed agent's list and opened bytes.
    pub artifacts: ArtifactsState,
    pub usage_loading: bool,
    pub usage_error: Option<String>,
    pub last_seq: Option<u64>,
    pub subscription: Option<u64>,
    pub connected: bool,
    pub host: host::HostState,
    remote_pending: HashMap<String, Then>,
    /// Remote operations the host could not account for, awaiting `/outbox`.
    pub undelivered: Vec<aether_client::Unresolved>,
    /// Outbox files that are not operations; `/outbox` can only discard them.
    pub unreadable: Vec<aether_client::Unreadable>,
    remote_snapshot_pending: bool,
    /// Bumped on each disconnect; a reply tagged with an older value came from a dead connection.
    pub(crate) connection: u64,
    /// Every animation's timing comes from here.
    pub motion: Motion,
    pub size: (u16, u16),
    pub input: InputState,
    pub tick: u64,
    /// Tick of the last periodic `Status` request. Ticks advance by varying
    /// steps, so the refresh is due by elapsed frames, never by divisibility.
    status_requested_at: u64,
    /// A periodic `Status` is awaiting its answer, or its failure.
    status_poll_in_flight: bool,
    /// The wall clock the renderer paints against, refreshed each tick.
    /// Rendering never reads the clock itself, so one state draws one frame.
    pub now: Timestamp,
    /// Something on screen changed: the loop draws at the next frame.
    pub dirty: bool,
    /// Something changed that the person did not cause and is not watching
    /// closely (another agent, a status poll): drawn at a throttled rate.
    pub background_dirty: bool,
    pub quit_requested: bool,
    tree_in_flight: bool,
    tree_stale: bool,
    /// Trees requested so far; the latest reply answers the latest request.
    tree_requests: u64,
    /// Agents a reply named before any tree showed them, with `tree_requests`
    /// at that moment: only a tree requested later can say they are gone.
    unconfirmed: HashMap<AgentId, u64>,
    /// Whether a root agent has been asked for at all this session: the
    /// automatic first agent is offered once, not on every tree.
    root_create_attempted: bool,
    /// Roots being started, or whose start failed and waits for a retry.
    root_starts: BTreeMap<StartId, RootStart>,
    next_start: u64,
    /// What the operator wants viewed. A tree never overrides it, so a late
    /// reply or stale tree cannot take the view.
    intent: Intent,
    /// One update per agent at a time. A later choice waits so request
    /// scheduling cannot make an older choice win after a newer one.
    model_patches_in_flight: HashMap<AgentId, ModelChain>,
    queued_model_patches: HashMap<AgentId, ModelChain>,
    /// New roots whose messages wait for their model change.
    held_roots: HashMap<AgentId, HeldRoot>,
    /// The workspace last picked or started in; `None` until then, meaning
    /// the launch directory. [`App::active_workspace`] prefers the viewed agent's.
    workspace: Option<PathBuf>,
    /// Workspaces agents were started in, newest first: the picker's recents.
    pub recent_workspaces: Vec<String>,
    /// Workspaces the picker lists first, in pin order, as `ui.toml` spells them.
    pub pinned_workspaces: Vec<String>,
    /// Agents whose oldest queued message is on its way to the daemon; it
    /// leaves the queue only once the daemon has it.
    queue_sends: BTreeSet<AgentId>,
    /// One disconnect can fail many in-flight replies at once; the first one
    /// flashes, and this stays set until reconnected.
    disconnect_reply_noticed: bool,
    /// Same one-per-outage guard for the runner's "connection lost" report,
    /// fired once per failed in-flight reply.
    disconnect_notice_shown: bool,
    /// Provider sign-in status for the login picker.
    pub auth: Vec<AuthInfo>,
    /// Effort chosen before any agent existed; applied to the next root.
    default_effort: Option<Effort>,
    /// Whether the terminal window has focus; bells ring only without it.
    pub terminal_focused: bool,
    /// Tokens the last model call carried per agent: the context in use.
    pub context_tokens: HashMap<AgentId, u64>,
    /// Latest subscription quota reported by an engine, per provider.
    pub quotas: HashMap<String, aether_core::provider::ProviderUsage>,
    /// Branch per workspace path on screen; see `app/git.rs`.
    pub branches: HashMap<PathBuf, Option<String>>,
    /// Ctrl+T details view.
    pub details: bool,
    /// Scroll intents for `ui::transcript` to resolve at draw time.
    pub scroll_log: transcript::ScrollLog,
    pub board: board::BoardState,
    pub bottom: crate::ui::bottom::BottomPane,
    /// Every colour the UI paints comes from here.
    pub theme: Theme,
    /// What `/theme` last chose; `theme` is it resolved for this terminal.
    pub theme_choice: crate::ui::theme::ThemeChoice,
    /// Bumped on every theme change; part of every layout-cache key.
    pub theme_epoch: u32,
    /// `helpers::names_version` of `tree`; part of every layout-cache key.
    pub names_version: u64,
    pub flash: Option<flash::Flash>,
    terminal_replies: crate::keyboard::TerminalReplyFilter,
    pending_client_errors: Vec<(Timestamp, String)>,
    /// Quota-exhaustion chip in the hint row, until acknowledged.
    pub quota_alert: Option<String>,
    /// What the mouse does; capture is on unless `Off`.
    pub mouse: crate::prefs::MouseMode,
    /// Printed once the terminal is restored after quitting.
    pub exit_message: Option<String>,
    /// Tick of the next self-driven frame (pulse, clocks); `None` when nothing moves.
    pub animation_due: Option<u64>,
    /// Events that arrived live while an agent's backfill was in flight,
    /// replayed over it on arrival (`app/backfill.rs`).
    backfill: backfill::Backfill,
}

mod options;
pub use options::TuiOptions;
mod effects;
pub use effects::{AppEvent, Effect, MemoryAction, Then};
pub use flash::Flash;
mod view_data;
pub use view_data::{ArtifactsState, OpenArtifact, ReportView, ScheduleView};
mod input_state;
pub use input_state::{EFFORTS, InputState, PastedText};
mod helpers;
mod images;
pub(crate) use helpers::sibling_names;
pub use helpers::{
    InboxEntry, cli_missing, install_command, name_of_id, split_inbox_text, status_glyph,
    subscription_query, tool_input_hint,
};
use helpers::{decode_artifact, eviction_notice, explain, names_version, then_operation};
pub(crate) use images::{image_label, with_image_lines};
mod agents;
pub mod approval_flow;
pub(crate) mod approvals;
mod artifacts;
mod backfill;
pub mod board;
mod board_actions;
mod command_actions;
mod composer_ops;
mod events;
mod flash;
pub(crate) mod git;
pub mod host;
mod input;
mod intent;
mod memories;
mod outbox;
mod preferences;
mod reducer;
mod replies;
mod root_starts;
mod schedules;
pub mod transcript;
mod view_modes;
mod view_state;
mod views;
mod workspaces;
use intent::Intent;
pub use root_starts::StartId;
use root_starts::{HeldRoot, RootStart};
pub use workspaces::LiveWorkspace;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod host_tests;
