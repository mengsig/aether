//! The host workspace picker never inspects local paths.
use super::picker::{Picker, PickerRow};
use crate::app::{
    App,
    host::{HostRequest, request},
};
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::theme::tokens::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Frame, layout::Rect};
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct RemoteWorkspaceView {
    query: String,
    cursor: usize,
}

impl RemoteWorkspaceView {
    fn picker(&self, app: &App) -> Picker<PathBuf> {
        let entries = if self.query.starts_with(['/', '~', '.']) {
            app.host
                .directory
                .as_ref()
                .filter(|(prefix, _)| prefix == &self.query)
                .map(|(_, paths)| paths.iter().map(PathBuf::from).collect())
                .unwrap_or_default()
        } else {
            app.host
                .workspaces
                .iter()
                .map(|w| w.path.clone())
                .collect::<Vec<_>>()
        };
        let rows = entries
            .into_iter()
            .map(|path| {
                (
                    PickerRow::new(
                        path.display().to_string(),
                        app.branch_at(&path).unwrap_or(""),
                    ),
                    path,
                )
            })
            .collect();
        let mut picker = Picker::new("Host workspace", rows);
        if !self.query.starts_with(['/', '~', '.']) {
            picker.filter = self.query.clone();
        }
        picker.cursor = self.cursor.min(picker.visible().len().saturating_sub(1));
        picker
    }

    fn changed(&mut self) -> ViewOutcome {
        if self.query.starts_with('.') {
            return ViewOutcome::Act(vec![ViewAction::Flash(
                "use an absolute host path or ~/ for workspace completion".into(),
            )]);
        }
        if !self.query.starts_with(['/', '~', '.']) {
            return ViewOutcome::Consumed;
        }
        self.cursor = 0;
        let prefix = self.query.clone();
        ViewOutcome::Act(vec![ViewAction::Effect(Box::new(request(
            aether_protocol::Command::CompletePath {
                kind: aether_protocol::remote::PathCompletionKind::Directory,
                prefix: prefix.clone(),
                workspace: None,
                limit: Some(200),
            },
            HostRequest::Directory(prefix),
        )))])
    }
}

impl BottomView for RemoteWorkspaceView {
    fn kind(&self) -> Option<super::ViewKind> {
        Some(super::ViewKind::Workspace)
    }
    fn handle_key(&mut self, key: KeyEvent, app: &App) -> ViewOutcome {
        let picker = self.picker(app);
        match key.code {
            KeyCode::Char(c) => {
                self.query.push(c);
                self.changed()
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.changed()
            }
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                ViewOutcome::Consumed
            }
            KeyCode::Down => {
                self.cursor = self
                    .cursor
                    .saturating_add(1)
                    .min(picker.visible().len().saturating_sub(1));
                ViewOutcome::Consumed
            }
            KeyCode::Tab => {
                if let Some(path) = picker.selected() {
                    self.query = format!("{}/", path.display().to_string().trim_end_matches('/'));
                }
                self.changed()
            }
            KeyCode::Enter => {
                let path = picker
                    .selected()
                    .cloned()
                    .or_else(|| (!self.query.is_empty()).then(|| PathBuf::from(&self.query)));
                match path {
                    Some(path) => ViewOutcome::Close(vec![ViewAction::Command(
                        crate::commands::SlashCommand::Workspace(Some(path)),
                    )]),
                    None => ViewOutcome::Consumed,
                }
            }
            KeyCode::Esc => ViewOutcome::Close(Vec::new()),
            _ => ViewOutcome::Ignored,
        }
    }
    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        self.query.push_str(text.lines().next().unwrap_or(""));
        self.changed()
    }
    fn desired_height(&self, _width: u16, app: &App) -> u16 {
        super::frame_height(self.picker(app).body_len())
    }
    fn render(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        self.picker(app).render_titled(
            frame,
            area,
            super::title("Host workspace", &self.query, theme),
            theme,
            &[
                ("type", "host path"),
                ("tab", "complete"),
                ("enter", "open"),
            ],
        );
    }
}
