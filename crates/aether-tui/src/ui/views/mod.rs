//! Slash views: the old overlays as bottom-pane list views (SPEC §6.1, §8
//! Phase 6). Each implements `ui::bottom::BottomView`. Owner: W7.

pub mod approvals;
pub mod artifacts;
pub mod doctor;
pub mod keys;
pub mod login;
pub mod mcp;
pub mod memory;
pub mod model;
pub mod modes;
pub mod notifications;
pub mod outbox;
pub mod pager;
pub mod picker;
pub mod remote_workspace;
pub mod report;
pub mod schedules;
pub mod settings;
pub mod theme;
pub mod usage;
pub mod voice;
pub mod workspace;

#[cfg(test)]
mod tests;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use aether_core::id::ArtifactId;

use crate::app::App;
use crate::ui::bottom::BottomView;
use crate::ui::theme::tokens::Theme;
use crate::voice::setup::ModelKind;

pub(super) fn wheel_key(up: bool) -> KeyEvent {
    KeyEvent::new(
        if up { KeyCode::Up } else { KeyCode::Down },
        KeyModifiers::NONE,
    )
}

/// Every view a slash command can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Modes,
    /// `/model`: model + effort, two columns.
    Model,
    /// `/usage`: providers, sign-in, health, limit windows, engine load.
    Usage,
    /// `/approvals`: rules list, revoke, auto-approve toggle.
    Approvals,
    Memory,
    /// `/notifications` and Alt+N: the notification center.
    Notifications,
    Schedules,
    Mcp,
    /// `/outbox`: remote operations whose delivery is unconfirmed.
    Outbox,
    Doctor,
    Report,
    Artifacts,
    /// `/settings`: theme, motion, mouse, ascii; shows the overrides path.
    Settings,
    Keys,
    /// `/theme`: built-in themes and theme files, each with a swatch.
    Theme,
    Voice,
    /// `/workspace` and Ctrl+O: where agents work.
    Workspace,
    /// `/login`: provider picker.
    Login,
    /// Full-screen pager on one artifact's bytes (from `/artifacts`).
    ArtifactPager(ArtifactId),
    /// Start installing a voice backend the person confirmed (from `/voice`).
    VoiceInstall(ModelKind),
}

/// Build the view for `kind` from current app state. Data refreshes are
/// requested separately by `App::open_view` (`app/views.rs`).
pub fn open(kind: ViewKind, app: &App) -> Box<dyn BottomView> {
    match kind {
        ViewKind::Modes => Box::new(modes::ModesView::default()),
        ViewKind::Model => Box::new(model::ModelView::new(app)),
        ViewKind::Usage => Box::new(usage::UsageView::default()),
        ViewKind::Approvals => Box::new(approvals::ApprovalsView::default()),
        ViewKind::Memory => Box::new(memory::MemoryView::default()),
        ViewKind::Notifications => Box::new(notifications::NotificationsView::default()),
        ViewKind::Schedules => Box::new(schedules::SchedulesView::default()),
        ViewKind::Mcp => Box::new(mcp::McpView::default()),
        ViewKind::Outbox => Box::new(outbox::OutboxView::default()),
        ViewKind::Doctor => Box::new(doctor::DoctorView::default()),
        ViewKind::Report => Box::new(report::ReportView::default()),
        ViewKind::Artifacts => Box::new(artifacts::ArtifactsView::default()),
        ViewKind::Settings => Box::new(settings::SettingsView::default()),
        ViewKind::Keys => Box::new(keys::KeysView::default()),
        ViewKind::Theme => Box::new(theme::ThemeView::new(app)),
        ViewKind::Voice => Box::new(voice::VoiceView::new(app)),
        ViewKind::Workspace => Box::new(workspace::WorkspaceView::new(app)),
        ViewKind::Login => Box::new(login::LoginView::default()),
        ViewKind::ArtifactPager(id) => {
            Box::new(pager::PagerView::new(pager::PagerSource::Artifact(id)))
        }
        ViewKind::VoiceInstall(model) => Box::new(voice::VoiceInstallView::new(model)),
    }
}

