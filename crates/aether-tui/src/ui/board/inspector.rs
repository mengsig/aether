//! Inspector: lineage, objective, asks, context bar, model, tokens, cost with
//! children, workspace/branch, children, last activity (§2.2). Owner: W3.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use aether_core::time::Timestamp;

use super::tree::right_columns;
use super::{
    ago_text, cost_text, elapsed_text, fit_spans, left_room, split_line, status_glyph,
    status_style, status_word, truncate,
};
use crate::app::board::{Asks, BoardStatus, InspectorModel};
use crate::ui::theme::tokens::Theme;

/// Label column width.
pub const LABEL_WIDTH: u16 = 11;
/// Objective, error and question text wrap to at most this many rows.
const TEXT_MAX_ROWS: usize = 3;
const CHILDREN_MAX_ROWS: usize = 5;
/// From here the context percentage turns bold (the colour stays accent).
const CONTEXT_WARN_PERCENT: u64 = 85;
/// Room right of the context bar: `  100%`.
const PERCENT_COLUMNS: usize = 6;

/// Rows that give way when the board is short, dropped in this order (§1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Kept,
    LastActivity,
    Branch,
    Workspace,
    Tokens,
}

const DROP_ORDER: [Part; 4] = [
    Part::LastActivity,
    Part::Branch,
    Part::Workspace,
    Part::Tokens,
];

pub fn render(
    frame: &mut Frame,
    area: Rect,
    model: &InspectorModel,
    now: Timestamp,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let mut lines = lines(model, usize::from(area.width), now, theme);
    let height = usize::from(area.height);
    for part in DROP_ORDER {
        if lines.len() <= height {
            break;
        }
        lines.retain(|(p, _)| *p != part);
    }
    let buf = frame.buffer_mut();
    for (y, (_, line)) in (area.y..area.y + area.height).zip(&lines) {
        buf.set_line(area.x, y, line, area.width);
    }
}

type Tagged = (Part, Line<'static>);

fn lines(model: &InspectorModel, width: usize, now: Timestamp, theme: &Theme) -> Vec<Tagged> {
    let mut out: Vec<Tagged> = Vec::new();
    let kept = |out: &mut Vec<Tagged>, line: Line<'static>| out.push((Part::Kept, line));

    for line in head(model, width, now, theme) {
        kept(&mut out, line);
    }
    kept(&mut out, lineage(model, width, now, theme));

    let summary = summary_lines(model, width, theme);
    if !summary.is_empty() {
        kept(&mut out, Line::default());
        out.extend(summary.into_iter().map(|l| (Part::Kept, l)));
    }
    if let Some(asks) = &model.asks {
        kept(&mut out, Line::default());
        out.extend(
            asks_lines(asks, width, theme)
                .into_iter()
                .map(|l| (Part::Kept, l)),
        );
    }

    kept(&mut out, Line::default());
    out.extend(
        context_lines(model, width, theme)
            .into_iter()
            .map(|l| (Part::Kept, l)),
    );
    let dot = format!(" {} ", theme.glyphs().dot);
    let mut model_parts = vec![model.model.clone()];
    model_parts.extend(model.effort.map(|effort| effort.to_string()));
    let mut tagged = |part: Part, name: &'static str, rows: Vec<String>| {
        out.extend(
            field(name, rows, width, theme)
                .into_iter()
                .map(|l| (part, l)),
        );
    };
    let value_width = width.saturating_sub(usize::from(LABEL_WIDTH));
    tagged(Part::Kept, "Model", pack(&model_parts, &dot, value_width));
    tagged(
        Part::Tokens,
        "Tokens",
        pack(&tokens(model), &dot, value_width),
    );
    if let Some(cost) = cost(model) {
        tagged(Part::Kept, "Cost", pack(&cost, &dot, value_width));
    }
    if let Some(workspace) = &model.workspace {
        let rows = pack(&path_parts(workspace), "", value_width);
        tagged(Part::Workspace, "Workspace", rows);
    }
    if let Some(branch) = &model.branch {
        tagged(
            Part::Branch,
            "Branch",
            pack(&path_parts(branch), "", value_width),
        );
    }

    if !model.children.is_empty() {
        kept(&mut out, Line::default());
        kept(
            &mut out,
            Line::from(Span::styled("Children", theme.secondary())),
        );
        out.extend(
            children_lines(model, width, now, theme)
                .into_iter()
                .map(|l| (Part::Kept, l)),
        );
    }

    if let Some(activity) = &model.last_activity {
        let ago = ago_text(u64::try_from((now - activity.at).num_milliseconds()).unwrap_or(0));
        out.push((Part::LastActivity, Line::default()));
        out.push((
            Part::LastActivity,
            split_line(
                vec![Span::styled("Last activity", theme.secondary())],
                vec![Span::styled(ago, theme.secondary())],
                width,
                theme,
            ),
        ));
        let verb_style = if activity.failed {
            theme.failure().patch(theme.title())
        } else {
            theme.title()
        };
        let action = vec![
            Span::styled(activity.verb.clone(), verb_style),
            Span::raw(" "),
            Span::styled(activity.rest.clone(), theme.text()),
        ];
        out.push((
            Part::LastActivity,
            Line::from(fit_spans(action, width, theme)),
        ));
    }
    out
}

