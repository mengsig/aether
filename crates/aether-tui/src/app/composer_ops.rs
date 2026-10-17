//! Composer behaviour behind the keys (SPEC §3.2, §4.1): kill ring, paste
//! placeholder, steer vs queue per agent, ⇧← edit, popups, Ctrl+R, find. Owner: W4.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use aether_core::agent::AgentStatusKind;
use aether_core::id::AgentId;

use super::transcript::{ScrollIntent, Steer};
use super::{App, ClientError, Command, Effect, InputState, Then, image_label, with_image_lines};
use crate::commands::InputAction;
use crate::ui::bottom::composer::{
    Killed, PASTE_PLACEHOLDER_CHARS, PastedText, paste_placeholder, paste_spans,
};
use crate::ui::bottom::find::{self, FindBar};
use crate::ui::bottom::popup::{self, MentionTab, Popup, PopupItem, PopupKind};

/// Which composer edit a key asked for; W6 maps keys onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    KillToEnd,
    KillToStart,
    KillWordBack,
    Yank,
    WordLeft,
    WordRight,
    Home,
    End,
    /// Deletes a whole paste placeholder when the cursor ends one.
    Backspace,
    /// Deletes a whole paste placeholder when the cursor starts one.
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupKeyOutcome {
    Pass,
    Consumed,
    SubmitCommand,
}

/// Kills kept for Ctrl+Y; the oldest drop first.
pub const KILL_RING_MAX: usize = 32;
/// The `@` Files tab reads at most this many workspace paths.
pub const MENTION_MAX_FILES: usize = 5_000;
const MENTION_MAX_DEPTH: usize = 8;
const MENTION_SKIP_DIRS: &[&str] = &["target", "node_modules"];

impl App {
    pub(crate) fn composer_edit(&mut self, op: EditOp) {
        let input = &self.input;
        let cursor = input.cursor;
        let range = match op {
            EditOp::KillToEnd => {
                let end = input.line_end();
                // At a line's end, Ctrl+K joins the next line (Emacs).
                let end = if end == cursor && end < input.text.len() {
                    end + 1
                } else {
                    end
                };
                cursor..end
            }
            EditOp::KillToStart => input.line_start()..cursor,
            EditOp::KillWordBack => input.word_start_before()..cursor,
            EditOp::Yank => {
                if let Some(killed) = self.bottom.composer.kill_ring.last().cloned() {
                    self.yank(killed);
                }
                return;
            }
            EditOp::WordLeft => return self.composer_move(InputState::word_left),
            EditOp::WordRight => return self.composer_move(InputState::word_right),
            EditOp::Home => return self.composer_move(InputState::home),
            EditOp::End => return self.composer_move(InputState::end),
            EditOp::Backspace | EditOp::Delete => return self.delete_char(op),
        };
        let killed = self.cut_draft(range);
        if killed.text.is_empty() {
            return;
        }
        let ring = &mut self.bottom.composer.kill_ring;
        ring.push(killed);
        if ring.len() > KILL_RING_MAX {
            ring.remove(0);
        }
    }

    /// Move the cursor with `motion`; a placeholder it lands inside is stepped
    /// over in the direction it moved, so nothing is ever typed into one.
    pub(crate) fn composer_move(&mut self, motion: impl FnOnce(&mut InputState)) {
        let from = self.input.cursor;
        motion(&mut self.input);
        let to = self.input.cursor;
        if let Some(span) = self.paste_around(to) {
            self.input.cursor = if to < from { span.start } else { span.end };
        }
    }

    /// The paste placeholder `at` falls strictly inside.
    fn paste_around(&self, at: usize) -> Option<Range<usize>> {
        paste_spans(&self.input.text, &self.input.pastes)
            .into_iter()
            .map(|(span, _)| span)
            .find(|span| span.start < at && at < span.end)
    }

    /// Cut `range`, widened to whole placeholders, keeping the pastes it took.
    fn cut_draft(&mut self, range: Range<usize>) -> Killed {
        let start = self
            .paste_around(range.start)
            .map_or(range.start, |s| s.start);
        let end = self.paste_around(range.end).map_or(range.end, |s| s.end);
        let pastes = self
            .input
            .pastes
            .iter()
            .filter(|paste| start <= paste.range.start && paste.range.end <= end)
            .cloned()
            .map(|mut paste| {
                paste.range = paste.range.start - start..paste.range.end - start;
                paste
            })
            .collect();
        let text = self.input.cut(start..end);
        Killed { text, pastes }
    }

