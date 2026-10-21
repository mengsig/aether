//! Application reducer.
use super::{
    AgentId, App, AppEvent, Command, Effect, ModelInfo, ModelRef, STATUS_REFRESH_TICKS,
    SubscriptionFrame, Then, now,
};
use crate::app::board::BoardFocus;
use crate::ui::layout::BoardMode;

impl App {
    // ----- reducer ----------------------------------------------------------------

    pub fn reduce(&mut self, event: AppEvent) -> Vec<Effect> {
        let reconcile = !matches!(
            event,
            AppEvent::Tick { .. }
                | AppEvent::Resize(..)
                | AppEvent::Focus(_)
                | AppEvent::Frame(SubscriptionFrame::Delta { .. })
        );
        // Compact search previews also change when the terminal resizes.
        let track_find = !matches!(event, AppEvent::Tick { .. } | AppEvent::Focus(_));
        let selection = self.selected;
        let mut effects = self.reduce_inner(event);
        if track_find {
            self.track_find();
        }
        effects.extend(self.host_effects());
        self.flush_client_errors();
        self.yield_board_to_approval();
        self.track_approvals();
        if selection != self.selected {
            self.voice.selection += 1;
        }
        if reconcile {
            effects.extend(self.voice_reconcile());
        }
        self.track_remote_operations(&mut effects);
        if self.quit_requested {
            self.save_remote_session();
        }
        effects
    }

    /// §1.3: a due approval takes input from the board until it is answered,
    /// so the board's own hints never promise keys the approval would take.
    /// An open view or popup keeps the pane, so the approval waits instead.
    fn yield_board_to_approval(&mut self) {
        let overlay = self.board.open && self.board_mode() == BoardMode::Overlay;
        if !(self.board.focused() || overlay)
            || self.approvals.is_empty()
            || self.bottom_covered()
            || self.due_approval().is_none()
        {
            return;
        }
        if self.board_mode() == BoardMode::Overlay {
            self.board.open = false;
        }
        self.board.focus = BoardFocus::BottomPane;
        self.dirty = true;
    }

