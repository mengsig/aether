//! Approvals as a bottom-pane flow (SPEC §3.4): FIFO across agents, typing
//! guard, defer on Ctrl+C, answers mapped onto the existing commands. Owner: W5.

use std::collections::BTreeSet;

use aether_core::approval::RuleScope;
use aether_core::id::{AgentId, ApprovalId};
use aether_core::time::Timestamp;
use aether_core::tool::{ApprovalDecision, ApprovalRequest};
use aether_protocol::{Command, RememberSpec};
use chrono::Duration;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::transcript::{CellModel, ScrollIntent};
use super::{App, Effect, Then};
use crate::ui::bottom::ViewAction;
use crate::ui::bottom::approval::{OPTION_COUNT, TurnPause};
use crate::ui::layout::BoardMode;

/// An approval arriving within this long of a keystroke waits for idle.
pub const TYPING_GUARD_MS: i64 = 1_000;
/// Answer keys count only once an approval has been on screen this long.
pub const MIN_VISIBLE_MS: i64 = 300;

/// The four answers of the approval view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    /// `y`: approve once.
    Once,
    /// `a`: save an allow rule for this agent and its subagents.
    Subtree,
    /// `p`: save an allow rule everywhere.
    Persist,
    /// `esc`/`n`: deny; the composer then targets the requesting agent.
    Deny,
}

impl ApprovalAnswer {
    /// Option rows in display order; `1`–`4` and Enter index into this.
    pub const ALL: [ApprovalAnswer; OPTION_COUNT] = [
        ApprovalAnswer::Once,
        ApprovalAnswer::Subtree,
        ApprovalAnswer::Persist,
        ApprovalAnswer::Deny,
    ];
}

/// One row of the needs-you strip (`◆ name asks: … · ctrl+g to open`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsYou {
    pub agent: AgentId,
    pub name: String,
    pub what: NeedsYouKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeedsYouKind {
    Question {
        first_line: String,
    },
    /// Deferred (Ctrl+C), opened (`o`), held behind an open view or popup,
    /// or held by the typing guard.
    Approval {
        id: ApprovalId,
        summary: String,
    },
}

impl App {
    /// The approval at the head of the queue: oldest undeferred, past the
    /// typing guard. A view or popup may still cover it; see [`App::shown_approval`].
    pub fn due_approval(&self) -> Option<&ApprovalRequest> {
        let view = &self.bottom.approval;
        self.approvals
            .iter()
            .filter(|a| !self.resolving.contains(&a.id) && !view.deferred.contains(&a.id))
            .min_by_key(|a| a.requested_at)
            // Anything younger arrived later still, so it is held too.
            .filter(|a| view.guard_allows(view.arrived_at(a.id, self.now), self.now))
    }

    /// The approval on screen, which takes keys: due, and nothing covers its slot.
    /// Rendering and key dispatch both read this, so an unseen card never answers.
    pub fn shown_approval(&self) -> Option<&ApprovalRequest> {
        let overlay = self.board.open && self.board_mode() == BoardMode::Overlay;
        if overlay || self.bottom_covered() {
            return None;
        }
        self.due_approval()
            .filter(|request| self.approval_fits(request))
    }