/// Body rows a view shows before it scrolls.
pub const BODY_ROWS: usize = 12;
/// Rows of frame around the body: padding, title, gap, gap, hints, padding.
const CHROME_ROWS: u16 = 6;
/// The 2-column inner margin on each side, as in the approval card.
const MARGIN: &str = "  ";

/// One footer hint: the key in `title()`, what it does in `secondary()`.
pub type Hint = (&'static str, &'static str);

/// ` · ` in unicode, ` | ` in ascii.
pub fn sep(theme: &Theme) -> String {
    format!(" {} ", theme.glyphs().dot)
}

/// A short id cut with the theme's ellipsis.
pub fn short_id(theme: &Theme, id: &impl std::fmt::Display) -> String {
    let marker = theme.glyphs().ellipsis.chars().next().unwrap_or('~');
    crate::format::short_id_with_marker(id, marker)
}

/// Daemon- or agent-supplied text made safe for one row: no control bytes,
/// first line only.
/// `command output`, not the `{:?}` spelling `commandoutput`.
pub fn artifact_kind(kind: &aether_core::artifact::ArtifactKind) -> String {
    use aether_core::artifact::ArtifactKind;
    match kind {
        ArtifactKind::File => "file".to_owned(),
        ArtifactKind::CommandOutput => "command output".to_owned(),
        ArtifactKind::Diff => "diff".to_owned(),
        ArtifactKind::Commit => "commit".to_owned(),
        ArtifactKind::Result => "result".to_owned(),
        ArtifactKind::Other { label } => clean(label),
    }
}

pub fn clean(text: &str) -> String {
    let text = aether_core::text::strip_controls(text);
    text.lines().next().unwrap_or("").trim_end().to_owned()
}

/// A path whole, as people write it: `~` for home. Views give a path its
/// own row, so it is never cut to its last parts the way the status line does.
pub fn home_path(path: &std::path::Path) -> String {
    let home = dirs::home_dir();
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Wanted height of a framed view whose body has `body_rows` rows.
pub fn frame_height(body_rows: usize) -> u16 {
    CHROME_ROWS + body_rows.clamp(1, BODY_ROWS) as u16
}

/// Which optional frame rows a height leaves room for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chrome {
    pad_top: bool,
    gap_title: bool,
    gap_hints: bool,
    hints: bool,
    pad_bottom: bool,
}

impl Chrome {
    /// Short panes drop gaps, then top padding, then hints; bottom padding
    /// goes last, so the block never runs into the status line.
    fn fit(height: u16) -> (Chrome, usize) {
        let mut chrome = Chrome {
            pad_top: true,
            gap_title: true,
            gap_hints: true,
            hints: true,
            pad_bottom: true,
        };
        let drops: [fn(&mut Chrome) -> &mut bool; 5] = [
            |c| &mut c.gap_title,
            |c| &mut c.gap_hints,
            |c| &mut c.pad_top,
            |c| &mut c.hints,
            |c| &mut c.pad_bottom,
        ];
        for drop in drops {
            if usize::from(height) > chrome.rows() {
                break;
            }
            *drop(&mut chrome) = false;
        }
        (chrome, usize::from(height).saturating_sub(chrome.rows()))
    }

    /// Frame rows in use, the title included.
    fn rows(self) -> usize {
        let flags = [
            self.pad_top,
            self.gap_title,
            self.gap_hints,
            self.hints,
            self.pad_bottom,
        ];
        1 + flags.into_iter().filter(|on| *on).count()
    }
}

/// Body rows a framed view gets in `area`.
pub fn body_room(area: Rect) -> usize {
    Chrome::fit(area.height).1
}

