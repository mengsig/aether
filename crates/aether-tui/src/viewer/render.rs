use super::{Preview, Viewer, references::Reference};
use crate::{
    app::App,
    ui::{cells::text::clean, theme::Theme},
};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct Hit {
    pub area: Rect,
    pub reference: Reference,
    pub cell: crate::app::transcript::CellId,
}

pub fn split(area: Rect, viewer: &Viewer) -> (Rect, Rect) {
    if !viewer.open {
        return (area, Rect::default());
    }
    if viewer.fullscreen || area.width < 40 {
        return (Rect::default(), area);
    }
    let right = ((u32::from(area.width) * u32::from(viewer.share)) / 100) as u16;
    let left = area.width.saturating_sub(right);
    debug_assert!(left <= area.width);
    debug_assert!(right <= area.width);
    (
        Rect {
            width: left,
            ..area
        },
        Rect {
            x: area.x + left,
            width: right,
            ..area
        },
    )
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.is_empty() {
        return;
    }
    let viewer = &app.viewer;
    frame.render_widget(Clear, area);
    let focus = if viewer.focused { "chat" } else { "focus" };
    let path = viewer
        .path()
        .map(|p| clean(&p.file_name().unwrap_or(p.as_os_str()).to_string_lossy()))
        .unwrap_or_else(|| "Files".into());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {path} · Tab: {focus} "))
        .style(app.theme.text());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let rows = content(viewer, &app.theme);
    frame.render_widget(
        Paragraph::new(rows)
            .scroll((viewer.scroll, 0))
            .wrap(Wrap { trim: false }),
        body,
    );
    if inner.height > 0 {
        let footer = Rect {
            y: inner.bottom() - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(if !viewer.focused {
                "Alt+W close · Tab focus viewer · Alt+Z full"
            } else if matches!(viewer.preview, Some(Ok(Preview::Outside(_)))) {
                "Enter allow this file · Esc close"
            } else {
                "Esc close · Alt+E external · Alt+Z full · Alt+-/= resize"
            }),
            footer,
        );
    }
}

fn content(viewer: &Viewer, theme: &Theme) -> Vec<Line<'static>> {
    if !viewer.picker.is_empty() {
        return viewer
            .picker
            .iter()
            .enumerate()
            .map(|(i, r)| {
                Line::styled(
                    format!(
                        "{} {}",
                        if i == viewer.cursor { ">" } else { " " },
                        clean(&r.target)
                    ),
                    if i == viewer.cursor {
                        theme.accent()
                    } else {
                        theme.text()
                    },
                )
            })
            .collect();
    }
    match &viewer.preview {
        None => vec![Line::from("Loading…")],
        Some(Ok(Preview::Failed { error, .. })) | Some(Err(error)) => {
            vec![Line::from(clean(error))]
        }
        Some(Ok(Preview::Outside(path))) => vec![
            Line::from(clean(&path.display().to_string())),
            Line::from("Outside agent workspace. Enter to allow this file; Esc to cancel."),
        ],
        Some(Ok(Preview::Unsupported(path))) => vec![
            Line::from(clean(&path.display().to_string())),
            Line::from(
                "Use Alt+E to open externally. PDF/HTML/SVG pane rendering is not available yet.",
            ),
        ],
        Some(Ok(Preview::Image { rows, .. })) => rows.clone(),
        Some(Ok(Preview::Text { rows, .. })) => rows.clone(),
    }
}

pub fn mark_references(
    line: &mut Line<'static>,
    at: Rect,
    refs: &[(Reference, Vec<(usize, usize)>)],
    cell: crate::app::transcript::CellId,
    hits: &mut Vec<Hit>,
    theme: &Theme,
) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    for (reference, ranges) in refs {
        for &(start, end) in ranges {
            debug_assert!(start <= end && end <= text.len());
            debug_assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
            let x = text[..start].width();
            let width = text[start..end]
                .width()
                .min(usize::from(at.width).saturating_sub(x));
            if width == 0 {
                continue;
            }
            hits.push(Hit {
                area: Rect::new(at.x + x as u16, at.y, width as u16, 1),
                reference: reference.clone(),
                cell,
            });
        }
        crate::ui::transcript::highlight_line(line, ranges, theme.file_reference());
    }
}