    pub(super) fn reduce_inner(&mut self, event: AppEvent) -> Vec<Effect> {
        match event {
            AppEvent::PreviewReady { id, result } => {
                self.preview_ready(id, result);
                Vec::new()
            }
            AppEvent::ViewModesLoaded(catalog) => {
                self.modes_loaded(catalog);
                Vec::new()
            }
            AppEvent::Voice(reply) => self.voice_reply(reply),
            AppEvent::Key(key) => self.dispatch_key(key),

            AppEvent::Paste(text) => self.dispatch_paste(&text),
            AppEvent::TerminalImagePaste => self.terminal_image_paste(),
            AppEvent::ClipboardRead(content) => self.clipboard_read(content),
            AppEvent::ImagePathPasted { text, image } => self.image_path_pasted(&text, image),
            AppEvent::ImageRead(image) => {
                self.image_read(image);
                Vec::new()
            }
            AppEvent::Resize(w, h) => {
                // §1.2: a docked board that would become an overlay closes instead;
                // not saved, since narrowing the terminal is not a choice to hide it.
                let docked = matches!(self.board_mode(), BoardMode::Docked { .. });
                self.size = (w, h);
                if docked && self.board_mode() == BoardMode::Overlay {
                    self.board.open = false;
                    self.board.focus = BoardFocus::BottomPane;
                }
                self.dirty = true;
                if self.viewer.open {
                    self.reload_preview()
                } else {
                    Vec::new()
                }
            }
            AppEvent::Focus(focused) => {
                self.terminal_focused = focused;
                Vec::new()
            }
            AppEvent::Tick { frames } => self.on_tick(frames),
            AppEvent::Connected => self.on_connected(),
            AppEvent::HistoryExpired => {
                self.reduce(AppEvent::Disconnected { because: None });
                self.last_seq = None;
                self.backfill = Default::default();
                for transcript in self.transcripts.values_mut() {
                    *transcript = transcript.continuing();
                }
                self.connected = false;
                self.flash("remote history expired; reconnecting to reload current state");
                Vec::new()
            }
            AppEvent::ExternalFinished { label, error } => {
                self.dirty = true;
                let ok = error.is_none();
                match error {
                    None => self.flash(format!("{label} finished")),
                    Some(reason) => self.flash_error(format!("{label} did not complete: {reason}")),
                }
                let mut effects = vec![
                    Effect::Request {
                        command: Command::ListModels,
                        then: Then::Models,
                    },
                    Effect::Request {
                        command: Command::AuthStatus,
                        then: Then::Auth,
                    },
                    Effect::Request {
                        command: Command::Status,
                        then: Then::Status,
                    },
                ];
                // A login usually means the first agent can start now.
                if ok && self.selected.is_none() {
                    let here = self.active_workspace().map(std::path::Path::to_path_buf);
                    effects.extend(self.await_root_in(here, None));
                }
                effects
            }
            AppEvent::Copied { lines, confirmed } => {
                self.flash(if confirmed {
                    format!("copied {lines} lines")
                } else {
                    format!("sent {lines} lines to terminal clipboard (OSC 52)")
                });
                Vec::new()
            }
            AppEvent::EffectFailed { action, error } => {
                self.flash_error(format!("{action} failed: {error}"));
                Vec::new()
            }
            AppEvent::Disconnected { because } => {
                self.connection += 1;
                let open = self.bottom.view_stack.len();
                self.bottom
                    .view_stack
                    .retain(|view| !view.abandon_on_disconnect());
                let abandoned = self.board.confirmation_pending.take().is_some()
                    || self.bottom.view_stack.len() != open;
                self.backfill_disconnected();
                self.connected = false;
                self.subscription = None;
                for agent in self.live_accounting.keys() {
                    self.context_tokens.remove(agent);
                }
                self.live_accounting.clear();
                // Deltas are not replayed: a stream cut off here would resume
                // with a hole, so the reconnect's events supply the reply instead.
                for transcript in self.transcripts.values_mut() {
                    transcript.stream.reset();
                    if let Some(live) = &mut transcript.live {
                        live.stream.reset();
                    }
                }
                if !self.disconnect_notice_shown {
                    self.disconnect_notice_shown = true;
                    match because {
                        Some(why) => self.flash(crate::ui_format!(
                            self.theme,
                            "daemon connection lost · reconnecting · {why}",
                            "daemon connection lost | reconnecting | {why}"
                        )),
                        None => self.flash(self.theme.decoration(
                            "daemon connection lost · reconnecting",
                            "daemon connection lost | reconnecting",
                        )),
                    }
                }
                if abandoned {
                    self.flash_error(
                        "agent action abandoned: daemon connection lost; no action taken",
                    );
                }
                if !self.board.pending_removals.is_empty() {
                    self.board.pending_removals.clear();
                    self.flash_error("delete interrupted by disconnect; stop was requested, removal did not complete");
                }
                Vec::new()
            }
            AppEvent::Subscribed { id, last_seq } => {
                self.voice.watermark = self.voice.watermark.max(last_seq);
                self.subscription = Some(id);
                if self.last_seq.is_none() {
                    self.last_seq = Some(last_seq);
                }
                if self.remote_snapshot_pending {
                    self.remote_snapshot_pending = false;
                    return self.snapshot_requests();
                }
                Vec::new()
            }
            AppEvent::Frame(frame) => match frame {
                SubscriptionFrame::Event(event) => self.on_event(*event),
                SubscriptionFrame::Lagged { capacity } => {
                    // Not an outage: claim the one notice, then reset like a disconnect.
                    if !self.disconnect_notice_shown {
                        self.disconnect_notice_shown = true;
                        self.flash(crate::ui_format!(
                            self.theme,
                            "reconnecting · subscription exceeded {capacity} queued frames",
                            "reconnecting | subscription exceeded {capacity} queued frames"
                        ));
                    }
                    self.reduce(AppEvent::Disconnected { because: None })
                }
                SubscriptionFrame::Delta {
                    agent,
                    invocation,
                    delta,
                } => {
                    if let aether_protocol::Delta::Accounting {
                        total,
                        context_tokens,
                        ..
                    } = delta
                    {
                        self.project_accounting(agent, total, context_tokens);
                        return Vec::new();
                    }
                    if matches!(delta, aether_protocol::Delta::TurnFinished) {
                        self.live_accounting.remove(&agent);
                    }
                    self.backfill_delta(agent, invocation, &delta);
                    // Only the viewed agent's stream is drawn; another's is no frame.
                    if self.selected == Some(agent) {
                        self.dirty = true;
                    }
                    self.transcripts
                        .entry(agent)
                        .or_default()
                        .stream(invocation, &delta, self.now);
                    Vec::new()
                }
            },
            AppEvent::Reply { then, result } => self.on_reply(then, result),
            AppEvent::Outbox { settled, undecided } => {
                self.outbox_read(settled, undecided);
                Vec::new()
            }
        }
    }