/// The approval card's tinted frame (SPEC §3.4): title, body, hints. Body rows
/// carry their own margin; rows past [`body_room`] are cut behind a cue.
pub fn render_frame(
    frame: &mut Frame,
    area: Rect,
    title: Vec<Span<'static>>,
    body: Vec<Line<'static>>,
    hints: &[Hint],
    theme: &Theme,
) {
    if area.is_empty() {
        return;
    }
    let (chrome, room) = Chrome::fit(area.height);
    let width = area.width.saturating_sub(MARGIN.len() as u16);
    let mut lines = Vec::with_capacity(usize::from(area.height));
    if chrome.pad_top {
        lines.push(Line::default());
    }
    let mut title = title;
    title.insert(0, Span::raw(MARGIN));
    lines.push(picker::clip_line(Line::from(title), width, theme));
    if chrome.gap_title {
        lines.push(Line::default());
    }
    let window = Window::scrolled(body.len(), room, 0);
    lines.extend(
        window
            .lines(body, theme)
            .into_iter()
            .map(|line| picker::clip_line(line, width, theme)),
    );
    if chrome.gap_hints || chrome.hints {
        // A short body still sits the hints on the frame's bottom rows.
        let used = lines.len() + usize::from(chrome.hints) + usize::from(chrome.pad_bottom);
        let fill = usize::from(area.height).saturating_sub(used);
        lines.extend(std::iter::repeat_n(Line::default(), fill));
    }
    if chrome.hints {
        lines.push(hints_line(hints, width, theme));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.tint()), area);
}

/// `enter apply · esc back`, keys bold. Hints that do not fit go from the
/// middle, so the first and the way out (last) stay.
fn hints_line(hints: &[Hint], width: u16, theme: &Theme) -> Line<'static> {
    let dot = sep(theme);
    let key = |key: &str| -> String {
        if !theme.ascii() {
            return key.to_owned();
        }
        key.replace("↑↓", "up/down").replace("←→", "left/right")
    };
    let mut kept: Vec<(String, &str)> = hints.iter().map(|(k, d)| (key(k), *d)).collect();
    let used = |kept: &[(String, &str)]| -> usize {
        let words: usize = kept.iter().map(|(k, d)| k.width() + 1 + d.width()).sum();
        MARGIN.len() + words + dot.width() * kept.len().saturating_sub(1)
    };
    while kept.len() > 2 && used(&kept) > usize::from(width) {
        kept.remove(kept.len() - 2);
    }
    let mut spans = vec![Span::raw(MARGIN)];
    for (i, (key, desc)) in kept.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(dot.clone(), theme.secondary()));
        }
        spans.push(Span::styled(key, theme.title()));
        spans.push(Span::styled(format!(" {desc}"), theme.secondary()));
    }
    picker::clip_line(Line::from(spans), width, theme)
}

/// The on-screen rows of a long body; a clipped one ends in `↓ N more`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub shown: usize,
    len: usize,
    cue: bool,
}

impl Window {
    /// `room` rows of a `len`-row body, scrolled down by `offset`.
    pub fn scrolled(len: usize, room: usize, offset: usize) -> Window {
        let (shown, cue) = Window::fit(len, room);
        let start = offset.min(len - shown);
        Window {
            start,
            shown,
            len,
            cue,
        }
    }

    /// `room` rows of a `len`-row list, with row `cursor` in view.
    pub fn following(len: usize, room: usize, cursor: usize) -> Window {
        let (shown, cue) = Window::fit(len, room);
        let start = (cursor + 1).saturating_sub(shown).min(len - shown);
        Window {
            start,
            shown,
            len,
            cue,
        }
    }

    /// Rows shown and whether a cue row is needed. Below two rows of room
    /// the cue would crowd out the content itself, so it is left off.
    fn fit(len: usize, room: usize) -> (usize, bool) {
        if len <= room {
            (len, false)
        } else if room < 2 {
            (room, false)
        } else {
            (room - 1, true)
        }
    }

