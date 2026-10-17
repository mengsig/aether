//! Rendering. Every function here reads [`App`] and paints; what it derives
//! from layout (wrap, scroll anchors) lives in the runner-owned [`ViewCache`].

pub mod board;
pub mod bottom;
pub mod cells;
pub mod layout;
pub mod markdown;
pub mod selection;
pub(crate) mod text_width;
pub mod theme;
pub mod transcript;
pub mod views;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::ui::transcript::ViewCache;

/// Paint one frame (CONTRACTS §4.2): the transcript, the board beside or
/// over it, then the bottom pane from the needs-you rows down to the hint row.
pub fn render(frame: &mut Frame, app: &App, cache: &mut ViewCache) {
    cache.update_presentation(app);
    cache.regions = selection::FrameRegions::default();
    selection::validate(app, cache);
    render_panes(frame, app, cache);
    selection::highlight(frame.buffer_mut(), app, cache);
}

fn render_panes(frame: &mut Frame, app: &App, cache: &mut ViewCache) {
    let area = frame.area();
    let theme = &app.theme;
    let top_view = app.bottom.top_view();
    if let Some(view) = top_view.filter(|view| view.fullscreen()) {
        let hint = Rect {
            y: area.bottom().saturating_sub(1),
            height: area.height.min(1),
            ..area
        };
        cache.regions.view = Rect {
            height: area.height - hint.height,
            ..area
        };
        view.render(
            frame,
            Rect {
                height: area.height - hint.height,
                ..area
            },
            app,
            theme,
        );
        bottom::footer::render(frame, hint, app, theme, false, false);
        return;
    }
    let mode = layout::board_mode(area.width, app.board.open);
    let width = bottom_width(area.width, mode);
    // The same predicate key dispatch uses: only a card on screen takes keys.
    let approval = app.shown_approval();
    let block = match (top_view, approval) {
        (Some(view), _) => view.desired_height(width, app),
        (None, Some(request)) => bottom::approval::desired_height(request, app, width),
        (None, None) => bottom::composer::desired_height(app, width, &mut cache.composer),
    };
    let mut status_row = bottom::status_row::model(app);
    let at = bottom_layout(app, area, mode, block, status_row.is_some());
    if let Some(row) = status_row.as_mut().filter(|_| at.needs_you.height > 0) {
        row.needs_you = 0;
    }

    cache.regions.transcript = at.transcript;
    cache.mouse.transcript_area = at.transcript;
    if top_view.is_some() {
        cache.regions.view = at.composer;
    } else if approval.is_none() && !app.approval_too_short() {
        cache.regions.composer_block = at.composer;
        let rows = bottom::composer::block_rows(app, at.composer.height);
        cache.regions.composer = Rect::new(
            at.composer.x + 2.min(at.composer.width),
            at.composer.y + rows.pad + rows.chips,
            at.composer.width.saturating_sub(4),
            rows.draft,
        );
    }
    transcript::render(frame, at.transcript, app, cache, theme);
    let reading = app.selected.is_some_and(|agent| {
        matches!(
            cache.transcript.anchor(agent),
            transcript::Anchor::Reading { .. }
        )
    });
    if let Some(separator) = at.separator {
        let rule =
            vec![Line::styled(theme.glyphs().pipe, theme.rule()); usize::from(separator.height)];
        frame.render_widget(Paragraph::new(rule), separator);
    }
    if let Some(board_area) = at.board {
        cache.regions.board = layout::board_inner(board_area);
        if mode == layout::BoardMode::Overlay {
            cache.regions.transcript = Rect::default();
            cache.regions.composer = Rect::default();
            cache.regions.composer_block = Rect::default();
            cache.regions.view = Rect::default();
            board::render_overlay(frame, board_area, app, &mut cache.board, theme);
            bottom::footer::render(
                frame,
                at.hint,
                app,
                theme,
                reading,
                mode == layout::BoardMode::Overlay,
            );
            return;
        }
        board::render(
            frame,
            layout::board_inner(board_area),
            app,
            &mut cache.board,
            theme,
        );
    }
    bottom::pending::render_needs_you(frame, at.needs_you, app, theme);
    bottom::pending::render_pending(frame, at.pending, app, theme);
    bottom::agents_strip::render(frame, at.agents_strip, app, theme);
    if let Some(row) = &status_row {
        bottom::status_row::render(frame, at.status_row, row, app, theme);
    }
    match (top_view, approval) {
        (Some(view), _) => view.render(frame, at.composer, app, theme),
        (None, Some(request)) => {
            bottom::approval::render(
                frame,
                at.composer,
                request,
                &app.bottom.approval,
                app,
                theme,
            );
        }
        (None, None) if app.approval_too_short() => {
            frame.render_widget(
                Paragraph::new("Terminal too short to show approval; resize to answer")
                    .style(theme.accent()),
                at.composer,
            );
        }
        (None, None) => {
            let cursor =
                bottom::composer::render(frame, at.composer, app, &mut cache.composer, theme);
            if let Some(popup) = &app.bottom.popup {
                let popup = bottom::popup::render(frame, at.composer, popup, app, theme);
                cache.regions.view = popup;
                cache.regions.transcript.height = cache
                    .regions
                    .transcript
                    .height
                    .min(popup.y.saturating_sub(at.transcript.y));
            } else if app.bottom.shortcuts {
                let popup = bottom::shortcuts::render(
                    frame,
                    at.composer,
                    app.bottom.shortcuts_scroll,
                    theme,
                    &app.keys,
                    app.help_modes(),
                    &app.options.host_commands,
                );
                cache.regions.view = popup;
                cache.regions.transcript.height = cache
                    .regions
                    .transcript
                    .height
                    .min(popup.y.saturating_sub(at.transcript.y));
            }
            if let Some((x, y)) = cursor {
                frame.set_cursor_position((x, y));
            }
        }
    }
    bottom::status_line::render(frame, at.status_line, app, theme);
    match &app.bottom.find {
        Some(bar)
            if app
                .status
                .as_ref()
                .is_none_or(|status| status.low_disk.is_none()) =>
        {
            bottom::find::render(frame, at.hint, bar, app.live_flash(), theme)
        }
        _ => bottom::footer::render(
            frame,
            at.hint,
            app,
            theme,
            reading,
            mode == layout::BoardMode::Overlay,
        ),
    }
}