    fn approval_fits(&self, request: &ApprovalRequest) -> bool {
        let mode = self.board_mode();
        let width = crate::ui::bottom_width(self.size.0, mode);
        let height = crate::ui::bottom::approval::desired_height(request, self, width);
        let layout = crate::ui::layout::frame_layout(
            ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1),
            mode,
            crate::ui::layout::BottomRows {
                composer: height,
                hint: 1,
                ..Default::default()
            },
        );
        width > 0 && layout.composer.height >= height
    }

    pub(crate) fn approval_too_short(&self) -> bool {
        self.due_approval()
            .is_some_and(|request| !self.approval_fits(request))
    }

    /// A view, popup, find bar or `?` panel owns the bottom pane; an arriving
    /// approval waits in the needs-you strip until it closes.
    pub(crate) fn bottom_covered(&self) -> bool {
        let bottom = &self.bottom;
        !bottom.view_stack.is_empty()
            || bottom.popup.is_some()
            || bottom.find.is_some()
            || bottom.shortcuts
    }

    /// Bookkeeping after every event: when each approval was first held, when
    /// the slot came free, and when the shown card was first drawn.
    pub(crate) fn track_approvals(&mut self) {
        let now = self.now;
        let covered = self.bottom_covered();
        let view = &mut self.bottom.approval;
        if view.covered && !covered {
            view.uncovered_at = Some(now);
        }
        view.covered = covered;
        let pending: BTreeSet<ApprovalId> = self.approvals.iter().map(|a| a.id).collect();
        view.deferred.retain(|id| pending.contains(id));
        view.first_seen.retain(|id, _| pending.contains(id));
        for id in &pending {
            view.first_seen.entry(*id).or_insert(now);
        }
        if view.expanded.is_some_and(|id| !pending.contains(&id)) {
            view.expanded = None;
        }
        let shown = self.shown_approval().map(|a| a.id);
        if shown != self.bottom.approval.shown.map(|(id, _)| id) {
            self.bottom.approval.shown = shown.map(|id| (id, now));
            self.dirty = true;
        }
    }

    /// Milliseconds until the typing guard lets a held approval show; the
    /// frame loop wakes then, so the card is drawn before a key can answer it.
    pub(crate) fn approval_guard_ends_in_ms(&self) -> Option<u64> {
        let view = &self.bottom.approval;
        let last = view.last_key_at?;
        let waiting = self
            .approvals
            .iter()
            .any(|a| !self.resolving.contains(&a.id) && !view.deferred.contains(&a.id));
        if !waiting || self.due_approval().is_some() {
            return None;
        }
        let ms = (last + Duration::milliseconds(TYPING_GUARD_MS) - self.now).num_milliseconds();
        Some(u64::try_from(ms).unwrap_or(0))
    }

    /// Keys while an approval is shown (W6 calls this first in `dispatch_key`).
    /// `None` passes the key on: none is shown, or it is a global chord.
    pub(crate) fn approval_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let request = self.shown_approval()?;
        let (id, agent) = (request.id, request.agent_id);
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.defer_approval(id);
            return Some(Vec::new());
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return None;
        }
        let answer = match key.code {
            KeyCode::Char('y') => ApprovalAnswer::Once,
            KeyCode::Char('a') => ApprovalAnswer::Subtree,
            KeyCode::Char('p') => ApprovalAnswer::Persist,
            KeyCode::Char('n') | KeyCode::Esc => ApprovalAnswer::Deny,
            KeyCode::Char(digit @ '1'..='4') => {
                ApprovalAnswer::ALL[usize::from(digit as u8 - b'1')]
            }
            KeyCode::Enter => ApprovalAnswer::ALL[self.bottom.approval.cursor_for(id)],
            KeyCode::Up => {
                self.move_approval_cursor(id, -1);
                return Some(Vec::new());
            }
            KeyCode::Down => {
                self.move_approval_cursor(id, 1);
                return Some(Vec::new());
            }
            KeyCode::Char('o') => return Some(self.open_requester(id, agent)),
            KeyCode::Char('e') => {
                self.toggle_approval_body(id);
                return Some(Vec::new());
            }
            // The view is modal: stray typing must not reach the composer.
            _ => return Some(Vec::new()),
        };
        Some(self.answer_shown_approval(id, answer))
    }

    /// Answer the card on screen, unless it appeared too recently to have been read.
    pub(crate) fn answer_shown_approval(
        &mut self,
        id: ApprovalId,
        answer: ApprovalAnswer,
    ) -> Vec<Effect> {
        if !self.bottom.approval.answerable(id, self.now) {
            return Vec::new();
        }
        self.answer_approval(id, answer)
    }

    pub(crate) fn answer_approval(
        &mut self,
        id: ApprovalId,
        answer: ApprovalAnswer,
    ) -> Vec<Effect> {
        if self.resolving.contains(&id) {
            return Vec::new();
        }
        let Some(request) = self.approvals.iter().find(|a| a.id == id).cloned() else {
            self.flash("that approval is no longer pending");
            return Vec::new();
        };
        let (decision, remember) = match answer {
            ApprovalAnswer::Once => (ApprovalDecision::Approved, None),
            ApprovalAnswer::Subtree => (
                ApprovalDecision::Approved,
                Some(narrow_rule(RuleScope::Subtree {
                    root: request.agent_id,
                })),
            ),
            ApprovalAnswer::Persist => (
                ApprovalDecision::Approved,
                Some(narrow_rule(RuleScope::Everywhere)),
            ),
            ApprovalAnswer::Deny => (ApprovalDecision::Denied, None),
        };
        self.record_approval_pause(&request);
        self.resolving.insert(id);
        self.bottom.approval.deferred.remove(&id);
        self.dirty = true;
        let mut effects = vec![Effect::Request {
            command: Command::ResolveApproval {
                id,
                decision,
                note: None,
                remember,
                idempotency_key: None,
            },
            then: Then::ApprovalResolved(id),
        }];
        if answer == ApprovalAnswer::Deny {
            self.bottom.approval.deny_target = Some(request.agent_id);
            let before = self.selected;
            effects.extend(self.view_requester(request.agent_id));
            if self.selected != before {
                // The view moved to the requester; say so rather than switch silently.
                let name = self.agent_display_name(request.agent_id);
                self.flash(crate::ui_format!(
                    self.theme,
                    "viewing {name} · tell it what to do instead",
                    "viewing {name} | tell it what to do instead"
                ));
            }
        }
        effects
    }

    /// Ctrl+C on the view: close without deciding; it moves to the needs-you strip.
    pub(crate) fn defer_approval(&mut self, id: ApprovalId) {
        let pending: BTreeSet<ApprovalId> = self.approvals.iter().map(|a| a.id).collect();
        let deferred = &mut self.bottom.approval.deferred;
        deferred.retain(|d| pending.contains(d));
        if pending.contains(&id) {
            deferred.insert(id);
        }
        self.dirty = true;
    }

    /// Bring `agent`'s deferred approvals back into the queue. For the board's
    /// Enter on a needs-you agent (the strip says `ctrl+g to open`).
    pub(crate) fn reopen_approvals(&mut self, agent: AgentId) {
        let ids: BTreeSet<ApprovalId> = self
            .approvals
            .iter()
            .filter(|a| a.agent_id == agent)
            .map(|a| a.id)
            .collect();
        self.bottom.approval.deferred.retain(|d| !ids.contains(d));
        self.dirty = true;
    }

    /// Needs-you rows for the strip, oldest first (the strip shows at most 2).
    /// One row per agent: its oldest open question or deferred approval.
    pub fn needs_you(&self) -> Vec<NeedsYou> {
        let mut rows: Vec<(Timestamp, NeedsYou)> = Vec::new();
        for transcript in self.transcripts.values() {
            let transcript = transcript.live_state();
            for cell in transcript.cells.iter().chain(transcript.active.iter()) {
                if let CellModel::Question {
                    asker,
                    asker_name,
                    text,
                    state: crate::app::transcript::QuestionState::Pending,
                    ..
                } = &cell.model
                {
                    let first_line = text.lines().next().unwrap_or_default().trim().to_owned();
                    rows.push((
                        cell.at,
                        NeedsYou {
                            agent: *asker,
                            name: asker_name.clone(),
                            what: NeedsYouKind::Question { first_line },
                        },
                    ));
                }
            }
        }
        // Held: the typing guard keeps every undeferred approval back.
        let held = self.due_approval().is_none();
        let covered = self.bottom_covered() || self.approval_too_short();
        for request in self.approvals.iter().filter(|a| {
            (held || covered || self.bottom.approval.deferred.contains(&a.id))
                && !self.resolving.contains(&a.id)
        }) {
            rows.push((
                request.requested_at,
                NeedsYou {
                    agent: request.agent_id,
                    name: self.agent_display_name(request.agent_id),
                    what: NeedsYouKind::Approval {
                        id: request.id,
                        summary: approval_summary(request),
                    },
                },
            ));
        }
        rows.sort_by(|(a_at, a), (b_at, b)| a_at.cmp(b_at).then(a.agent.cmp(&b.agent)));
        let mut seen = BTreeSet::new();
        rows.into_iter()
            .filter(|(_, row)| seen.insert(row.agent))
            .map(|(_, row)| row)
            .collect()
    }

    /// Every agent blocked on the operator: a pending approval (shown,
    /// queued or deferred) or an open question. The board/strip `◆` marker.
    pub fn needs_you_agents(&self) -> BTreeSet<AgentId> {
        let mut agents: BTreeSet<AgentId> = self
            .approvals
            .iter()
            .filter(|a| !self.resolving.contains(&a.id))
            .map(|a| a.agent_id)
            .collect();
        agents.extend(self.needs_you().into_iter().map(|row| row.agent));
        agents
    }

    /// Name for the operator; never an id (SPEC §2.2).
    pub(crate) fn agent_display_name(&self, agent: AgentId) -> String {
        super::helpers::agent_display_name(&self.tree, agent)
            .unwrap_or_else(|| "an agent".to_owned())
    }

    /// `e`: show the whole command or target, or fold it back to three rows.
    fn toggle_approval_body(&mut self, id: ApprovalId) {
        let view = &mut self.bottom.approval;
        view.expanded = if view.expanded == Some(id) {
            None
        } else {
            Some(id)
        };
        self.dirty = true;
    }

    fn move_approval_cursor(&mut self, id: ApprovalId, delta: isize) {
        let view = &mut self.bottom.approval;
        let next = view.cursor_for(id).saturating_add_signed(delta);
        view.cursor = next.min(OPTION_COUNT - 1);
        view.cursor_on = Some(id);
        self.dirty = true;
    }

    /// `o`: read the requester's transcript; the approval waits in the needs-you strip.
    fn open_requester(&mut self, id: ApprovalId, agent: AgentId) -> Vec<Effect> {
        if self.selected == Some(agent) {
            return Vec::new();
        }
        self.defer_approval(id);
        self.view_requester(agent)
    }

    fn view_requester(&mut self, agent: AgentId) -> Vec<Effect> {
        let effects = self.apply_view_actions(vec![ViewAction::ViewAgent(agent)]);
        self.scroll_log.push(agent, ScrollIntent::Latest);
        effects
    }

    /// Bank an answered approval's wait so the status-row clock stays paused over it.
    fn record_approval_pause(&mut self, request: &ApprovalRequest) {
        let Some(turn_started) = turn_started_at(self, request.agent_id) else {
            return;
        };
        let waited = (self.now - request.requested_at.max(turn_started))
            .num_milliseconds()
            .max(0) as u64;
        let entry = self
            .bottom
            .approval
            .paused
            .entry(request.agent_id)
            .or_insert(TurnPause {
                turn_started,
                paused_ms: 0,
            });
        if entry.turn_started != turn_started {
            *entry = TurnPause {
                turn_started,
                paused_ms: 0,
            };
        }
        entry.paused_ms += waited;
    }
}