    /// The shown slice of `body`, then the cue when clipped.
    pub fn lines(self, body: Vec<Line<'static>>, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines: Vec<_> = body.into_iter().skip(self.start).take(self.shown).collect();
        if self.cue {
            lines.push(self.cue_line(theme));
        }
        lines
    }

    fn cue_line(self, theme: &Theme) -> Line<'static> {
        let below = self.len - self.start - self.shown;
        let text = match (below, theme.ascii()) {
            (0, false) => format!("↑ {} more", self.start),
            (0, true) => format!("{} more above", self.start),
            (n, false) => format!("↓ {n} more"),
            (n, true) => format!("{n} more below"),
        };
        row(text, theme.secondary())
    }
}

/// Scroll position of a read-only body. Lines are cut, never wrapped, so the
/// line count (and so the limit) does not depend on the width.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BodyScroll {
    pub offset: usize,
}

impl BodyScroll {
    /// ↑↓ PgUp PgDn Home End; `true` when the key was a scroll key.
    pub fn key(&mut self, key: KeyEvent, total: usize) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        // A clipped body shows one row fewer: the last goes to the cue.
        let max = if total > BODY_ROWS {
            total - (BODY_ROWS - 1)
        } else {
            0
        };
        self.offset = match key.code {
            KeyCode::Up => self.offset.saturating_sub(1),
            KeyCode::Down => self.offset.saturating_add(1),
            KeyCode::PageUp => self.offset.saturating_sub(BODY_ROWS - 2),
            KeyCode::PageDown => self.offset.saturating_add(BODY_ROWS - 2),
            KeyCode::Home => 0,
            KeyCode::End => max,
            _ => return false,
        }
        .min(max);
        true
    }
}

/// A read-only view: the frame around `body`, scrolled to `scroll`.
pub fn render_body(
    frame: &mut Frame,
    area: Rect,
    title: Vec<Span<'static>>,
    body: Vec<Line<'static>>,
    scroll: BodyScroll,
    hints: &[Hint],
    theme: &Theme,
) {
    let window = Window::scrolled(body.len(), body_room(area), scroll.offset);
    let body = window.lines(body, theme);
    render_frame(frame, area, title, body, hints, theme);
}

/// A title in `title()` with dim meta after it: `Approvals  auto-approve off`.
pub fn title(
    text: impl Into<String>,
    meta: impl Into<String>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let meta = meta.into();
    let mut spans = vec![Span::styled(text.into(), theme.title())];
    if !meta.is_empty() {
        spans.push(Span::styled(format!("  {meta}"), theme.secondary()));
    }
    spans
}

/// A list view with nothing in it: the title, one dim sentence on what
/// would fill it, and only the way out.
pub fn render_empty(
    frame: &mut Frame,
    area: Rect,
    title: Vec<Span<'static>>,
    text: &str,
    theme: &Theme,
) {
    let body = vec![row(text, theme.secondary())];
    render_frame(frame, area, title, body, &[("esc", "back")], theme);
}

/// One body row at the margin, in a single style.
pub fn row(text: impl Into<String>, style: ratatui::style::Style) -> Line<'static> {
    Line::from(Span::styled(format!("{MARGIN}{}", text.into()), style))
}

/// `line` moved onto the margin.
pub fn indent(line: Line<'static>) -> Line<'static> {
    let mut spans = line.spans;
    spans.insert(0, Span::raw(MARGIN));
    Line::from(spans)
}

/// What a view answers to a key it has no use for: chords and function keys
/// go on to the global keys; anything typed is swallowed so it never edits
/// the hidden draft.
pub fn unhandled(key: KeyEvent) -> crate::ui::bottom::ViewOutcome {
    use crate::ui::bottom::ViewOutcome;
    let chord = !key.modifiers.is_empty();
    if chord || matches!(key.code, KeyCode::F(_) | KeyCode::BackTab) {
        ViewOutcome::Ignored
    } else {
        ViewOutcome::Consumed
    }
}
