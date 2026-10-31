//! Resolve and extract selections against the transcript's cell-based layout.

use super::window::Pos;
use super::{ViewCache, rows_for, window};
use crate::app::App;

pub(crate) fn selection_order(
    app: &App,
    point: crate::ui::selection::TextPoint,
) -> Option<(usize, usize, u16)> {
    let t = app.transcripts.get(&app.selected?)?;
    let index = if point.cell == window::HEADER_ID {
        0
    } else if point.cell == window::STREAM_ID {
        if point.stream_generation != Some(t.stream.generation) || t.stream.is_empty() {
            return None;
        }
        usize::MAX
    } else {
        t.position(point.cell)
            .or_else(|| {
                if !t.pending_messages.contains_key(&point.cell) {
                    return None;
                }
                // Receipt IDs order pending messages after committed cells.
                t.cells
                    .len()
                    .checked_add(usize::from(t.active.is_some()))?
                    .checked_add(usize::try_from(point.cell.0).ok()?)
            })?
            .checked_add(1)?
    };
    Some((index, point.row, point.column))
}

pub(crate) fn selection_resolves(app: &App, cache: &mut ViewCache) -> bool {
    let Some(s) = cache
        .mouse
        .selection
        .as_ref()
        .filter(|s| s.pane == crate::ui::selection::Pane::Transcript)
    else {
        return true;
    };
    let Some((agent, t)) = s.agent.and_then(|id| Some((id, app.transcripts.get(&id)?))) else {
        return false;
    };
    let rows = rows_for(app, agent, t, s.rect.width, &mut cache.stream, &app.theme);
    [s.start, s.end].into_iter().all(|point| {
        selection_order(app, point).is_some()
            && rows
                .find(point.cell)
                .is_some_and(|item| point.row < rows.block(item, &mut cache.layout))
    })
}

pub(crate) fn selection_rows(
    app: &App,
    cache: &mut ViewCache,
) -> Vec<Option<crate::ui::selection::TextPoint>> {
    use crate::ui::selection::TextPoint;
    let area = cache.regions.transcript;
    let Some((agent, t)) = app
        .selected
        .and_then(|id| Some((id, app.transcripts.get(&id)?)))
    else {
        return Vec::new();
    };
    let rows = rows_for(app, agent, t, area.width, &mut cache.stream, &app.theme);
    let full = cache.mouse.transcript_area;
    let mut pos = rows.resolve(
        cache.mouse.displayed_top,
        usize::from(full.height),
        &mut cache.layout,
    );
    pos = rows.down(
        pos,
        usize::from(area.y.saturating_sub(full.y)),
        &mut cache.layout,
    );
    let mut result = Vec::with_capacity(usize::from(area.height));
    while result.len() < usize::from(area.height) && pos.item < rows.len() {
        let block = rows.block(pos.item, &mut cache.layout);
        for row in pos.row..block {
            if result.len() == usize::from(area.height) {
                break;
            }
            result.push(Some(TextPoint {
                stream_generation: (rows.id(pos.item) == window::STREAM_ID)
                    .then_some(t.stream.generation),
                cell: rows.id(pos.item),
                row,
                column: 0,
            }));
        }
        pos = Pos {
            item: pos.item + 1,
            row: 0,
        };
    }
    if let Some(pinned) = cache.mouse.pinned {
        if let Some(first) = result.first_mut() {
            *first = Some(pinned);
        }
        if let Some(second) = result.get_mut(1) {
            *second = None;
        }
    }
    result
}

pub(crate) fn selection_point(
    app: &App,
    cache: &mut ViewCache,
    x: u16,
    y: u16,
) -> Option<crate::ui::selection::TextPoint> {
    let area = cache.regions.transcript;
    let mut point = selection_rows(app, cache)
        .get(usize::from(y - area.y))
        .copied()
        .flatten()?;
    point.column = x - area.x;
    Some(point)
}

pub(crate) fn selected_text(app: &App, cache: &mut ViewCache) -> Result<String, String> {
    use crate::ui::selection::append_row;
    let s = cache.mouse.selection.as_ref().ok_or("no selection")?;
    let agent = s.agent.ok_or("no selected agent")?;
    let t = app
        .transcripts
        .get(&agent)
        .ok_or("transcript no longer available")?;
    let rows = rows_for(app, agent, t, s.rect.width, &mut cache.stream, &app.theme);
    let mut a = (
        rows.find(s.start.cell)
            .ok_or("selected content no longer available")?,
        s.start.row,
        s.start.column,
    );
    let mut b = (
        rows.find(s.end.cell)
            .ok_or("selected content no longer available")?,
        s.end.row,
        s.end.column,
    );
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    let mut text = String::new();
    for item in a.0..=b.0 {
        let gap = rows.gap(item, &mut cache.layout);
        let lines = rows.lines(item, &mut cache.layout);
        for row in 0..gap + lines.len() {
            if (item, row) < (a.0, a.1) || (item, row) > (b.0, b.1) {
                continue;
            }
            let key = (rows.id(item), row);
            let excerpt = if key == (s.start.cell, s.start.row) {
                s.start_excerpt.as_deref()
            } else {
                None
            }
            .or_else(|| {
                if key == (s.end.cell, s.end.row) {
                    s.end_excerpt.as_deref()
                } else {
                    None
                }
            });
            let line = excerpt.map(str::to_owned).unwrap_or_else(|| {
                row.checked_sub(gap)
                    .and_then(|r| lines.get(r))
                    .map(|l| {
                        l.spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>()
                    })
                    .unwrap_or_default()
            });
            append_row(
                &mut text,
                &line,
                if (item, row) == (a.0, a.1) { a.2 } else { 0 },
                if (item, row) == (b.0, b.1) {
                    b.2.saturating_add(1)
                } else {
                    s.rect.width
                },
            )?;
        }
    }
    text.pop();
    Ok(text)
}

/// The source line under `point`: its first and last rows in the cell and its
/// text, soft wraps rejoined. Rows whose wrapping is unknown stand alone.
pub(crate) fn line_at(
    app: &App,
    cache: &mut ViewCache,
    point: crate::ui::selection::TextPoint,
) -> Option<(usize, usize, String)> {
    let agent = app.selected?;
    let t = app.transcripts.get(&agent)?;
    let width = cache.regions.transcript.width;
    let rows = rows_for(app, agent, t, width, &mut cache.stream, &app.theme);
    let item = rows.find(point.cell)?;
    let gap = rows.gap(item, &mut cache.layout);
    let lines = rows.lines(item, &mut cache.layout);
    let row = point.row.checked_sub(gap)?;
    lines.get(row)?;
    let mut first = row;
    while first > 0 && lines.source(first).continues.is_some() {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < lines.len() && lines.source(last + 1).continues.is_some() {
        last += 1;
    }
    let mut text = String::new();
    for r in first..=last {
        let source = lines.source(r);
        let (part, start) = source.text_of(lines.get(r)?);
        if r == first {
            text.push_str(&part);
            continue;
        }
        debug_assert!(source.continues.is_some(), "row {r} joins the line above");
        text.extend(std::iter::repeat_n(' ', source.continues.unwrap_or(0)));
        text.push_str(&part[start..]);
    }
    debug_assert!(first <= row && row <= last && last < lines.len());
    Some((gap + first, gap + last, text.trim_end().to_owned()))
}
