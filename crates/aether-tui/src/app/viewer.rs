use super::{App, Effect};
use crate::{
    keys::KeyAction,
    ui::cells::ChatCell,
    viewer::{Preview, ReadRequest, references::Reference},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(crate) fn viewer_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        match mouse.kind {
            MouseEventKind::ScrollUp => self.viewer_scroll(-3),
            MouseEventKind::ScrollDown => self.viewer_scroll(3),
            MouseEventKind::Down(_) => self.viewer.focused = true,
            _ => return,
        }
        self.dirty = true;
    }

    pub(crate) fn file_picker(&mut self) {
        if self.options.is_remote() {
            self.flash("Remote agent files are not available locally; no files fetched");
            return;
        }
        let Some(transcript) = self.selected.and_then(|id| self.transcripts.get(&id)) else {
            return;
        };
        let cell = self
            .viewer
            .source_cell
            .filter(|(agent, _)| *agent == self.selected)
            .and_then(|(_, id)| {
                transcript
                    .cells
                    .iter()
                    .chain(transcript.active.iter())
                    .find(|c| c.id == id)
            })
            .or_else(|| {
                transcript
                    .active
                    .as_ref()
                    .filter(|c| c.model.copy_source().is_some())
            })
            .or_else(|| {
                transcript
                    .cells
                    .iter()
                    .rev()
                    .find(|c| c.model.copy_source().is_some())
            });
        let refs = cell
            .and_then(|c| c.model.copy_source())
            .map(crate::viewer::references::parse)
            .unwrap_or_default();
        if refs.is_empty() {
            self.flash("No local file references in this message");
            return;
        }
        self.viewer.picker = refs;
        self.viewer.request = self.viewer.request.wrapping_add(1);
        self.viewer.pending = None;
        self.viewer.preview = None;
        self.viewer.cursor = 0;
        self.viewer.scroll = 0;
        self.viewer.open = true;
        self.viewer.focused = true;
        self.board.open = false;
        self.leave_board();
    }

    pub(crate) fn open_reference(&mut self, reference: Reference) -> Vec<Effect> {
        if self.options.is_remote() {
            self.flash("Remote files need a local copy; no files fetched");
            return Vec::new();
        }
        let workspace = self
            .selected
            .and_then(|id| self.tree.nodes.get(&id))
            .and_then(|n| n.agent.scope.workspace_root.as_deref())
            .or_else(|| self.active_workspace());
        let Some(workspace) = workspace.map(ToOwned::to_owned) else {
            self.flash_error("Cannot preview a file without the agent workspace");
            return Vec::new();
        };
        let request = ReadRequest {
            id: 0,
            target: reference.target,
            workspace,
            approved_outside: None,
            width: 1,
            height: 1,
            protocol: self.viewer.protocol,
            cell_pixels: self.viewer.cell_pixels,
            theme: self.theme,
        };
        self.viewer.focused = true;
        self.request_preview(request)
    }

    fn request_preview(&mut self, mut request: ReadRequest) -> Vec<Effect> {
        self.viewer.open = true;
        self.viewer.picker.clear();
        self.viewer.scroll = 0;
        self.viewer.request = self.viewer.request.wrapping_add(1);
        self.board.open = false;
        self.leave_board();
        let (_, area) = crate::viewer::split(
            ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1),
            &self.viewer,
        );
        request.id = self.viewer.request;
        request.width = area.width.saturating_sub(2).max(1);
        request.height = area.height.saturating_sub(3).max(1);
        request.theme = self.theme;
        debug_assert!(request.width > 0 && request.height > 0);
        debug_assert_eq!(request.id, self.viewer.request);
        self.viewer.pending = Some(request.clone());
        self.viewer.preview = None;
        vec![Effect::PreviewFile(request)]
    }

    pub(crate) fn viewer_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if self.bottom_covered() || self.board.focused() {
            return None;
        }
        if self.keys.matches(KeyAction::Files, key) {
            self.file_picker();
            return Some(Vec::new());
        }
        if !crate::viewer::visible(self) {
            return None;
        }
        if self.keys.matches(KeyAction::ViewerExternal, key) {
            return Some(
                self.viewer
                    .external_path()
                    .map(|p| vec![Effect::OpenFile(p.clone())])
                    .unwrap_or_default(),
            );
        }
        if self.keys.matches(KeyAction::ViewerClose, key)
            || (self.viewer.focused && key.code == KeyCode::Esc && key.modifiers.is_empty())
            || (self.viewer.fullscreen
                && key.code == KeyCode::Char('c')
                && key.modifiers == KeyModifiers::CONTROL)
        {
            self.viewer.open = false;
            self.viewer.pending = None;
            self.viewer.preview = None;
            return Some(Vec::new());
        }
        if self.keys.matches(KeyAction::ViewerFull, key) {
            self.viewer.fullscreen = !self.viewer.fullscreen;
            if self.viewer.fullscreen {
                self.viewer.focused = true;
            }
            return Some(self.reload_preview());
        }
        if self.keys.matches(KeyAction::ViewerLess, key)
            || self.keys.matches(KeyAction::ViewerMore, key)
        {
            let more = self.keys.matches(KeyAction::ViewerMore, key);
            return Some(self.resize_viewer(more));
        }
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            self.viewer.focused = !self.viewer.focused;
            if !self.viewer.focused && self.viewer.fullscreen {
                self.viewer.fullscreen = false;
                return Some(self.reload_preview());
            }
            return Some(Vec::new());
        }
        self.viewer_content_key(key)
    }

    fn viewer_content_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if !self.viewer.focused {
            return None;
        }
        if !key.modifiers.is_empty() {
            let editing = key.modifiers == KeyModifiers::SHIFT
                || [KeyAction::Send, KeyAction::Queue, KeyAction::Newline]
                    .into_iter()
                    .any(|action| self.keys.matches(action, key));
            return (self.viewer.fullscreen && editing).then(Vec::new);
        }
        self.viewer_navigation(key)
            .or_else(|| self.viewer.fullscreen.then(Vec::new))
    }

    fn resize_viewer(&mut self, more: bool) -> Vec<Effect> {
        self.viewer.share = if more {
            self.viewer.share.saturating_add(5).min(80)
        } else {
            self.viewer.share.saturating_sub(5).max(20)
        };
        debug_assert!((20..=80).contains(&self.viewer.share));
        self.reload_preview()
    }

    fn viewer_navigation(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let effects = match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                let delta = match key.code {
                    KeyCode::Up => -1,
                    KeyCode::Down => 1,
                    KeyCode::PageUp => -10,
                    _ => 10,
                };
                self.viewer_scroll(delta);
                Vec::new()
            }
            KeyCode::Enter if !self.viewer.picker.is_empty() => {
                debug_assert!(self.viewer.cursor < self.viewer.picker.len());
                let reference = self.viewer.picker[self.viewer.cursor].clone();
                self.open_reference(reference)
            }
            KeyCode::Enter if matches!(self.viewer.preview, Some(Ok(Preview::Outside(_)))) => {
                if let (Some(request), Some(Ok(Preview::Outside(path)))) =
                    (self.viewer.pending.as_mut(), &self.viewer.preview)
                {
                    request.approved_outside = Some(path.clone());
                }
                self.reload_preview()
            }
            _ => return None,
        };
        Some(effects)
    }

    fn viewer_scroll(&mut self, delta: i32) {
        let (_, area) = crate::viewer::split(
            ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1),
            &self.viewer,
        );
        let height = usize::from(area.height.saturating_sub(3).max(1));
        let rows = self.viewer.content_rows();
        let max = rows.saturating_sub(height).min(usize::from(u16::MAX));
        if self.viewer.picker.is_empty() {
            self.viewer.scroll =
                (i32::from(self.viewer.scroll) + delta).clamp(0, max as i32) as u16;
            return;
        }
        self.viewer.cursor =
            (self.viewer.cursor as i32 + delta).clamp(0, rows.saturating_sub(1) as i32) as usize;
        let offset = usize::from(self.viewer.scroll).min(self.viewer.cursor);
        self.viewer.scroll = offset
            .max((self.viewer.cursor + 1).saturating_sub(height))
            .min(max) as u16;
        debug_assert!(self.viewer.cursor < rows);
        debug_assert!(usize::from(self.viewer.scroll) <= max);
    }

    pub(crate) fn reload_preview(&mut self) -> Vec<Effect> {
        let Some(previous) = self.viewer.pending.clone() else {
            return Vec::new();
        };
        self.request_preview(previous)
    }

    pub(crate) fn preview_ready(&mut self, id: u64, result: Result<Preview, String>) {
        if !self.viewer.open || id != self.viewer.request {
            return;
        }
        debug_assert!(self.viewer.pending.is_some());
        debug_assert_eq!(self.viewer.pending.as_ref().map(|p| p.id), Some(id));
        self.viewer.preview = Some(result);
        self.dirty = true;
    }
}