    fn save_remote_session(&mut self) {
        if !self.options.is_remote() {
            return;
        }
        let last_seq = self.last_seq;
        let draft = (!self.input.text.is_empty()).then(|| self.input.text.clone());
        if let Err(error) = self.save_prefs(|prefs| {
            prefs.last_seq = last_seq;
            prefs.draft = draft;
        }) {
            self.flash_error(format!("remote session state not saved: {error}"));
        }
    }

    fn track_remote_operations(&mut self, effects: &mut [Effect]) {
        if !self.options.is_remote() {
            return;
        }
        for effect in effects {
            let Effect::Request { command, then } = effect else {
                continue;
            };
            if matches!(then, Then::RemoteAck { .. }) || !aether_client::key_command(command) {
                continue;
            }
            let Some(Ok(keyed)) = command.keyed_request() else {
                continue;
            };
            let key = keyed.key.to_owned();
            self.remote_pending.insert(key.clone(), then.clone());
            *then = Then::RemoteAck {
                key,
                next: Box::new(then.clone()),
            };
        }
    }

    pub(crate) fn reconnected(&mut self, client: &aether_client::Client) -> Vec<Effect> {
        let instance = client.welcome().map(|welcome| welcome.instance_id.clone());
        let changed = self.options.instance_id.is_some() && self.options.instance_id != instance;
        if changed {
            let mut options = self.options.clone();
            options.instance_id = instance;
            options.focus = None;
            options.queued.clear();
            options.resume_seq = None;
            options.draft = None;
            options.recent_workspaces.clear();
            options.pinned_workspaces.clear();
            options.default_model = None;
            options.default_fallbacks.clear();
            options.recent_models.clear();
            options.yolo = None;
            if let (Some(endpoint), Some(instance)) = (&options.endpoint, &options.instance_id)
                && endpoint.is_remote()
            {
                options.prefs_dir = Some(endpoint.state_dir(instance));
            }
            let (size, link) = (self.size, self.link);
            *self = App::new(options);
            self.size = size;
            // The runner's links keep counting across daemons.
            self.link = link;
        } else {
            self.options.instance_id = instance;
        }
        self.notifications.set_served(client.serves_notifications());
        let mut effects = self.on_connected();
        for (command, reply) in client.recovered_commands() {
            if let Some(Ok(keyed)) = command.keyed_request()
                && let Some(then) = self.remote_pending.remove(keyed.key)
            {
                effects.extend(self.on_reply(
                    then,
                    reply.clone().map_err(aether_client::ClientError::Protocol),
                ));
            } else {
                self.reconcile_recovered_queue(command, reply);
            }
        }
        self.track_remote_operations(&mut effects);
        if self.options.is_remote() {
            effects.push(Effect::Outbox { settle: None });
        }
        if changed {
            self.flash(
                "daemon identity changed; discarded old drafts, queued sends and cursor; reloading",
            );
        }
        effects
    }

    fn reconcile_recovered_queue(
        &mut self,
        command: &Command,
        reply: &Result<aether_protocol::Reply, aether_protocol::ErrorReply>,
    ) {
        let Command::SendMessage {
            to,
            content,
            idempotency_key: Some(key),
            ..
        } = command
        else {
            return;
        };
        if !key.starts_with("tui-queue-") {
            return;
        }
        let matches = self
            .transcripts
            .get(to)
            .and_then(|transcript| transcript.queue.first())
            == Some(content);
        if matches {
            self.queued_sent(
                *to,
                reply
                    .as_ref()
                    .err()
                    .cloned()
                    .map(aether_client::ClientError::Protocol),
            );
        }
    }

    pub(super) fn on_connected(&mut self) -> Vec<Effect> {
        self.approvals_revision = self.approvals_revision.wrapping_add(1);
        self.link += 1;
        self.connected = true;
        self.status = None;
        // The runner drops the old connection's answers to these; nothing waits on them.
        self.status_poll_in_flight = false;
        self.tree_in_flight = false;
        // A fresh connection ends the outage the flags were suppressing
        // repeats for; the next disconnect gets to report itself again.
        self.disconnect_reply_noticed = false;
        self.disconnect_notice_shown = false;
        self.flash(match &self.options.endpoint {
            Some(aether_client::Endpoint::Ssh(remote)) => {
                format!("{} · connected · {}", remote.name(), remote.target())
            }
            _ => "connected".into(),
        });
        self.report_mode_diagnostics();
        let mut effects = vec![Effect::Subscribe {
            after: self.last_seq,
        }];
        self.remote_snapshot_pending = self.options.is_remote();
        if !self.remote_snapshot_pending {
            effects.extend(self.snapshot_requests());
        }
        effects.extend(self.resume_parked_starts());
        debug_assert!(!self.status_poll_in_flight, "the next tick polls status");
        debug_assert_eq!(
            self.tree_in_flight, !self.remote_snapshot_pending,
            "only this connection's snapshot may hold the tree request"
        );
        effects
    }