/// `implementer  worker                ● Running`; a name too long for that
/// row wraps whole above it, since this is where an agent is identified.
fn head(model: &InspectorModel, width: usize, now: Timestamp, theme: &Theme) -> Vec<Line<'static>> {
    let chip = status_chip(model, now, theme);
    let name = Span::styled(model.name.clone(), theme.title());
    let role =
        (!model.role.is_empty()).then(|| Span::styled(model.role.clone(), theme.secondary()));
    if model.name.width() <= left_room(&chip, width) {
        let mut left = vec![name];
        if let Some(role) = role {
            left.extend([Span::raw("  "), role]);
        }
        return vec![split_line(left, chip, width, theme)];
    }
    let mut lines: Vec<Line<'static>> = textwrap::wrap(&model.name, width.max(1))
        .iter()
        .map(|part| Line::from(Span::styled(part.to_string(), theme.title())))
        .collect();
    lines.push(split_line(role.into_iter().collect(), chip, width, theme));
    lines
}

/// `● Running`, in the status's own hue.
fn status_chip(model: &InspectorModel, now: Timestamp, theme: &Theme) -> Vec<Span<'static>> {
    let word_style = status_style(model.status, theme);
    vec![
        status_glyph(model.status, theme, now),
        Span::styled(format!(" {}", status_word(model.status)), word_style),
    ]
}

/// `child of aether · turn 6 · 2m 40s`
fn lineage(model: &InspectorModel, width: usize, now: Timestamp, theme: &Theme) -> Line<'static> {
    let mut parts = Vec::new();
    if let Some(parent) = &model.parent_name {
        parts.push(format!("child of {parent}"));
    }
    if let Some(turn) = model.turn {
        parts.push(format!("turn {turn}"));
    }
    let elapsed = elapsed_text(model.status, model.elapsed_ms(now));
    if !elapsed.is_empty() {
        parts.push(elapsed);
    }
    let text = parts.join(&format!(" {} ", theme.glyphs().dot));
    Line::from(Span::styled(
        truncate(&text, width, theme),
        theme.secondary(),
    ))
}

/// The objective; for a failed agent, its error, quota and recovery hint instead.
fn summary_lines(model: &InspectorModel, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    match (&model.error, &model.objective) {
        (Some(error), _) => out.extend(wrapped(error, width, theme.failure(), theme)),
        (None, Some(objective)) => out.extend(wrapped(objective, width, theme.text(), theme)),
        (None, None) => {}
    }
    if let Some(quota) = &model.quota {
        out.push(Line::from(Span::styled(
            truncate(quota, width, theme),
            theme.failure(),
        )));
    }
    if model.status == BoardStatus::Failed {
        let hint = format!("/wake {} /restart", theme.glyphs().dot);
        out.push(Line::from(Span::styled(hint, theme.secondary())));
    }
    out
}

/// `◆ Asks: …` and how to answer it.
fn asks_lines(asks: &Asks, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let (text, hint) = match asks {
        Asks::Question(text) => (format!("Asks: {text}"), Some("view and reply")),
        Asks::Approval(wants) => (wants.clone(), None),
    };
    let marker = Span::styled(
        format!("{} ", theme.glyphs().needs_you),
        theme.attention().patch(theme.title()),
    );
    let mut out: Vec<Line<'static>> = wrapped(&text, width.saturating_sub(2), theme.text(), theme)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let lead = if i == 0 {
                marker.clone()
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![lead];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect();
    if let Some(hint) = hint {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("enter", theme.title()),
            Span::styled(format!(" {hint}"), theme.secondary()),
        ]));
    }
    out
}