    /// Ctrl+Y: re-insert a kill, its pastes under placeholders free in this draft.
    fn yank(&mut self, killed: Killed) {
        let mut text = String::with_capacity(killed.text.len());
        let mut pastes = Vec::new();
        let cursor = self.input.cursor;
        let mut taken = self.input.text.clone();
        let mut start = 0;
        for (span, _) in paste_spans(&killed.text, &killed.pastes) {
            taken.push_str(&killed.text[start..span.start]);
            start = span.end;
        }
        taken.push_str(&killed.text[start..]);
        let mut rest = 0;
        for (span, paste) in paste_spans(&killed.text, &killed.pastes) {
            let placeholder = paste_placeholder(
                paste.text.chars().count(),
                &self.input.pastes,
                &format!("{taken}{text}"),
            );
            text.push_str(&killed.text[rest..span.start]);
            let start = cursor + text.len();
            text.push_str(&placeholder);
            pastes.push(PastedText {
                range: start..cursor + text.len(),
                placeholder,
                text: paste.text.clone(),
            });
            rest = span.end;
        }
        text.push_str(&killed.text[rest..]);
        self.input.insert_str(&text);
        self.input.pastes.extend(pastes);
    }

    /// Backspace/Delete, treating a paste placeholder as one char.
    fn delete_char(&mut self, op: EditOp) {
        let cursor = self.input.cursor;
        let hit = paste_spans(&self.input.text, &self.input.pastes)
            .into_iter()
            .map(|(span, _)| span)
            .find(|span| match op {
                EditOp::Backspace => span.start < cursor && cursor <= span.end,
                _ => span.start <= cursor && cursor < span.end,
            });
        let Some(span) = hit else {
            return match op {
                EditOp::Backspace => self.input.backspace(),
                _ => self.input.delete(),
            };
        };
        self.input.cut(span);
    }

    /// A paste whose placeholder is no longer whole in the draft: sending would lose its text.
    fn refuse_damaged_paste(&mut self) -> bool {
        let draft = &self.input.text;
        let Some(paste) = self
            .input
            .pastes
            .iter()
            .find(|paste| draft.get(paste.range.clone()) != Some(paste.placeholder.as_str()))
        else {
            return false;
        };
        self.flash(format!(
            "not sent: {} was damaged and its pasted text would be lost",
            paste.placeholder
        ));
        true
    }