    fn snapshot_requests(&mut self) -> Vec<Effect> {
        let mut effects = vec![
            Effect::Request {
                command: Command::Status,
                then: Then::Status,
            },
            self.tree_request(),
            self.approvals_request(),
            Effect::Request {
                command: Command::ListModels,
                then: Then::Models,
            },
            Effect::Request {
                command: Command::AuthStatus,
                then: Then::Auth,
            },
        ];
        effects.extend(self.notifications_request());
        if let Some(agent) = self.selected {
            effects.extend(self.backfill_request(agent));
        }
        effects
    }

    /// Catalogue lookup with the registry's case-sensitive, provider-scoped aliases.
    pub(crate) fn model_info(&self, model: &ModelRef) -> Option<&ModelInfo> {
        self.models.iter().find(|info| {
            info.model.provider == model.provider
                && (info.model.model == model.model
                    || info.aliases.iter().any(|alias| alias == &model.model))
        })
    }

    /// Context window of the model an agent runs on, when known.
    pub fn context_window(&self, model: &ModelRef) -> Option<u64> {
        self.model_info(model)
            .map(|info| u64::from(info.capabilities.context_window))
    }

    /// Context size at which the model's agent compacts: the catalogue's
    /// `compaction_tokens`, else three quarters of the window.
    pub fn compaction_threshold(&self, model: &ModelRef) -> Option<u64> {
        let capabilities = &self.model_info(model)?.capabilities;
        let window = u64::from(capabilities.context_window);
        Some(
            capabilities
                .compaction_tokens
                .map_or(window / 4 * 3, u64::from)
                .min(window),
        )
    }

    /// Whether the selected agent has an approval waiting on the person.
    pub fn approval_pending(&self) -> bool {
        self.selected
            .is_some_and(|id| self.approvals.iter().any(|a| a.agent_id == id))
    }

    /// Ring, and tell the desktop, only while the window is unfocused: the
    /// person is elsewhere and something wants them (§9.9). Labelled with
    /// `agent`, not necessarily the selected agent, so a notification about a
    /// background agent never claims to be about whichever one is on screen.
    pub(super) fn attention(&self, effects: &mut Vec<Effect>, agent: AgentId, what: &str) {
        if self.terminal_focused {
            return;
        }
        effects.push(Effect::Bell);
        let who = self.agent_name(agent);
        effects.push(Effect::Notify {
            title: crate::ui_format!(self.theme, "aether · {who}", "aether | {who}"),
            body: what.to_owned(),
        });
    }

    pub(super) fn tree_request(&mut self) -> Effect {
        self.tree_in_flight = true;
        self.tree_stale = false;
        self.tree_requests += 1;
        Effect::Request {
            command: Command::GetTree {
                run: None,
                include_archived: self.show_archived,
            },
            then: Then::Tree,
        }
    }

    /// Ask for a fresh tree, coalescing while one request is outstanding.
    pub(super) fn refresh_tree(&mut self, effects: &mut Vec<Effect>) {
        if self.tree_in_flight {
            self.tree_stale = true;
        } else {
            effects.push(self.tree_request());
        }
    }

    pub(super) fn on_tick(&mut self, frames: u64) -> Vec<Effect> {
        self.tick += frames.max(1);
        self.now = now();
        let mut effects = Vec::new();
        // A frame for what moves on its own, then the next one booked from now.
        if self.animation_due.is_none_or(|at| self.tick >= at) {
            self.animation_due = self
                .animation_in_ms()
                .map(|ms| self.tick + super::view_state::ticks_in(ms));
            self.dirty |= self.animation_due.is_some();
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|flash| flash.until <= self.now)
        {
            self.flash = None;
            self.dirty = true;
        }
        if self.connected && self.tick >= self.status_requested_at + STATUS_REFRESH_TICKS {
            self.status_requested_at = self.tick;
            self.save_remote_session();
            self.refresh_branches();
            if !self.board.pending_removals.is_empty() {
                self.refresh_tree(&mut effects);
            }
            // One poll at a time: a slow daemon gets one request, not one per refresh.
            if !self.status_poll_in_flight {
                self.status_poll_in_flight = true;
                effects.push(Effect::Request {
                    command: Command::Status,
                    then: Then::StatusPoll,
                });
            }
        }
        effects
    }
}