/// `Context    ━━━━━━━━──────   41%` then `82k of 256k until compaction · window 1M`.
fn context_lines(model: &InspectorModel, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(used) = model.context_used else {
        return Vec::new();
    };
    let Some(threshold) = model.compaction_threshold.filter(|t| *t > 0) else {
        return field("Context", vec![tokens_short(used)], width, theme);
    };
    let percent = (used.saturating_mul(100) / threshold).min(100);
    let percent_style = if percent >= CONTEXT_WARN_PERCENT {
        theme.text().patch(theme.title())
    } else {
        theme.text()
    };
    let g = theme.glyphs();
    let bar_width = width.saturating_sub(usize::from(LABEL_WIDTH) + PERCENT_COLUMNS);
    let filled = (bar_width as u64 * percent / 100) as usize;
    let mut bar = vec![label("Context", theme)];
    if bar_width > 0 {
        bar.push(Span::styled(g.bar.repeat(filled), theme.accent()));
        // Unicode: one bar glyph, track by colour. ASCII `=` on `-` reads without colour.
        let track = if theme.ascii() { g.rule } else { g.bar };
        bar.push(Span::styled(
            track.repeat(bar_width - filled),
            theme.track(),
        ));
    }
    bar.push(Span::styled(format!("{percent:>5}%"), percent_style));
    let detail = context_detail(model, used, threshold, width, theme);
    vec![
        Line::from(fit_spans(bar, width, theme)),
        Line::from(fit_spans(
            vec![label("", theme), Span::styled(detail, theme.secondary())],
            width,
            theme,
        )),
    ]
}

/// The line under the context bar; the real window is added only when it fits.
fn context_detail(
    model: &InspectorModel,
    used: u64,
    threshold: u64,
    width: usize,
    theme: &Theme,
) -> String {
    let dot = theme.glyphs().dot;
    if used >= threshold {
        return format!("at threshold {dot} compacting");
    }
    let until = format!(
        "{} of {} until compaction",
        tokens_short(used),
        tokens_short(threshold)
    );
    let room = width.saturating_sub(usize::from(LABEL_WIDTH));
    match model.context_window.filter(|window| *window > threshold) {
        Some(window) => {
            let with_window = format!("{until} {dot} window {}", tokens_short(window));
            if with_window.width() <= room {
                with_window
            } else {
                until
            }
        }
        None => until,
    }
}

/// `32k in`, `380k cached`, `18.2k out`
fn tokens(model: &InspectorModel) -> Vec<String> {
    let mut parts = vec![format!("{} in", tokens_short(model.tokens_in))];
    if model.tokens_cached > 0 {
        parts.push(format!("{} cached", tokens_short(model.tokens_cached)));
    }
    parts.push(format!("{} out", tokens_short(model.tokens_out)));
    parts
}

