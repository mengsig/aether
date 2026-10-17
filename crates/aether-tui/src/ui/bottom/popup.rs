//! Lists that open above the composer without shifting the transcript:
//! `/` commands (ranked, fuzzy), `@` files + agents, Ctrl+R history. Owner: W4.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use aether_core::id::AgentId;

use crate::app::App;
use crate::commands::COMMANDS;
use crate::ui::theme::tokens::Theme;

pub const POPUP_MAX_ROWS: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionTab {
    Files,
    Agents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    Slash,
    Mention(MentionTab),
    /// Ctrl+R reverse history search; Esc restores `saved_draft`.
    History,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Popup {
    pub kind: PopupKind,
    pub query: String,
    pub cursor: usize,
    /// Draft before Ctrl+R, restored on Esc.
    pub saved_draft: Option<String>,
    pub saved_pastes: Vec<super::composer::PastedText>,
    /// Workspace paths for the `@` Files tab, relative and sorted; read once on open.
    pub files: Vec<String>,
}

impl Popup {
    pub fn new(kind: PopupKind) -> Self {
        Popup {
            kind,
            query: String::new(),
            cursor: 0,
            saved_draft: None,
            saved_pastes: Vec::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupItem {
    Mode {
        name: String,
        about: String,
    },
    Command {
        name: &'static str,
        usage: &'static str,
        about: &'static str,
    },
    /// A command the host behind the agent runs; `about` ends with its marker.
    HostCommand {
        name: String,
        usage: String,
        about: String,
    },
    File {
        path: String,
    },
    Agent {
        id: AgentId,
        name: String,
        role: String,
    },
    History {
        text: String,
    },
}

/// How well a candidate matches a query; lower ranks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    Exact,
    Prefix,
    Substring,
    /// The query's chars appear in order, with gaps.
    Fuzzy,
}

/// Case-insensitive match of `query` against `candidate`; `None` when the
/// query's chars do not all appear in order. An empty query matches as a prefix.
pub fn fuzzy_match(candidate: &str, query: &str) -> Option<Match> {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    if candidate == query {
        return Some(Match::Exact);
    }
    if candidate.starts_with(&query) {
        return Some(Match::Prefix);
    }
    if candidate.contains(&query) {
        return Some(Match::Substring);
    }
    let mut rest = candidate.chars();
    query
        .chars()
        .all(|q| rest.any(|c| c == q))
        .then_some(Match::Fuzzy)
}

/// Keep the candidates that match, best class first; ties keep input order,
/// so the registry's rank decides among equally good matches.
fn ranked<T>(candidates: impl IntoIterator<Item = (T, Option<Match>)>) -> Vec<T> {
    let mut matched: Vec<(Match, T)> = candidates
        .into_iter()
        .filter_map(|(item, m)| m.map(|m| (m, item)))
        .collect();
    matched.sort_by_key(|(m, _)| *m);
    matched.into_iter().map(|(_, item)| item).collect()
}

/// Ranked, filtered items for the popup's current query.
pub fn items(popup: &Popup, app: &App) -> Vec<PopupItem> {
    let query = popup.query.as_str();
    match popup.kind {
        PopupKind::Slash => ranked(
            COMMANDS
                .iter()
                .map(|spec| {
                    let item = PopupItem::Command {
                        name: spec.name,
                        usage: spec.usage,
                        about: spec.about,
                    };
                    // A typed alias (`/cd`) leads with its command, not a fuzzy rival.
                    let alias = spec
                        .hidden_aliases
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(query))
                        .then_some(Match::Exact);
                    (item, alias.or_else(|| fuzzy_match(spec.name, query)))
                })
                .chain(host_items(app, query))
                .chain(app.view_modes.modes.values().map(|mode| {
                    (
                        PopupItem::Mode {
                            name: mode.name.clone(),
                            about: format!(
                                "{}{}",
                                if mode.name == app.view_mode {
                                    format!("selected {} ", app.theme.glyphs().dot)
                                } else {
                                    String::new()
                                },
                                mode.definition
                                    .description
                                    .as_deref()
                                    .unwrap_or("view mode")
                            ),
                        },
                        fuzzy_match(&mode.name, query),
                    )
                })),
        ),
        PopupKind::Mention(MentionTab::Files) => ranked(popup.files.iter().map(|path| {
            // A path-only hit ranks no better than a substring, so file names lead.
            let name = path.rsplit('/').next().unwrap_or(path);
            let in_path = fuzzy_match(path, query).map(|m| m.max(Match::Substring));
            let m = match (fuzzy_match(name, query), in_path) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            (PopupItem::File { path: path.clone() }, m)
        })),
        PopupKind::Mention(MentionTab::Agents) => {
            ranked(mention_agents(app).into_iter().map(|(id, name, role)| {
                let m = fuzzy_match(&name, query);
                (PopupItem::Agent { id, name, role }, m)
            }))
        }
        PopupKind::History => history_matches(app, query)
            .into_iter()
            .map(|text| PopupItem::History { text })
            .collect(),
    }
}

/// The host's commands after the built-ins, each marked with the host's name.
fn host_items<'a>(
    app: &'a App,
    query: &'a str,
) -> impl Iterator<Item = (PopupItem, Option<Match>)> + 'a {
    let host = &app.options.host_commands;
    let dot = app.theme.glyphs().dot;
    host.commands.iter().map(move |command| {
        let item = PopupItem::HostCommand {
            name: command.name.clone(),
            usage: command.args.clone(),
            about: format!("{} {dot} {}", command.about, host.host)
                .trim_start()
                .to_owned(),
        };
        (item, fuzzy_match(&command.name, query))
    })
}

