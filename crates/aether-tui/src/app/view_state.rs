//! Application view state.
use aether_core::agent::AgentStatusKind;

use super::transcript::CellModel;
use super::{
    Agent, AgentId, AgentTree, App, ApprovalRequest, ArtifactsState, BTreeMap, BTreeSet, HashMap,
    InputState, Intent, Motion, ReportView, STATUS_REFRESH_TICKS, ScheduleView, Theme, TuiOptions,
    now,
};
use crate::ui::theme::ThemeSpec;

/// Ticks covering `ms`, rounded up so a deadline is never woken early.
pub(super) fn ticks_in(ms: u64) -> u64 {
    (ms.saturating_mul(super::TICKS_PER_SECOND)).div_ceil(1_000)
}

impl App {
    pub fn new(mut options: TuiOptions) -> Self {
        let modes = &options.view_modes.modes;
        options
            .host_commands
            .refuse_taken(|name| modes.contains_key(name), "shadows a view mode");
        let theme = Theme::resolve(ThemeSpec {
            ascii: options.ascii,
            reduced_motion: options.reduced_motion,
            ..options.theme_spec
        });
        let resume_seq = options.resume_seq;
        let draft = options.draft.clone();
        let motion = Motion::new(options.reduced_motion);
        let recent_models = options.recent_models.clone();
        let recent_workspaces = options.recent_workspaces.clone();
        let pinned_workspaces = options.pinned_workspaces.clone();
        let show_archived = options.show_archived;
        let mouse = options.mouse;
        let startup_flash = options.startup_flash.clone();
        let startup_error = [
            options.startup_error.as_deref(),
            options.host_commands.warning.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let startup_error = (!startup_error.is_empty()).then(|| startup_error.join("; "));
        let theme_choice = options.theme.clone();
        let (keys, key_notices) = crate::keys::KeyBindings::load(&options.keys);
        let mut app = Self {
            view_modes: options.view_modes.clone(),
            view_mode: options
                .view_mode
                .clone()
                .unwrap_or_else(|| "standard".into()),
            presentation_revision: 0,
            modes_reloading: false,
            keys,
            options,
            voice: Default::default(),
            tree: AgentTree::default(),
            selected: None,
            transcripts: HashMap::new(),
            approvals: Vec::new(),
            approvals_revision: 0,
            resolving: BTreeSet::new(),
            approval_rules: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_refreshing: false,
            memories: Vec::new(),
            notifications: Default::default(),
            status: None,
            models: Vec::new(),
            recent_models,
            show_archived,
            providers: Vec::new(),
            schedules: ScheduleView::default(),
            report: ReportView::default(),
            artifacts: ArtifactsState::default(),
            usage_loading: false,
            usage_error: None,
            last_seq: resume_seq,
            subscription: None,
            connected: false,
            host: Default::default(),
            remote_pending: Default::default(),
            undelivered: Vec::new(),
            unreadable: Vec::new(),
            remote_snapshot_pending: false,
            connection: 0,
            link: 0,
            artifact_reads: 0,
            motion,
            size: (0, 0),
            input: InputState::default(),
            tick: 0,
            status_requested_at: 0,
            status_poll_in_flight: false,
            now: now(),
            dirty: true,
            background_dirty: false,
            quit_requested: false,
            tree_in_flight: false,
            tree_stale: false,
            tree_requests: 0,
            unconfirmed: HashMap::new(),
            root_create_attempted: false,
            root_starts: BTreeMap::new(),
            next_start: 0,
            intent: Intent::FirstTree,
            model_patches_in_flight: HashMap::new(),
            queued_model_patches: HashMap::new(),
            held_roots: HashMap::new(),
            workspace: None,
            recent_workspaces,
            pinned_workspaces,
            queue_sends: BTreeSet::new(),
            disconnect_reply_noticed: false,
            disconnect_notice_shown: false,
            forwarded_command_noticed: false,
            auth: Vec::new(),
            default_effort: None,
            terminal_focused: true,
            context_tokens: HashMap::new(),
            live_accounting: HashMap::new(),
            quotas: HashMap::new(),
            branches: HashMap::new(),
            details: false,
            scroll_log: Default::default(),
            board: Default::default(),
            viewer: Default::default(),
            bottom: Default::default(),
            theme,
            theme_choice,
            theme_epoch: 0,
            names_version: 0,
            flash: None,
            terminal_replies: Default::default(),
            pending_client_errors: Vec::new(),
            quota_alert: None,
            mouse,
            exit_message: None,
            animation_due: None,
            backfill: Default::default(),
        };
        app.report_mode_diagnostics();
        app.restore_queues();
        if let Some(draft) = draft {
            app.input.set_text(draft);
        }
        app.refresh_branches();
        app.rebuild_board();
        if let Some(text) = startup_flash {
            app.flash(text);
        }
        if key_notices.is_empty()
            && let Some(text) = &startup_error
        {
            app.flash_error(text);
        }
        if !key_notices.is_empty() {
            let message = [
                startup_error.as_deref().unwrap_or(""),
                &key_notices.join("; "),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
            app.flash_error(message);
        }
        app
    }

    /// The terminal's size at startup: sizes the frame and decides whether
    /// the board starts open (§1.2).
    pub fn start_at(&mut self, cols: u16, rows: u16) {
        self.size = (cols, rows);
        self.board.open = super::board::default_board_open(cols, self.options.board_open);
    }

    // ----- accessors used by the UI -------------------------------------------

    /// Agents running and agents blocked on the operator, across the tree.
    pub fn fleet_counts(&self) -> (usize, usize) {
        let running = self
            .tree
            .nodes
            .values()
            .filter(|node| node.agent.status.kind() == AgentStatusKind::Running)
            .count();
        let needs_you = self
            .needs_you_agents()
            .into_iter()
            .filter(|agent| self.tree.nodes.contains_key(agent))
            .count();
        (running, needs_you)
    }

    /// The terminal title (SPEC Phase 7): `⠙ aether · 2 running · 1 needs you`.
    pub fn terminal_title(&self) -> String {
        let (running, needs_you) = self.fleet_counts();
        let dot = self.theme.glyphs().dot;
        let mut title = String::new();
        if running > 0 {
            let spinner = if self.theme.ascii() {
                "*"
            } else {
                self.motion.spinner(self.tick)
            };
            title.push_str(spinner);
            title.push(' ');
        }
        title.push_str("aether");
        if running > 0 {
            title.push_str(&format!(" {dot} {running} running"));
        }
        if needs_you > 0 {
            title.push_str(&format!(" {dot} {needs_you} needs you"));
        }
        title
    }

    /// How the board shows at the current width.
    pub fn board_mode(&self) -> crate::ui::layout::BoardMode {
        crate::ui::layout::board_mode(self.size.0, self.board.open)
    }

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.selected
            .and_then(|id| self.tree.nodes.get(&id))
            .map(|n| &n.agent)
    }

    pub fn selected_requested_model(&self) -> Option<&aether_core::model::ModelChain> {
        let id = self.selected?;
        if let Some(next) = self.held_root_model(id) {
            return next;
        }
        self.queued_model_patches
            .get(&id)
            .or_else(|| self.model_patches_in_flight.get(&id))
    }

    /// Pending approvals for the selected agent, oldest first.
    pub fn selected_approvals(&self) -> Vec<&ApprovalRequest> {
        let Some(id) = self.selected else {
            return Vec::new();
        };
        self.approvals.iter().filter(|a| a.agent_id == id).collect()
    }

    /// The name behind an id the daemon wrote into free text: an agent's
    /// name, or a task's objective. `None` for anything this client has not
    /// been told about, so the id stays rather than becoming a wrong name.
    pub fn name_of_id(&self, id: &str) -> Option<String> {
        super::name_of_id(&self.tree, id)
    }

    pub fn agent_name(&self, id: AgentId) -> String {
        self.tree
            .nodes
            .get(&id)
            .map(|n| self.agent_display_name(n.agent.id))
            .unwrap_or_else(|| crate::ui::views::short_id(&self.theme, &id))
    }

    /// Frames from `tick` until the loop must deliver the next `Tick`: the
    /// soonest animation step, flash expiry, status refresh or typing-guard
    /// release. `None` when
    /// nothing is pending, so an idle TUI sleeps until an event (§9.7, §9.8).
    pub fn next_tick_in(&self) -> Option<u64> {
        let mut due = Vec::with_capacity(5);
        match self.animation_due {
            Some(at) => due.push(at.saturating_sub(self.tick)),
            None => due.extend(self.animation_in_ms().map(ticks_in)),
        }
        if let Some(flash) = &self.flash {
            let ms = (flash.until - self.now).num_milliseconds();
            due.push(ticks_in(u64::try_from(ms).unwrap_or(0)));
        }
        if self.connected {
            due.push((self.status_requested_at + STATUS_REFRESH_TICKS).saturating_sub(self.tick));
        }
        due.extend(self.approval_guard_ends_in_ms().map(ticks_in));
        // At least one frame ahead: a deadline already passed is the next frame.
        due.into_iter().min().map(|frames| frames.max(1))
    }

    /// Milliseconds until something on screen moves on its own: the status
    /// row and agents strip, the open board, or the viewed agent's live cell.
    pub(crate) fn animation_in_ms(&self) -> Option<u64> {
        use crate::ui::cells::ChatCell as _;
        let cell = self
            .selected
            .and_then(|agent| self.transcripts.get(&agent)?.active.as_ref())
            .filter(|cell| {
                let policy = self.policy(self.selected);
                policy.shows(&cell.model) && !policy.compact(&cell.model)
            })
            .and_then(|cell| cell.model.animation_tick())
            .map(|ms| {
                if self.theme.reduced_motion() {
                    1_000
                } else {
                    ms
                }
            });
        let row = crate::ui::bottom::status_row::next_frame_in(self);
        let board = crate::ui::board::next_frame_in_ms(self, &self.theme);
        [cell, row, board].into_iter().flatten().min()
    }

    /// The most recent complete reply of the selected agent.
    pub(super) fn last_reply(&self) -> Option<String> {
        let transcript = self.transcripts.get(&self.selected?)?.live_state();
        transcript
            .cells
            .iter()
            .rev()
            .find_map(|cell| match &cell.model {
                CellModel::Assistant { text } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
    }
}
