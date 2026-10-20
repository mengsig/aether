//! Mouse input uses the geometry of the last painted frame.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::board::BoardFocus;
use crate::app::transcript::ScrollIntent;
use crate::app::{App, Effect};
use crate::prefs::MouseMode;
use crate::ui::selection::{self, Pane, Selection};
use crate::ui::transcript::ViewCache;

const WHEEL_LINES: u16 = 3;

impl App {
    pub fn dispatch_mouse(&mut self, mouse: MouseEvent, cache: &mut ViewCache) -> Vec<Effect> {
        if self.mouse == MouseMode::Off || mouse.modifiers.contains(KeyModifiers::SHIFT) {
            return Vec::new();
        }
        if self.viewer.open && cache.viewer_area.contains((mouse.column, mouse.row).into()) {
            self.viewer_mouse(mouse);
            cache.pending_file_hit = None;
            cache.mouse.clear_selection();
            return Vec::new();
        }
        selection::validate(self, cache);
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let pane = cache
                    .mouse
                    .selection
                    .as_ref()
                    .filter(|s| s.dragging)
                    .map(|s| s.pane)
                    .or_else(|| cache.regions.hit(mouse.column, mouse.row));
                let up = mouse.kind == MouseEventKind::ScrollUp;
                let effects = self.mouse_scroll(pane, up, cache);
                if let Some(s) = cache.mouse.selection.as_mut().filter(|s| s.dragging) {
                    s.moved = true;
                    cache.mouse.extend_after_scroll = true;
                }
                return effects;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                cache.pending_file_hit = if !self.bottom_covered()
                    && cache.regions.hit(mouse.column, mouse.row) == Some(Pane::Transcript)
                {
                    cache
                        .file_hits
                        .iter()
                        .filter(|h| h.area.contains((mouse.column, mouse.row).into()))
                        .max_by_key(|h| h.reference.target.len())
                        .cloned()
                        .map(|h| (self.selected, h))
                } else {
                    None
                };
                cache.mouse.clear_selection();
                self.dirty = true;
                let Some(pane) = cache.regions.hit(mouse.column, mouse.row) else {
                    return Vec::new();
                };
                if pane == Pane::Board && cache.board.rendered_revision != self.board.revision {
                    self.flash("agents board changed; click the row again");
                    return Vec::new();
                }
                if matches!(pane, Pane::Transcript | Pane::Composer) {
                    self.leave_board();
                    self.bottom.view_stack.clear();
                    self.bottom.popup = None;
                    self.bottom.shortcuts = false;
                    self.bottom.find = None;
                }
                if pane == Pane::View {
                    return Vec::new();
                }
                if pane == Pane::Board
                    && cache
                        .board
                        .tree_area
                        .contains((mouse.column, mouse.row).into())
                {
                    cache.mouse.pending_board_click = crate::ui::board::tree::agent_at(
                        &self.board.model,
                        cache.board.tree_offset,
                        usize::from(mouse.row - cache.board.tree_area.y),
                    );
                }
                let Some(point) = selection::point(self, cache, pane, mouse.column, mouse.row)
                else {
                    return Vec::new();
                };
                if pane == Pane::Transcript {
                    self.viewer.source_cell = Some((self.selected, point.cell));
                }
                let excerpt = if pane == Pane::Transcript {
                    selection::pinned_excerpt(self, cache, point, mouse.row)
                } else {
                    None
                };
                cache.mouse.selection = Some(Selection {
                    pane,
                    rect: cache.regions.rect(pane),
                    agent: self.selected,
                    start: point,
                    end: point,
                    start_excerpt: excerpt,
                    end_excerpt: None,
                    dragging: true,
                    moved: false,
                    pointer: mouse,
                    rows: Vec::new(),
                    epoch: self
                        .selected
                        .and_then(|id| self.transcripts.get(&id))
                        .map(|t| t.epoch)
                        .unwrap_or_default(),
                    input_revision: self.input.revision,
                    board_revision: self.board.revision,
                    details: self.details,
                });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                cache.pending_file_hit = None;
                self.extend_mouse(mouse, cache);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if cache.mouse.selection.as_ref().is_some_and(|s| s.dragging) {
                    self.extend_mouse(mouse, cache);
                }
                let moved = cache.mouse.selection.as_ref().is_some_and(|s| s.moved);
                if let Some(s) = cache.mouse.selection.as_mut() {
                    s.dragging = false;
                }
                self.dirty = true;
                if moved {
                    cache.pending_file_hit = None;
                    cache.mouse.pending_board_click = None;
                    return match selection::extract(self, cache) {
                        Ok(text) if !text.is_empty() => vec![Effect::Copy(text)],
                        Ok(_) => Vec::new(),
                        Err(error) => {
                            self.flash_error(error);
                            Vec::new()
                        }
                    };
                }
                cache.mouse.selection = None;
                if let Some((agent, hit)) = cache.pending_file_hit.take()
                    && agent == self.selected
                    && hit.area.contains((mouse.column, mouse.row).into())
                    && cache.file_hits.iter().any(|h| {
                        h.cell == hit.cell
                            && h.reference.target == hit.reference.target
                            && h.area == hit.area
                    })
                {
                    return self.open_reference(hit.reference);
                }
                if let Some(agent) = cache.mouse.pending_board_click.take() {
                    // Like Ctrl+G: an open view or popup keeps the keys and the selection.
                    if self.bottom_covered() {
                        self.flash_close_first("click the agent");
                        return Vec::new();
                    }
                    self.board.cursor = Some(agent);
                    self.board.focus = BoardFocus::Board;
                    return self.select(agent);
                }
            }
            MouseEventKind::Down(_) => {
                self.dirty |= cache.mouse.selection.take().is_some();
                cache.mouse.pending_board_click = None;
                cache.pending_file_hit = None;
            }
            _ => {}
        }
        Vec::new()
    }

    fn extend_mouse(&mut self, mouse: MouseEvent, cache: &mut ViewCache) {
        let Some(s) = cache.mouse.selection.as_ref().filter(|s| s.dragging) else {
            return;
        };
        let pane = s.pane;
        let rect = s.rect;
        let start = s.start;
        if let Some(end) = selection::point(self, cache, pane, mouse.column, mouse.row) {
            let excerpt = if pane == Pane::Transcript {
                selection::pinned_excerpt(self, cache, end, mouse.row)
            } else {
                None
            };
            if let Some(s) = cache.mouse.selection.as_mut() {
                s.end = end;
                s.end_excerpt = excerpt;
                s.moved |=
                    end != start || mouse.column != s.pointer.column || mouse.row != s.pointer.row;
                s.pointer = mouse;
            }
        }
        if mouse.kind == MouseEventKind::Drag(MouseButton::Left)
            && pane == Pane::Transcript
            && (mouse.row <= rect.y || mouse.row >= rect.bottom())
        {
            let effects = self.mouse_scroll(Some(pane), mouse.row <= rect.y, cache);
            debug_assert!(effects.is_empty());
            cache.mouse.extend_after_scroll = true;
        }
        self.dirty = true;
    }

    fn mouse_scroll(&mut self, pane: Option<Pane>, up: bool, cache: &mut ViewCache) -> Vec<Effect> {
        match pane {
            Some(Pane::Transcript) => {
                if let Some(agent) = self.selected {
                    self.scroll_log.push(
                        agent,
                        if up {
                            ScrollIntent::LineUp(WHEEL_LINES)
                        } else {
                            ScrollIntent::LineDown(WHEEL_LINES)
                        },
                    );
                    self.dirty = true;
                }
            }
            Some(Pane::Board) => {
                cache.board.wheel_delta =
                    cache
                        .board
                        .wheel_delta
                        .saturating_add(if up { -3 } else { 3 });
                self.dirty = true;
            }
            Some(Pane::View) => {
                let mut effects = Vec::new();
                for _ in 0..WHEEL_LINES {
                    let Some(mut view) = self.bottom.view_stack.pop() else {
                        if self.bottom.popup.is_none() && self.bottom.shortcuts {
                            self.shortcuts_wheel(up);
                        } else {
                            self.popup_wheel(up);
                        }
                        continue;
                    };
                    let outcome = view.wheel(up, self);
                    self.dirty = true;
                    match outcome {
                        crate::ui::bottom::ViewOutcome::Ignored
                        | crate::ui::bottom::ViewOutcome::Consumed => self.bottom.push_view(view),
                        crate::ui::bottom::ViewOutcome::Act(actions) => {
                            self.bottom.push_view(view);
                            effects.extend(self.apply_view_actions(actions));
                        }
                        crate::ui::bottom::ViewOutcome::Close(actions) => {
                            effects.extend(self.apply_view_actions(actions));
                            break;
                        }
                    }
                }
                return effects;
            }
            _ => {}
        }
        Vec::new()
    }
}