/// Agents other than the viewed one: its children first, then the rest in
/// tree order. Archived agents only when the board shows them.
fn mention_agents(app: &App) -> Vec<(AgentId, String, String)> {
    let mut names = crate::app::sibling_names(&app.tree);
    let viewed = app.selected;
    let children = viewed
        .and_then(|id| app.tree.nodes.get(&id))
        .map(|node| node.children.clone())
        .unwrap_or_default();
    let others = app
        .tree
        .nodes
        .keys()
        .copied()
        .filter(|id| !children.contains(id));
    children
        .iter()
        .copied()
        .chain(others)
        .filter(|id| Some(*id) != viewed)
        .filter_map(|id| app.tree.nodes.get(&id))
        .filter(|node| app.show_archived || !node.agent.archived)
        .map(|node| {
            let agent = &node.agent;
            let name = names
                .remove(&agent.id)
                .unwrap_or_else(|| agent.name.clone());
            (agent.id, name, agent.role.to_string())
        })
        .collect()
}

/// History entries containing `query` (case-insensitive), newest first, without repeats.
pub fn history_matches(app: &App, query: &str) -> Vec<String> {
    let needle = query.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    app.input
        .history()
        .iter()
        .rev()
        .filter(|text| crate::format::contains_fold(text, &needle))
        .filter(|text| seen.insert(text.as_str()))
        .cloned()
        .collect()
}

/// The popup's rect: `rows` directly above `composer`, clipped to the screen.
pub fn area_above(composer: Rect, rows: u16) -> Rect {
    let rows = rows.min(composer.y);
    Rect {
        y: composer.y - rows,
        height: rows,
        ..composer
    }
}

/// Header row, list rows (at least one, for "no matches"), blank, hints, blank.
pub fn desired_height(popup: &Popup, app: &App) -> u16 {
    let rows = u16::try_from(items(popup, app).len()).unwrap_or(u16::MAX);
    rows.clamp(1, POPUP_MAX_ROWS) + 4
}

const INDENT: &str = "  ";
/// Gap between the name column and the description.
const COLUMN_GAP: usize = 2;

pub fn render(frame: &mut Frame, composer: Rect, popup: &Popup, app: &App, theme: &Theme) -> Rect {
    let items = items(popup, app);
    let area = area_above(composer, desired_height(popup, app));
    if area.is_empty() {
        return area;
    }
    let width = usize::from(area.width);
    let mut lines = vec![header(popup, theme)];
    if items.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled("no matches", theme.secondary()),
        ]));
    }
    let shown = usize::from(POPUP_MAX_ROWS);
    let top = popup.cursor.saturating_sub(shown - 1);
    let name_width = items
        .iter()
        .skip(top)
        .take(shown)
        .map(|item| label(item, theme.glyphs().ellipsis).width())
        .max()
        .unwrap_or(0);
    for (i, item) in items.iter().enumerate().skip(top).take(shown) {
        lines.push(item_line(item, i == popup.cursor, name_width, width, theme));
    }
    lines.push(more_line(items.len().saturating_sub(top + shown), theme));
    let history_key = app
        .keys
        .display(crate::keys::KeyAction::History, theme.ascii());
    let pairs = popup_hints(popup.kind, theme.ascii());
    let pairs: Vec<_> = pairs
        .into_iter()
        .map(|(key, label)| {
            (
                if popup.kind == PopupKind::History && label == "older" {
                    history_key.as_str()
                } else {
                    key
                },
                label,
            )
        })
        .collect();
    lines.push(hints_line(&pairs, theme));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines), area);
    area
}

/// `↓ 21 more · type to filter` under a clipped list, else the blank spacer row.
fn more_line(hidden: usize, theme: &Theme) -> Line<'static> {
    if hidden == 0 {
        return Line::default();
    }
    let (arrow, dot) = if theme.ascii() {
        ("v", "|")
    } else {
        ("↓", "·")
    };
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            format!("{arrow} {hidden} more {dot} type to filter"),
            theme.secondary(),
        ),
    ])
}

/// Blank for `/`; the tabs for `@`; the query for Ctrl+R.
fn header(popup: &Popup, theme: &Theme) -> Line<'static> {
    match popup.kind {
        PopupKind::Slash => Line::default(),
        PopupKind::Mention(active) => {
            let tab = |tab: MentionTab, label: &'static str| {
                let style = if tab == active {
                    theme.accent().patch(theme.title())
                } else {
                    theme.secondary()
                };
                Span::styled(label, style)
            };
            Line::from(vec![
                Span::raw(INDENT),
                tab(MentionTab::Files, "Files"),
                Span::styled(format!(" {} ", theme.glyphs().dot), theme.secondary()),
                tab(MentionTab::Agents, "Agents"),
            ])
        }
        PopupKind::History => Line::from(vec![
            Span::raw(INDENT),
            Span::styled("search history ", theme.secondary()),
            Span::styled(popup.query.clone(), theme.text()),
            Span::styled(theme.glyphs().caret, theme.accent()),
        ]),
    }
}