/// The frame's regions with the bottom block (view, approval or composer) `block` rows tall.
fn bottom_layout(
    app: &App,
    area: Rect,
    mode: layout::BoardMode,
    block: u16,
    status_row: bool,
) -> layout::FrameLayout {
    let width = bottom_width(area.width, mode);
    let rows = layout::BottomRows {
        needs_you: bottom::pending::needs_you_height(app),
        pending: bottom::pending::pending_height(app, width),
        agents_strip: bottom::agents_strip::height(app),
        status_row: u16::from(status_row),
        composer: block,
        status_line: 1,
        hint: 1,
    };
    layout::frame_layout(area, mode, rows)
}

/// The composer's rect at `app.size` as [`render`] places it with no view or
/// approval over it: keys that depend on painted geometry (`?` scrolling) ask here.
pub(crate) fn composer_area(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.size.0, app.size.1);
    let mode = layout::board_mode(area.width, app.board.open);
    let width = bottom_width(area.width, mode);
    let block = bottom::composer::desired_height(app, width, &mut Default::default());
    let status_row = bottom::status_row::model(app).is_some();
    bottom_layout(app, area, mode, block, status_row).composer
}

/// Columns the transcript and bottom pane get beside the board.
pub fn bottom_width(cols: u16, mode: layout::BoardMode) -> u16 {
    match mode {
        layout::BoardMode::Docked { width } => cols.saturating_sub(width + 2),
        _ => cols,
    }
}

#[cfg(test)]
pub(crate) mod tests_support;

#[cfg(test)]
mod snapshot_dump;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::approval_flow::fixtures::{at, draw, fleet, rows, shell_approval};
    use crate::app::transcript::ScrollIntent;

    #[test]
    fn needs_you_appears_once_across_the_bottom_surfaces() {
        let mut f = fleet();
        f.app.board.open = false;
        let request = shell_approval(f.tester, "cargo test", at(-1000));
        f.app.bottom.approval.deferred.insert(request.id);
        f.app.approvals = vec![request];
        let mut cache = ViewCache::default();
        let screen = draw(120, 40, |frame| render(frame, &f.app, &mut cache));
        let text = rows(&screen).join("\n");
        assert_eq!(text.matches("tester").count(), 1, "{text}");
        assert!(text.contains("tester wants approval"), "{text}");
        assert!(!text.contains("1 needs you"), "{text}");
    }

    #[test]
    fn the_footer_tracks_the_renderers_reading_anchor_and_follow() {
        let mut app = crate::testing::app_with_transcript(&["an answer"; 100]);
        let agent = app.selected.expect("selected agent");
        app.board.open = false;
        app.flash = None;
        let mut cache = ViewCache::default();
        app.scroll_log.push(agent, ScrollIntent::PageUp);
        let screen = draw(120, 40, |frame| render(frame, &app, &mut cache));
        assert!(
            rows(&screen)
                .last()
                .expect("footer")
                .contains("↑ reading · ctrl+end to follow")
        );
        app.scroll_log.push(agent, ScrollIntent::Latest);
        let screen = draw(120, 40, |frame| render(frame, &app, &mut cache));
        assert!(!rows(&screen).last().expect("footer").contains("reading"));
    }

    #[test]
    fn failed_exec_keeps_its_styles_through_the_transcript_renderer() {
        use crate::app::approval_flow::fixtures::{assert_style, column_of, push_cell};
        use crate::app::transcript::{CallStatus, CellModel};
        use crate::ui::theme::ToolKind;

        let mut f = fleet();
        f.app.board.open = false;
        push_cell(
            &mut f.app,
            f.root,
            at(0),
            CellModel::Exec {
                call: None,
                command: "cat missing-notes.txt".into(),
                status: CallStatus::Failed,
                output: "cat: missing-notes.txt: No such file or directory".into(),
                exit: Some(1),
                duration_ms: Some(10),
                is_search: false,
                artifacts: Vec::new(),
            },
        );
        let mut cache = ViewCache::default();
        for _ in 0..2 {
            let screen = draw(120, 40, |frame| render(frame, &f.app, &mut cache));
            let rows = rows(&screen);
            let head = rows
                .iter()
                .position(|row| row.contains("• Ran cat"))
                .expect("head") as u16;
            assert_style(
                &screen,
                column_of(&screen, head, "•"),
                head,
                f.app.theme.failure().patch(f.app.theme.title()),
            );
            assert_style(
                &screen,
                column_of(&screen, head, "Ran"),
                head,
                f.app.theme.tool(ToolKind::Shell).patch(f.app.theme.title()),
            );
            assert_style(
                &screen,
                column_of(&screen, head + 1, "cat:"),
                head + 1,
                f.app.theme.failure(),
            );
        }
    }
}