/// `$0.38`, `$0.51 with children`; `None` when nothing was priced.
fn cost(model: &InspectorModel) -> Option<Vec<String>> {
    let with_children = model
        .cost_with_children
        .filter(|total| Some(*total) != model.cost)
        .map(|total| format!("{} with children", cost_text(Some(total))));
    let parts: Vec<String> = model
        .cost
        .map(|own| cost_text(Some(own)))
        .into_iter()
        .chain(with_children)
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// `~/src/aether` as `~/`, `src/`, `aether`: a path breaks only after a slash.
fn path_parts(path: &str) -> Vec<String> {
    path.split_inclusive('/').map(str::to_owned).collect()
}

/// `parts` joined by `joiner` in rows at most `width` wide, breaking between
/// parts; oversized parts split at grapheme boundaries (a wide glyph needs two columns).
fn pack(parts: &[String], joiner: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let (mut row, mut used) = (String::new(), 0);
    for part in parts {
        let part_width = part.width();
        if !row.is_empty()
            && used + joiner.width() + part_width > width
            && (part_width <= width || !joiner.is_empty())
        {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        if used > 0 {
            row.push_str(joiner);
            used += joiner.width();
        }
        for grapheme in part.graphemes(true) {
            let w = grapheme.width();
            if used > 0 && used + w > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push_str(grapheme);
            used += w;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// `◆ tester  verifier                41s  $0.09`
fn children_lines(
    model: &InspectorModel,
    width: usize,
    now: Timestamp,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let shown = &model.children[..model.children.len().min(CHILDREN_MAX_ROWS)];
    let costs: Vec<String> = shown.iter().map(|c| cost_text(c.cost)).collect();
    let cost_width = costs.iter().map(|c| c.width()).max().unwrap_or(0);
    let mut out: Vec<Line<'static>> = shown
        .iter()
        .zip(costs)
        .map(|(child, cost)| {
            let mut left = vec![
                status_glyph(child.status, theme, now),
                Span::raw(" "),
                Span::styled(child.name.clone(), theme.text()),
            ];
            if !child.role.is_empty() {
                left.push(Span::raw("  "));
                left.push(Span::styled(child.role.clone(), theme.secondary()));
            }
            let elapsed = elapsed_text(child.status, child.elapsed_ms(now));
            let right = right_columns(&elapsed, &cost, cost_width, theme);
            split_line(left, right, width, theme)
        })
        .collect();
    let hidden = model.children.len() - shown.len();
    if hidden > 0 {
        out.push(Line::from(Span::styled(
            format!("+{hidden} more"),
            theme.secondary(),
        )));
    }
    out
}

/// `name` beside the first row of its value; later rows continue under the value column.
fn field(name: &'static str, rows: Vec<String>, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let rows = if rows.is_empty() {
        vec![String::new()]
    } else {
        rows
    };
    rows.into_iter()
        .enumerate()
        .map(|(i, row)| {
            let name = if i == 0 { name } else { "" };
            let spans = vec![label(name, theme), Span::styled(row, theme.text())];
            Line::from(fit_spans(spans, width, theme))
        })
        .collect()
}

/// Dim and [`LABEL_WIDTH`] columns wide, so values line up.
fn label(name: &'static str, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!("{name:<width$}", width = usize::from(LABEL_WIDTH)),
        theme.secondary(),
    )
}

/// Word-wrapped to `width`, at most [`TEXT_MAX_ROWS`] rows, the last ending in
/// an ellipsis when text was cut.
fn wrapped(text: &str, width: usize, style: Style, theme: &Theme) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let rows = textwrap::wrap(text, width);
    let cut = rows.len() > TEXT_MAX_ROWS;
    rows.iter()
        .take(TEXT_MAX_ROWS)
        .enumerate()
        .map(|(i, row)| {
            let row = if cut && i + 1 == TEXT_MAX_ROWS {
                // Force the marker even when the row itself fits.
                let ellipsis = theme.glyphs().ellipsis;
                truncate(&format!("{row} {ellipsis}"), width, theme)
            } else {
                row.to_string()
            };
            Line::from(Span::styled(row, style))
        })
        .collect()
}

/// `950`, `18.2k`, `412k`, `1.2M`: one decimal only while it says something.
fn tokens_short(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let (scaled, unit) = if n < 999_500 {
        (n as f64 / 1_000.0, "k")
    } else {
        (n as f64 / 1_000_000.0, "M")
    };
    let text = if scaled < 100.0 {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.0}")
    };
    format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_long_parts_never_overflows_with_a_separator() {
        let rows = pack(&["12345".into(), "12345678901".into()], " · ", 6);
        assert!(rows.iter().all(|row| row.width() <= 6), "{rows:?}");
        assert_eq!(rows, ["12345", "123456", "78901"]);
    }

    #[test]
    fn wrapping_preserves_graphemes_and_unicode_display_width() {
        for value in ["👩‍💻abc", "🇩🇰abc", "e\u{301}界abc"] {
            let rows = pack(&[value.into()], "", 3);
            assert_eq!(rows.concat(), value);
            assert!(rows.iter().all(|row| row.width() <= 3), "{rows:?}");
            for row in &rows {
                assert!(!row.ends_with('\u{200d}'), "{rows:?}");
            }
        }
        assert_eq!(pack(&["👩‍💻abc".into()], "", 3), ["👩‍💻a", "bc"]);

        let path = format!("/{}", "界/e\u{301}/👩‍💻/".repeat(512));
        let theme = Theme::resolve(Default::default());
        for width in 0..=32 {
            let rows = pack(&path_parts(&path), "", width);
            assert!(rows.len() <= path.len());
            if width >= 2 {
                assert_eq!(rows.concat(), path);
            }
            for line in field("Workspace", rows, width + usize::from(LABEL_WIDTH), &theme) {
                assert!(line.width() <= width + usize::from(LABEL_WIDTH));
            }
        }
    }
}