    /// Bracketed paste into the draft; large pastes become a placeholder.
    pub(crate) fn composer_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.insert_paste(text);
    }

    fn insert_paste(&mut self, text: String) {
        let chars = text.chars().count();
        if chars <= PASTE_PLACEHOLDER_CHARS {
            self.input.insert_str(&text);
            return;
        }
        let placeholder = paste_placeholder(chars, &self.input.pastes, &self.input.text);
        let start = self.input.cursor;
        let end = start + placeholder.len();
        self.input.insert_str(&placeholder);
        self.input.pastes.push(PastedText {
            placeholder,
            text,
            range: start..end,
        });
    }

    /// Enter: send to the viewed agent; steers while it runs (preview until consumed).
    pub(crate) fn composer_submit(&mut self) -> Vec<Effect> {
        if self.refuse_damaged_paste() || self.refuse_unread_image() {
            return Vec::new();
        }
        let text = self.expanded_draft();
        self.reset_composer_extras();
        // History keeps the expanded text, so ↑ recalls what was really sent.
        self.input.set_text(text);
        self.bottom.approval.deny_target = None;
        let text = match self.parse_input(&self.input.text) {
            InputAction::Message(text) => text,
            InputAction::Empty => String::new(),
            InputAction::Command(command) => {
                // Taken (into history) the way `submit_input` takes a message.
                self.input.take();
                self.dirty = true;
                return self.run_slash(command);
            }
        };
        // The preview reads as the transcript will: the text, then a line per image.
        let preview = with_image_lines(&text, self.input.images.iter().map(image_label));
        let steer = self.viewed_running().filter(|_| !preview.is_empty());
        let effects = self.submit_input();
        if let Some(agent) = steer {
            let sent_at = self.now;
            let transcript = self.transcripts.entry(agent).or_default();
            let text = preview;
            transcript.steering.push(Steer { text, sent_at });
        }
        effects
    }

    /// Tab: queue for after the turn while running; send when idle.
    pub(crate) fn composer_queue(&mut self) -> Vec<Effect> {
        let Some(agent) = self.viewed_running() else {
            return self.composer_submit();
        };
        let text = match self.parse_input(&self.input.text) {
            // A command is never "after this turn": run it now.
            InputAction::Command(_) => return self.composer_submit(),
            InputAction::Empty | InputAction::Message(_)
                if self.refuse_queued_images() || self.refuse_unread_image() =>
            {
                return Vec::new();
            }
            InputAction::Empty => return Vec::new(),
            InputAction::Message(_) if self.refuse_damaged_paste() => return Vec::new(),
            InputAction::Message(_) => self.expanded_draft(),
        };
        self.reset_composer_extras();
        self.input.set_text(text);
        let text = self.input.take().trim().to_owned();
        self.transcripts.entry(agent).or_default().queue.push(text);
        self.save_queue(agent);
        Vec::new()
    }

    /// ⇧←: pull the last queued message back into the draft.
    pub(crate) fn composer_unqueue(&mut self) {
        let Some(agent) = self.selected else {
            return;
        };
        let sending = self.queue_sends.contains(&agent);
        let Some(transcript) = self.transcripts.get_mut(&agent) else {
            return;
        };
        // The head is on its way to the daemon; only what waits behind it can come back.
        if sending && transcript.queue.len() == 1 {
            self.flash("that queued message is already being sent");
            return;
        }
        let Some(text) = transcript.queue.pop() else {
            return;
        };
        self.save_queue(agent);
        // The current draft is kept below the pulled-back message.
        let cursor = text.len();
        let prefix = if self.input.text.is_empty() {
            text
        } else {
            format!("{text}\n")
        };
        self.input.cursor = 0;
        self.input.insert_str(&prefix);
        self.input.cursor = cursor;
    }

    /// `agent`'s turn ended (event `seq`, now `next`): send its oldest queued
    /// message, which starts the next turn. A finished agent keeps its queue.
    pub(crate) fn composer_turn_ended(
        &mut self,
        agent: AgentId,
        next: AgentStatusKind,
        seq: u64,
    ) -> Vec<Effect> {
        let Some(text) = self
            .transcripts
            .get(&agent)
            .and_then(|t| t.queue.first().cloned())
        else {
            return Vec::new();
        };
        if self.queue_sends.contains(&agent) {
            return Vec::new();
        }
        // The daemon refuses messages to a finished agent (as `submit_input` knows).
        if next.is_terminal() || self.is_terminal(agent) {
            let count = self.transcripts.get(&agent).map_or(0, |t| t.queue.len());
            let noun = if count == 1 { "message" } else { "messages" };
            let name = self.agent_display_name(agent);
            let unqueue = self
                .keys
                .display(crate::keys::KeyAction::Unqueue, self.theme.ascii());
            self.flash(format!(
                "{name} has finished: {count} queued {noun} kept, not sent ({unqueue} pulls them back)"
            ));
            return Vec::new();
        }
        self.queue_sends.insert(agent);
        self.dirty = true;
        vec![Effect::Request {
            command: Command::SendMessage {
                to: agent,
                content: text,
                subtree: false,
                attachments: Vec::new(),
                // Every client sees the same turn end: the daemon delivers it once.
                idempotency_key: Some(format!("tui-queue-{agent}-{seq}")),
            },
            then: Then::QueuedSent(agent),
        }]
    }

    /// The daemon answered a queued send: drop the entry it took, or keep it.
    pub(crate) fn queued_sent(&mut self, agent: AgentId, error: Option<ClientError>) {
        self.queue_sends.remove(&agent);
        self.dirty = true;
        if let Some(error) = error {
            let name = self.agent_display_name(agent);
            self.flash_error(format!(
                "queued message for {name}: {error}. It stays queued; delivery may be unconfirmed"
            ));
            return;
        }
        if let Some(transcript) = self.transcripts.get_mut(&agent)
            && !transcript.queue.is_empty()
        {
            transcript.queue.remove(0);
        }
        self.save_queue(agent);
    }

    /// Save `agent`'s queue to `ui.toml`. Other agents' entries stay as on
    /// disk: another client may have written them since this one loaded.
    fn save_queue(&mut self, agent: AgentId) {
        let queue = self
            .transcripts
            .get(&agent)
            .map(|t| t.queue.clone())
            .unwrap_or_default();
        let key = agent.to_string();
        let saved = self.save_prefs(|prefs| {
            if queue.is_empty() {
                prefs.queued.remove(&key);
            } else {
                prefs.queued.insert(key, queue);
            }
        });
        if let Err(reason) = saved {
            self.flash_error(format!("queued messages not saved: {reason}"));
        }
    }

    /// Queued messages saved in `ui.toml` go back to their agents. Called from `App::new`.
    pub(crate) fn restore_queues(&mut self) {
        for (agent, queue) in &self.options.queued {
            self.transcripts.entry(*agent).or_default().queue = queue.clone();
        }
    }

    /// The viewed agent, when it is mid-turn.
    fn viewed_running(&self) -> Option<AgentId> {
        let agent = self.selected?;
        let node = self.tree.nodes.get(&agent)?;
        (node.agent.status.kind() == AgentStatusKind::Running).then_some(agent)
    }

    fn reset_composer_extras(&mut self) {
        self.input.pastes.clear();
        self.bottom.popup = None;
        self.bottom.shortcuts = false;
    }

    /// Expand mentions in typed text and substitute tracked pastes once.
    fn expanded_draft(&self) -> String {
        // A `/name` line forwarded to the agent is another program's syntax: keep it verbatim.
        let literal = self.expand_draft(str::to_owned);
        if matches!(self.parse_input(&literal), InputAction::Message(text) if text.starts_with('/'))
        {
            return literal;
        }
        self.expand_draft(|text| expand_mentions(text, |name| self.unique_agent(name)))
    }

    fn expand_draft(&self, literal: impl Fn(&str) -> String) -> String {
        let mut text = String::new();
        let mut rest = 0;
        for (span, paste) in paste_spans(&self.input.text, &self.input.pastes) {
            text.push_str(&literal(&self.input.text[rest..span.start]));
            text.push_str(&paste.text);
            rest = span.end;
        }
        text.push_str(&literal(&self.input.text[rest..]));
        text
    }

    /// Ctrl+C on a draft: clear it into history with pastes expanded, so ↑
    /// recalls the text itself rather than a dead `[Pasted N chars]`.
    pub(crate) fn clear_draft_to_history(&mut self) {
        let text = self.expand_draft(str::to_owned);
        self.reset_composer_extras();
        self.input.set_text(text);
        self.input.take();
    }

    /// A paste while the find bar is open extends its one-line query.
    pub(crate) fn find_paste(&mut self, text: &str) {
        let Some(bar) = &mut self.bottom.find else {
            return;
        };
        bar.query.push_str(&one_line_query(text));
        self.refresh_find();
    }

    /// A paste while Ctrl+R is open extends its query, as typing does.
    pub(crate) fn history_paste(&mut self, text: &str) {
        self.edit_history_query(|query| query.push_str(&one_line_query(text)));
    }

    /// The find bar or Ctrl+R search has the keys; the draft under it is not being edited.
    pub(crate) fn search_focused(&self) -> bool {
        self.bottom.find.is_some()
            || self
                .bottom
                .popup
                .as_ref()
                .is_some_and(|popup| popup.kind == PopupKind::History)
    }

    /// The one agent called `name`; `None` when no agent or several share it.
    fn unique_agent(&self, name: &str) -> Option<AgentId> {
        let mut named = self
            .tree
            .nodes
            .values()
            .filter(|node| node.agent.name == name);
        let first = named.next()?;
        named.next().is_none().then_some(first.agent.id)
    }

    /// Open `/`, `@` (files/agents), or Ctrl+R history popups. `/` and `@`
    /// insert their trigger char unless it is already in place.
    pub(crate) fn open_popup(&mut self, kind: PopupKind) {
        self.bottom.shortcuts = false;
        self.bottom.find = None;
        let mut popup = Popup::new(kind);
        match kind {
            PopupKind::Slash => {
                if self.input.text.is_empty() {
                    self.input.insert_char('/');
                }
                if !self.input.text.starts_with('/') {
                    return;
                }
            }
            PopupKind::Mention(_) => {
                if mention_start(&self.input.text, self.input.cursor).is_none() {
                    self.input.insert_char('@');
                }
                popup.files = self.mention_files();
            }
            PopupKind::History => {
                popup.saved_draft = Some(self.input.text.clone());
                popup.saved_pastes = self.input.pastes.clone();
            }
        }
        self.bottom.popup = Some(popup);
        self.sync_popup();
    }

    /// Close the popup; a history search puts the draft back as it was.
    pub(crate) fn close_popup(&mut self) {
        if let Some(popup) = self.bottom.popup.take()
            && let Some(saved) = popup.saved_draft
        {
            self.input.set_text(saved);
            self.input.pastes = popup.saved_pastes;
        }
    }

    pub(crate) fn toggle_shortcuts(&mut self) {
        self.bottom.shortcuts = !self.bottom.shortcuts;
        if self.bottom.shortcuts {
            self.bottom.shortcuts_scroll = 0;
            self.close_popup();
            self.bottom.find = None;
        }
    }

    /// ↑/↓/PgUp/PgDn scroll an open `?` panel taller than its room; `true` when used.
    pub(crate) fn shortcuts_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.is_empty() {
            return false;
        }
        let composer = crate::ui::composer_area(self);
        let (page, max) = crate::ui::bottom::shortcuts::scroll_range(
            composer,
            &self.theme,
            &self.keys,
            self.help_modes(),
            &self.options.host_commands,
        );
        let scroll = self.bottom.shortcuts_scroll;
        let next = match key.code {
            KeyCode::Up => scroll.saturating_sub(1),
            KeyCode::Down => scroll + 1,
            KeyCode::PageUp => scroll.saturating_sub(page),
            KeyCode::PageDown => scroll + page,
            _ => return false,
        };
        self.bottom.shortcuts_scroll = next.min(max);
        true
    }

    pub(crate) fn shortcuts_wheel(&mut self, up: bool) {
        let code = if up { KeyCode::Up } else { KeyCode::Down };
        self.dirty |= self.shortcuts_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub(crate) fn open_find(&mut self) {
        self.close_popup();
        self.bottom.shortcuts = false;
        self.bottom.find = Some(FindBar::default());
    }

    /// Slash-row activation submits independently of the configured Send key.
    pub(crate) fn popup_key(&mut self, key: KeyEvent) -> PopupKeyOutcome {
        let consumed = if self.bottom.find.is_some() {
            self.find_key(key)
        } else {
            match self.bottom.popup.as_ref().map(|p| p.kind) {
                Some(PopupKind::History) => self.history_key(key),
                Some(kind @ (PopupKind::Slash | PopupKind::Mention(_))) => {
                    return self.list_key(key, kind);
                }
                None => false,
            }
        };
        if consumed {
            PopupKeyOutcome::Consumed
        } else {
            PopupKeyOutcome::Pass
        }
    }

    pub(crate) fn popup_wheel(&mut self, up: bool) {
        if self.bottom.popup.is_none() || self.bottom.find.is_some() {
            return;
        }
        self.popup_key(KeyEvent::new(
            if up { KeyCode::Up } else { KeyCode::Down },
            KeyModifiers::NONE,
        ));
        self.dirty = true;
    }

    fn list_key(&mut self, key: KeyEvent, kind: PopupKind) -> PopupKeyOutcome {
        let plain = key.modifiers.is_empty();
        match key.code {
            KeyCode::Esc if plain => self.bottom.popup = None,
            KeyCode::Up if plain => self.move_popup_cursor(-1),
            KeyCode::Down if plain => self.move_popup_cursor(1),
            KeyCode::Left | KeyCode::Right if plain && matches!(kind, PopupKind::Mention(_)) => {
                self.switch_mention_tab()
            }
            KeyCode::Tab if plain => return self.accept_popup_item(true),
            KeyCode::Enter if plain => return self.accept_popup_item(false),
            KeyCode::Backspace if plain => {
                self.delete_char(EditOp::Backspace);
                self.sync_popup();
            }
            KeyCode::Char(c) if plain || key.modifiers == KeyModifiers::SHIFT => {
                self.input.insert_char(c);
                self.sync_popup();
            }
            _ => return PopupKeyOutcome::Pass,
        }
        PopupKeyOutcome::Consumed
    }

    fn move_popup_cursor(&mut self, delta: isize) {
        let Some(popup) = &self.bottom.popup else {
            return;
        };
        let len = popup::items(popup, self).len();
        if let Some(popup) = &mut self.bottom.popup {
            popup.cursor = popup
                .cursor
                .saturating_add_signed(delta)
                .min(len.saturating_sub(1));
        }
    }

    fn switch_mention_tab(&mut self) {
        if let Some(popup) = &mut self.bottom.popup {
            popup.kind = match popup.kind {
                PopupKind::Mention(MentionTab::Files) => PopupKind::Mention(MentionTab::Agents),
                _ => PopupKind::Mention(MentionTab::Files),
            };
            popup.cursor = 0;
        }
    }

    /// Insert the selected item. `complete` (Tab) never runs a command; Enter
    /// runs one unless it needs an argument.
    fn accept_popup_item(&mut self, complete: bool) -> PopupKeyOutcome {
        let Some(popup) = &self.bottom.popup else {
            return PopupKeyOutcome::Pass;
        };
        let item = popup::items(popup, self).into_iter().nth(popup.cursor);
        self.bottom.popup = None;
        match item {
            Some(PopupItem::Mode { name, .. }) => {
                self.input.set_text(format!("/{name}"));
                if complete {
                    PopupKeyOutcome::Consumed
                } else {
                    PopupKeyOutcome::SubmitCommand
                }
            }
            Some(PopupItem::Command { name, usage, .. }) => {
                self.accept_command(name, usage, complete)
            }
            // Submitted like any message; `parse_input` sends it to the agent verbatim.
            Some(PopupItem::HostCommand { name, usage, .. }) => {
                self.accept_command(&name, &usage, complete)
            }
            Some(PopupItem::File { path }) => {
                self.replace_mention(&path);
                PopupKeyOutcome::Consumed
            }
            Some(PopupItem::Agent { id, name, .. }) => {
                let mention = if self.unique_agent(&name) == Some(id) {
                    format!("@{name}")
                } else {
                    format!("@{name} ({id})")
                };
                self.replace_mention(&mention);
                PopupKeyOutcome::Consumed
            }
            Some(PopupItem::History { .. }) => PopupKeyOutcome::Consumed,
            None if complete => PopupKeyOutcome::Consumed,
            None => PopupKeyOutcome::Pass,
        }
    }

    /// Put `/name` in the draft: run it on Enter unless it needs an argument.
    fn accept_command(&mut self, name: &str, usage: &str, complete: bool) -> PopupKeyOutcome {
        let needs_arg = usage.starts_with('<');
        if complete || needs_arg {
            self.input.set_text(format!("/{name} "));
            return PopupKeyOutcome::Consumed;
        }
        self.input.set_text(format!("/{name}"));
        PopupKeyOutcome::SubmitCommand
    }

    /// Replace the `@query` token before the cursor with `text` and a space.
    fn replace_mention(&mut self, text: &str) {
        let Some(start) = mention_start(&self.input.text, self.input.cursor) else {
            return;
        };
        self.input.cut(start..self.input.cursor);
        self.input.insert_str(&format!("{text} "));
    }

    /// Re-derive the popup's query from the draft; close it when its token is gone.
    pub(super) fn sync_popup(&mut self) {
        let Some(popup) = &self.bottom.popup else {
            return;
        };
        let query = match popup.kind {
            PopupKind::History => return self.preview_history(),
            PopupKind::Slash => slash_query(&self.input.text),
            PopupKind::Mention(_) => mention_start(&self.input.text, self.input.cursor)
                .map(|start| &self.input.text[start + 1..self.input.cursor]),
        };
        let Some(query) = query.map(str::to_owned) else {
            self.bottom.popup = None;
            return;
        };
        if let Some(popup) = &mut self.bottom.popup
            && popup.query != query
        {
            popup.query = query;
            popup.cursor = 0;
        }
    }

    fn history_key(&mut self, key: KeyEvent) -> bool {
        let older = self.keys.matches(crate::keys::KeyAction::History, key);
        let clear_query = self.keys.matches(crate::keys::KeyAction::EraseStart, key);
        let plain = key.modifiers.is_empty();
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let Some(popup) = &mut self.bottom.popup else {
            return false;
        };
        match key.code {
            _ if older => popup.cursor += 1,
            KeyCode::Esc if plain => {
                self.close_popup();
                return true;
            }
            KeyCode::Char('c') if ctrl => {
                self.close_popup();
                return true;
            }
            KeyCode::Enter if plain => {
                // Accept: the previewed entry stays as the draft.
                self.bottom.popup = None;
                return true;
            }
            KeyCode::Up if plain => popup.cursor += 1,
            KeyCode::Char('s') if ctrl => popup.cursor = popup.cursor.saturating_sub(1),
            KeyCode::Down if plain => popup.cursor = popup.cursor.saturating_sub(1),
            KeyCode::Backspace if plain => {
                popup.query.pop();
                popup.cursor = 0;
            }
            _ if clear_query => {
                popup.query.clear();
                popup.cursor = 0;
            }
            KeyCode::Char(c) if plain || key.modifiers == KeyModifiers::SHIFT => {
                popup.query.push(c);
                popup.cursor = 0;
            }
            _ => return false,
        }
        self.preview_history();
        true
    }

    /// Change the Ctrl+R query as a typed key would.
    fn edit_history_query(&mut self, edit: impl FnOnce(&mut String)) {
        let Some(popup) = self
            .bottom
            .popup
            .as_mut()
            .filter(|popup| popup.kind == PopupKind::History)
        else {
            return;
        };
        edit(&mut popup.query);
        popup.cursor = 0;
        self.preview_history();
    }

    /// Show the selected match in the draft, or the saved draft when nothing matches.
    fn preview_history(&mut self) {
        let Some(popup) = &self.bottom.popup else {
            return;
        };
        let matches = popup::history_matches(self, &popup.query);
        let cursor = popup.cursor.min(matches.len().saturating_sub(1));
        let text = matches.into_iter().nth(cursor);
        let restoring = text.is_none();
        let pastes = popup.saved_pastes.clone();
        let text = text
            .or_else(|| popup.saved_draft.clone())
            .unwrap_or_default();
        if let Some(popup) = &mut self.bottom.popup {
            popup.cursor = cursor;
        }
        self.input.set_text(text);
        if restoring {
            self.input.pastes = pastes;
        }
    }

    fn find_key(&mut self, key: KeyEvent) -> bool {
        let clear_query = self.keys.matches(crate::keys::KeyAction::EraseStart, key);
        let plain = key.modifiers.is_empty();
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let Some(bar) = &mut self.bottom.find else {
            return false;
        };
        match key.code {
            KeyCode::Esc if plain => self.bottom.find = None,
            KeyCode::Char('c') if ctrl => self.bottom.find = None,
            KeyCode::Enter | KeyCode::Down if plain => self.step_find(1),
            KeyCode::Up if plain => self.step_find(-1),
            KeyCode::Backspace if plain => {
                bar.query.pop();
                self.refresh_find();
            }
            _ if clear_query => {
                bar.query.clear();
                self.refresh_find();
            }
            KeyCode::Char(c) if plain || key.modifiers == KeyModifiers::SHIFT => {
                bar.query.push(c);
                self.refresh_find();
            }
            _ => return false,
        }
        true
    }

    fn refresh_find(&mut self) {
        self.track_find();
        self.reveal_find_match();
    }

    /// Keep an open find bar's matches in step with the viewed transcript.
    /// No reveal: the reader may be scrolled elsewhere.
    pub(crate) fn track_find(&mut self) {
        let Some(mut bar) = self.bottom.find.take() else {
            return;
        };
        find::refresh(&mut bar, self);
        self.bottom.find = Some(bar);
    }

    /// Move to the next (`1`) or previous (`-1`) match, wrapping.
    fn step_find(&mut self, delta: isize) {
        let Some(bar) = &mut self.bottom.find else {
            return;
        };
        if !find::step(bar, delta) {
            return;
        }
        self.reveal_find_match();
    }

    /// Scroll to the current match, turning details on when only they show it.
    fn reveal_find_match(&mut self) {
        let Some(agent) = self.selected else {
            return;
        };
        let Some(bar) = &self.bottom.find else {
            return;
        };
        let Some(cell) = bar.current.and_then(|i| bar.matches.get(i).copied()) else {
            return;
        };
        if !self.details && find::current_needs_details(bar) {
            self.details = true;
            let dot = self.theme.glyphs().dot;
            self.flash(format!(
                "details on to show the match {dot} {} hides them",
                self.keys
                    .display(crate::keys::KeyAction::Details, self.theme.ascii())
            ));
        }
        self.scroll_log.push(agent, ScrollIntent::Reveal(cell));
    }

    /// Workspace files for `@`, relative to the viewed agent's workspace.
    /// Reads the disk once per popup; a failure is flashed, not hidden.
    fn mention_files(&mut self) -> Vec<String> {
        if self.options.is_remote() {
            return self
                .host
                .files
                .as_ref()
                .filter(|(root, _, _)| Some(root.as_path()) == self.active_workspace())
                .map(|(_, _, files)| files.clone())
                .unwrap_or_default();
        }
        let root = self
            .selected
            .and_then(|id| self.tree.nodes.get(&id))
            .and_then(|node| node.agent.scope.workspace_root.clone())
            .map(Ok)
            .unwrap_or_else(std::env::current_dir);
        match root.and_then(|root| workspace_files(&root, MENTION_MAX_FILES)) {
            Ok(files) => files,
            Err(err) => {
                self.flash_error(format!("can't list workspace files: {err}"));
                Vec::new()
            }
        }
    }
}