/// The left column: what Enter/Tab inserts, and a host command's argument hint.
fn label(item: &PopupItem, ellipsis: &str) -> String {
    match item {
        PopupItem::Mode { name, .. } => format!("/{name}"),
        PopupItem::Command { name, .. } => format!("/{name}"),
        // The host's argument hint is the only place it is shown.
        PopupItem::HostCommand { name, usage, .. } if !usage.is_empty() => {
            format!("/{name} {usage}")
        }
        PopupItem::HostCommand { name, .. } => format!("/{name}"),
        PopupItem::File { path } => path.clone(),
        PopupItem::Agent { name, .. } => format!("@{name}"),
        PopupItem::History { text } => first_line(text, ellipsis),
    }
}

fn description(item: &PopupItem) -> String {
    match item {
        PopupItem::Mode { about, .. } => about.clone(),
        PopupItem::Command { about, .. } => (*about).to_owned(),
        PopupItem::HostCommand { about, .. } => about.clone(),
        PopupItem::Agent { role, .. } => role.clone(),
        PopupItem::File { .. } | PopupItem::History { .. } => String::new(),
    }
}

fn first_line(text: &str, ellipsis: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().to_owned();
    match lines.next() {
        Some(_) => format!("{first} {ellipsis}"),
        None => first,
    }
}

fn item_line(
    item: &PopupItem,
    selected: bool,
    name_width: usize,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let label = label(item, theme.glyphs().ellipsis);
    let room = width.saturating_sub(INDENT.len());
    let name_width = name_width.min(room);
    let marker = theme.glyphs().ellipsis.chars().next().unwrap_or('~');
    let name = crate::format::truncate_with_marker(&label, name_width, marker);
    let pad = name_width.saturating_sub(name.width()) + COLUMN_GAP;
    let about_room = room.saturating_sub(name_width + COLUMN_GAP);
    let about = fit_words(&description(item), about_room, theme.glyphs().ellipsis);
    let name_style = if selected {
        theme.accent()
    } else {
        theme.text()
    };
    let used = INDENT.len() + name.width() + pad + about.width();
    let line = Line::from(vec![
        Span::raw(INDENT),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(about, theme.secondary()),
        // Fill so the selected row's highlight spans the popup.
        Span::raw(" ".repeat(width.saturating_sub(used))),
    ]);
    if selected {
        line.style(theme.selected())
    } else {
        line
    }
}

pub(crate) fn popup_hints(kind: PopupKind, ascii: bool) -> Vec<(&'static str, &'static str)> {
    let (up_down, left_right) = if ascii {
        ("^v", "<>")
    } else {
        ("↑↓", "←→")
    };
    match kind {
        PopupKind::Slash => vec![
            ("enter", "run"),
            ("tab", "complete"),
            (up_down, "select"),
            ("esc", "close"),
        ],
        PopupKind::Mention(_) => vec![
            ("enter", "insert"),
            (left_right, "files/agents"),
            (up_down, "select"),
            ("esc", "close"),
        ],
        PopupKind::History => vec![("enter", "accept"), ("ctrl+r", "older"), ("esc", "cancel")],
    }
}

/// `key desc · key desc`, keys bold and descriptions dim.
pub(crate) fn hints_line(pairs: &[(&str, &str)], theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(INDENT)];
    for (i, (key, desc)) in pairs.iter().enumerate() {
        if i > 0 {
            let sep = format!(" {} ", theme.glyphs().dot);
            spans.push(Span::styled(sep, theme.secondary()));
        }
        spans.push(Span::styled((*key).to_owned(), theme.title()));
        spans.push(Span::styled(format!(" {desc}"), theme.secondary()));
    }
    Line::from(spans)
}

/// `text` within `width` columns, cut only at a word boundary and marked with
/// `ellipsis`. Empty when not even the first word fits.
pub(crate) fn fit_words(text: &str, width: usize, ellipsis: &str) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let room = width.saturating_sub(ellipsis.width());
    let fitted = text
        .char_indices()
        .filter(|(_, c)| c.is_whitespace())
        .map(|(i, _)| text[..i].trim_end())
        .take_while(|head| head.width() <= room)
        .last()
        .unwrap_or_default();
    if !fitted.is_empty() {
        return format!("{fitted}{ellipsis}");
    }
    // One word wider than the room: cut inside it rather than drop the text.
    let mut head = String::new();
    for c in text.chars() {
        if head.width() + c.to_string().width() > room {
            break;
        }
        head.push(c);
    }
    if head.trim().is_empty() {
        return String::new();
    }
    format!("{}{ellipsis}", head.trim_end())
}

#[cfg(test)]
mod tests;