/// The narrowest rule the daemon can write from this call (its exact
/// argument, or its first resource), placed at `scope`.
fn narrow_rule(scope: RuleScope) -> RememberSpec {
    RememberSpec {
        scope: Some(scope),
        tool_wide: false,
        note: None,
    }
}

/// One line saying what an approval would allow.
pub(crate) fn approval_summary(request: &ApprovalRequest) -> String {
    request
        .argument
        .as_deref()
        .unwrap_or(&request.description)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// When `agent`'s current turn began, as its separator will count it; before
/// `agent.started` is seen, the first cell after its last separator, else
/// the start of its current status.
pub(crate) fn turn_started_at(app: &App, agent: AgentId) -> Option<Timestamp> {
    let from_cells = app.transcripts.get(&agent).and_then(|transcript| {
        let transcript = transcript.live_state();
        if let Some(started) = transcript.session_started() {
            return Some(started);
        }
        let cells = &transcript.cells;
        let after_separator = cells
            .iter()
            .rposition(|c| matches!(c.model, CellModel::Separator { .. }))
            .map_or(0, |i| i + 1);
        cells[after_separator..]
            .iter()
            .chain(transcript.active.iter())
            .filter(|c| !transcript.named_only.contains(&c.id))
            .find(|c| !matches!(c.model, CellModel::Header { .. }))
            .map(|c| c.at)
    });
    from_cells.or_else(|| app.tree.nodes.get(&agent)?.agent.status_since)
}

/// Time `agent` has spent waiting on approvals this turn: banked answers plus
/// any approval still open. The status row subtracts it from the turn clock.
pub(crate) fn approval_pause_ms(app: &App, agent: AgentId, turn_started: Timestamp) -> u64 {
    let banked = app
        .bottom
        .approval
        .paused
        .get(&agent)
        .filter(|p| p.turn_started == turn_started)
        .map_or(0, |p| p.paused_ms);
    let open = app
        .approvals
        .iter()
        .filter(|a| a.agent_id == agent && !app.resolving.contains(&a.id))
        .map(|a| a.requested_at.max(turn_started))
        .min()
        .map_or(0, |since| {
            (app.now - since).num_milliseconds().max(0) as u64
        });
    banked + open
}

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod tests;