/// Pasted text as one line of a search query.
fn one_line_query(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// The `/command` name being typed: the draft is `/` plus a name, nothing after.
fn slash_query(text: &str) -> Option<&str> {
    let name = text.strip_prefix('/')?;
    (!name.contains(char::is_whitespace)).then_some(name)
}

/// Byte offset of the `@` starting the mention the cursor is in: the `@` begins
/// the draft or follows whitespace, and no whitespace lies between it and the cursor.
fn mention_start(text: &str, cursor: usize) -> Option<usize> {
    let before = &text[..cursor];
    let start = before.rfind(|c: char| c == '@' || c.is_whitespace())?;
    if !before[start..].starts_with('@') {
        return None;
    }
    let at_boundary = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    at_boundary.then_some(start)
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// `@name` → `@name (agt_…)` for every name `resolve` knows. A mention never
/// changes who receives the message; it only tells the model whom it means.
fn expand_mentions(text: &str, resolve: impl Fn(&str) -> Option<AgentId>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        let boundary = rest[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace)
            && (at > 0 || out.chars().next_back().is_none_or(char::is_whitespace));
        let after = &rest[at + 1..];
        let len = after.find(|c| !is_name_char(c)).unwrap_or(after.len());
        let name = &after[..len];
        out.push_str(&rest[..at + 1 + len]);
        rest = &after[len..];
        if !boundary || name.is_empty() || rest.starts_with(" (agt_") {
            continue;
        }
        if let Some(id) = resolve(name) {
            out.push_str(&format!(" ({id})"));
        }
    }
    out.push_str(rest);
    out
}

/// Files under `root`, relative and sorted, skipping hidden entries and build
/// output. Unreadable subdirectories are skipped; an unreadable root is an error.
fn workspace_files(root: &Path, limit: usize) -> std::io::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut first = true;
    while let Some((dir, depth)) = dirs.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if first => return Err(err),
            Err(_) => continue,
        };
        first = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < MENTION_MAX_DEPTH && !MENTION_SKIP_DIRS.contains(&name.as_ref()) {
                    dirs.push((path, depth + 1));
                }
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().into_owned());
            }
            if files.len() >= limit {
                files.sort();
                return Ok(files);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests;
