//! One physical row for routine coordination; source bodies stay in the transcript.
use crate::app::transcript::{CallStatus, CallTarget, CellModel, ChildOutcome, UserFrom};
use crate::ui::theme::Theme;
use ratatui::text::Line;
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn line(model: &CellModel, width: u16, theme: &Theme) -> Option<Line<'static>> {
    let mut parts: Vec<&str> = Vec::new();
    let notice_text;
    match model {
        CellModel::Spawned {
            name, objective, ..
        } => parts.extend(["Delegated to ", name, ": ", objective]),
        CellModel::Sent { to, text } => parts.extend(["To ", to, ": ", text]),
        CellModel::User {
            from: UserFrom::Message { from },
            text,
        } => parts.extend(["From ", from, ": ", text]),
        CellModel::User {
            from: UserFrom::Task { from },
            text,
        } => parts.extend(["Task from ", from, ": ", text]),
        CellModel::ChildResult {
            name,
            outcome: ChildOutcome::Finished,
            result,
            ..
        } => parts.extend([name, " finished: ", result]),
        CellModel::Waiting { on, ended, .. } => {
            parts.push(if ended.is_some() {
                "Waited for "
            } else {
                "Waiting for "
            });
            for (index, (_, name)) in on.iter().enumerate().take(usize::from(width)) {
                if index > 0 {
                    parts.push(", ");
                }
                parts.push(name);
            }
        }
        CellModel::Notice { text, .. } => {
            notice_text = if theme.ascii() {
                text.ascii(theme.glyphs().dot)
            } else {
                Cow::Borrowed(text.as_str())
            };
            parts.push(notice_text.as_ref());
        }
        _ => parts = failed_call(model)?,
    }
    let prefix = format!("{} ", theme.glyphs().bullet);
    let text = clipped(
        std::iter::once(prefix.as_str()).chain(parts),
        usize::from(width),
        theme.glyphs().ellipsis,
    );
    Some(Line::styled(text, theme.text()))
}

/// `label failed: first error line`: the call's input and its later output stay out.
fn failed_call(model: &CellModel) -> Option<Vec<&str>> {
    let (mut parts, error) = match model {
        CellModel::Exec {
            status: CallStatus::Failed,
            output,
            ..
        } => (vec!["Command"], output.as_str()),
        CellModel::Called {
            target,
            status: CallStatus::Failed,
            error,
            ..
        } => {
            let label = match target {
                CallTarget::Mcp { server, tool } => vec![server.as_str(), ".", tool],
                CallTarget::WebSearch { .. } => vec!["Web search"],
                CallTarget::WebFetch { .. } => vec!["Web fetch"],
                CallTarget::Tool { name } => vec![name.as_str()],
            };
            (label, error.as_deref().unwrap_or_default())
        }
        _ => return None,
    };
    parts.push(" failed");
    if let Some(first) = error.lines().map(str::trim).find(|line| !line.is_empty()) {
        parts.extend([": ", first]);
    }
    Some(parts)
}

fn clipped<'a>(parts: impl Iterator<Item = &'a str>, width: usize, marker: &str) -> String {
    let mut out = String::new();
    let mut used = 0;
    for part in parts {
        // Bound sanitizing and segmentation, including escape payloads and combining clusters.
        let mut end = part.len().min(width.saturating_add(1).saturating_mul(64));
        while !part.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = end < part.len();
        let clean_part = super::text::clean(&part[..end]);
        for (at, grapheme) in clean_part.grapheme_indices(true) {
            if truncated && at + grapheme.len() == clean_part.len() {
                return ellipsis(out, used, width, marker);
            }
            let clean = if grapheme.chars().any(char::is_control) {
                " "
            } else {
                grapheme
            };
            let size = clean.width();
            if used + size > width {
                return ellipsis(out, used, width, marker);
            }
            out.push_str(clean);
            used += size;
        }
        if truncated {
            return ellipsis(out, used, width, marker);
        }
    }
    out
}

fn ellipsis(mut out: String, mut used: usize, width: usize, marker: &str) -> String {
    while used + marker.width() > width && !out.is_empty() {
        let Some((last, grapheme)) = out.grapheme_indices(true).next_back() else {
            break;
        };
        used -= grapheme.width();
        out.truncate(last);
    }
    if marker.width() <= width {
        out.push_str(marker);
    }
    out
}
