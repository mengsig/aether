//! Hint row (SPEC §1.1 item 6): contextual hints left (hidden while typing,
//! replaced by a live flash or `● Listening` while dictating), `$run · NN% until compaction` right, quota chip. Owner: W1.

use aether_core::model::Usage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::format;
use crate::keys::KeyAction;
use crate::ui::theme::tokens::{Glyphs, Theme};

/// Lines the bottom pane up with the transcript gutter.
const INDENT: &str = "  ";
/// Right margin after the right-hand side, as in the mocks.
const MARGIN: &str = " ";

/// Left-side hints for the current focus/view state, e.g. `? shortcuts · ctrl+g agents`.
/// The flash and the typing rule are the renderer's; this is the resting text.
pub fn hints(app: &App) -> String {
    hint_text(
        app,
        app.theme.glyphs(),
        app.board.open && app.size.0 < crate::ui::layout::BOARD_DOCK_MIN_COLS,
        match app.board_mode() {
            crate::ui::layout::BoardMode::Docked { width } => app.size.0.saturating_sub(width + 2),
            _ => app.size.0,
        },
    )
}

/// Board lifecycle keys (after Shift) and what each does, as the footer names them.
const BOARD_ACTIONS: [(&str, &str); 4] = [
    ("x", "stop"),
    ("d", "remove"),
    ("p", "pause"),
    ("i", "interrupt"),
];

fn hint_text(app: &App, glyphs: &Glyphs, overlay: bool, width: u16) -> String {
    let ascii = glyphs.dot.is_ascii();
    let separator = format!(" {} ", glyphs.dot);
    if app.board.focused() {
        let (updown, leftright, shift) = if ascii {
            ("up/down", "left/right", "shift+")
        } else {
            ("↑↓", "←→", "⇧")
        };
        let room = usize::from(width).saturating_sub(INDENT.width() + MARGIN.width());
        let fits = |keys: &[String]| keys.join(&separator).width() <= room;
        let mut keys = vec![
            updown.to_owned(),
            "enter view".to_owned(),
            if ascii { "esc" } else { "esc back" }.to_owned(),
        ];
        keys.extend(BOARD_ACTIONS.map(|(key, action)| format!("{shift}{key} {action}")));
        // Every action stays in view: group them, then drop the arrows, before any is cut.
        if !fits(&keys) {
            keys = vec![
                updown.to_owned(),
                board_essentials(glyphs, app.quota_alert.is_some(), room),
            ];
            if !fits(&keys) {
                keys.remove(0);
            }
        }
        if !overlay {
            keys.extend([
                format!("{leftright} fold"),
                format!("{} hide", app.keys.display(KeyAction::Board, ascii)),
            ]);
        }
        return keys.join(&separator);
    }
    // An open view owns the keys, so global hints would mislead.
    if let Some(view) = app.bottom.top_view() {
        return if ascii {
            ascii_hints(view.hints())
        } else {
            view.hints().to_owned()
        };
    }
    if app.reading_history() {
        return format!(
            "older history{separator}{} latest",
            app.keys.display(KeyAction::Latest, ascii)
        );
    }
    if app.earlier_history_available() {
        return format!("earlier history trimmed{separator}scroll up to load");
    }
    let mut parts = vec![
        format!(
            "{} shortcuts",
            app.keys.display(KeyAction::Shortcuts, ascii)
        ),
        format!("{} agents", app.keys.display(KeyAction::Agents, ascii)),
    ];
    let parent = app
        .selected_agent()
        .and_then(|agent| agent.parent)
        .and_then(|id| app.tree.nodes.get(&id));
    if let Some(parent) = parent {
        let key = app.keys.display(KeyAction::Parent, ascii);
        parts.push(format!(
            "{key} back to {}",
            app.agent_display_name(parent.agent.id)
        ));
    }
    parts.join(&separator)
}

/// The narrowest focused-board hints: Enter, Esc and every lifecycle action, grouped.
fn board_essentials(glyphs: &Glyphs, quota: bool, room: usize) -> String {
    let shift = if glyphs.dot.is_ascii() {
        "shift+"
    } else {
        "⇧"
    };
    let (letters, actions): (Vec<_>, Vec<_>) = BOARD_ACTIONS.into_iter().unzip();
    let grouped = format!("{shift}{} {}", letters.join("/"), actions.join("/"));
    let essentials = ["enter", "esc", &grouped].join(&format!(" {} ", glyphs.dot));
    if quota && essentials.width() + " quota".width() > room {
        format!("enter/esc {grouped}")
    } else {
        essentials
    }
}

/// A view's hints with the arrows spelled out, for `--ascii`.
fn ascii_hints(hints: &str) -> String {
    hints
        .replace("↑↓", "up/down")
        .replace("←→", "left/right")
        .replace('·', "-")
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    reading: bool,
    overlay: bool,
) {
    if area.is_empty() {
        return;
    }
    let line = footer_line(app, theme, area.width, reading, overlay);
    frame
        .buffer_mut()
        .set_line(area.x, area.y, &line, area.width);
}

fn footer_line(
    app: &App,
    theme: &Theme,
    width: u16,
    reading: bool,
    overlay: bool,
) -> Line<'static> {
    let glyphs = theme.glyphs();
    if let Some(disk) = app
        .status
        .as_ref()
        .and_then(|status| status.low_disk.as_ref())
    {
        let prefix = format!("{INDENT}{} ", glyphs.needs_you);
        let room = usize::from(width).saturating_sub(prefix.width());
        return Line::from(vec![
            Span::styled(prefix, theme.accent()),
            Span::styled(
                truncate(&format::low_disk(disk), room, glyphs),
                theme.title(),
            ),
        ]);
    }
    let separator = format!(" {} ", glyphs.dot);
    let room = usize::from(width).saturating_sub(INDENT.width() + MARGIN.width());
    let mut hints = hint_text(app, glyphs, overlay, width);
    let right_room = match app.quota_alert {
        Some(_) if app.board.focused() => {
            room.saturating_sub(board_essentials(glyphs, true, room).width() + 1)
        }
        Some(_) => room,
        None if overlay => room.saturating_sub(hints.width() + 1),
        // The lifecycle keys outrank usage; usage outranks the optional board hints.
        None if app.board.focused() => room
            .saturating_sub(board_essentials(glyphs, app.quota_alert.is_some(), room).width() + 1),
        None => room,
    };
    let right = right_side(app, theme, &separator, right_room, reading, overlay);
    let right_width: usize = right.iter().map(Span::width).sum();
    // One column always separates the two sides.
    let left_room = room.saturating_sub(right_width + usize::from(right_width > 0));

    if app.board.focused() && (!overlay || app.quota_alert.is_some()) {
        let hint_width = left_room + INDENT.width() + MARGIN.width();
        hints = hint_text(
            app,
            glyphs,
            overlay,
            u16::try_from(hint_width).unwrap_or(u16::MAX),
        );
    }

    // Dictation runs while the person types, so it shows whatever else does.
    let recording = app.voice.recording();
    let listening = format!("{} Listening{separator}", glyphs.running);
    let left_room = if recording {
        left_room.saturating_sub(listening.width())
    } else {
        left_room
    };

    let mut left = if let Some(flash) = app.live_flash() {
        vec![Span::styled(
            truncate(flash, left_room, glyphs),
            theme.text(),
        )]
    } else if !app.input.text.is_empty() && !app.board.focused() {
        // Typing hides hints; a focused board keeps its keys in view (§2 principle 5).
        Vec::new()
    } else {
        hint_spans(&hints, &separator, left_room, theme)
    };
    if recording {
        let mut prefix = vec![
            Span::styled(format!("{} ", glyphs.running), theme.accent()),
            Span::styled("Listening", theme.secondary()),
        ];
        if !left.is_empty() {
            prefix.push(Span::styled(separator.clone(), theme.secondary()));
        }
        left.splice(0..0, prefix);
    }
    let left_width: usize = left.iter().map(Span::width).sum();

    let mut spans = vec![Span::raw(INDENT)];
    spans.extend(left);
    let gap = room.saturating_sub(left_width + right_width);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    spans.push(Span::raw(MARGIN));
    Line::from(spans)
}

/// Whole hint segments that fit in `room`, each as a bold key and dim words.
fn hint_spans(text: &str, separator: &str, room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let mut segments: Vec<&str> = text.split(separator).filter(|s| !s.is_empty()).collect();
    let joined = |segments: &[&str]| segments.join(separator);
    while segments.len() > 1 && joined(&segments).width() > room {
        segments.pop();
    }
    let Some(first) = segments.first() else {
        return Vec::new();
    };
    if first.width() > room {
        return vec![Span::styled(
            truncate(first, room, theme.glyphs()),
            theme.secondary(),
        )];
    }
    let mut spans = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(separator.to_owned(), theme.secondary()));
        }
        let (key, words) = segment.split_once(' ').unwrap_or((segment, ""));
        spans.push(Span::styled(key.to_owned(), theme.title()));
        if !words.is_empty() {
            spans.push(Span::styled(format!(" {words}"), theme.secondary()));
        }
    }
    spans
}

/// `[quota chip · ]$run · NN% until compaction[ · notifications]`: the notification
/// count keeps the bottom-right corner and is dropped only when nothing else fits.
fn right_side(
    app: &App,
    theme: &Theme,
    separator: &str,
    room: usize,
    reading: bool,
    overlay: bool,
) -> Vec<Span<'static>> {
    let Some((text, style)) = notification_chip(app, theme, room) else {
        return usage_side(app, theme, separator, room, reading, overlay);
    };
    let rest = room.saturating_sub(text.width() + separator.width());
    let mut spans = usage_side(app, theme, separator, rest, reading, overlay);
    if !spans.is_empty() {
        spans.push(Span::styled(separator.to_owned(), theme.secondary()));
    }
    spans.push(Span::styled(text, style));
    spans
}

/// `◆ 2 waiting · 5 unread` when questions or blockers wait on the operator,
/// in the attention colour; `5 unread` otherwise; `◆ 5` or `5` when narrow.
fn notification_chip(app: &App, theme: &Theme, room: usize) -> Option<(String, Style)> {
    let unread = app.notifications.unread();
    if unread == 0 {
        return None;
    }
    let urgent = app.notifications.urgent_unread();
    debug_assert!(urgent <= unread, "urgent notifications are unread ones");
    let mark = theme.glyphs().needs_you;
    let dot = theme.glyphs().dot;
    let (full, short, style) = if urgent > 0 {
        (
            format!("{mark} {urgent} waiting {dot} {unread} unread"),
            format!("{mark} {unread}"),
            theme.attention().patch(theme.title()),
        )
    } else {
        (
            format!("{unread} unread"),
            unread.to_string(),
            theme.secondary(),
        )
    };
    [full, short]
        .into_iter()
        .find(|text| text.width() <= room)
        .map(|text| (text, style))
}

/// `[quota chip · ]$run · NN% until compaction`: context shortens to `NN%`, then cost
/// outranks it, then `NN%` alone; the chip is cut last: a quota failure is never silent.
fn usage_side(
    app: &App,
    theme: &Theme,
    separator: &str,
    room: usize,
    reading: bool,
    overlay: bool,
) -> Vec<Span<'static>> {
    let mut items: Vec<(String, Style)> = Vec::new();
    if let Some(alert) = &app.quota_alert {
        let alert = if app.board.focused() && alert.width() > room {
            "quota".to_owned()
        } else {
            alert.clone()
        };
        items.push((alert, theme.failure()));
    }
    if overlay {
        if let Some(agent) = app.selected {
            items.push((
                format!("viewing {}", app.agent_display_name(agent)),
                theme.secondary(),
            ));
        }
    } else {
        if reading {
            items.push((
                format!(
                    "{} reading{}{} to follow",
                    if theme.ascii() { "up" } else { "↑" },
                    separator,
                    app.keys.display(KeyAction::Latest, theme.ascii())
                ),
                theme.secondary(),
            ));
        }
        if app.details {
            items.push((
                format!(
                    "details{separator}{} hide",
                    app.keys.display(KeyAction::Details, theme.ascii())
                ),
                theme.secondary(),
            ));
        }
    }
    let agent = app.selected_agent();
    let cost = agent
        .and_then(|a| app.tree.nodes.get(&a.id))
        .and_then(|node| run_cost(&node.subtree_usage));
    let left = agent.and_then(|a| {
        let used = *app.context_tokens.get(&a.id)?;
        let threshold = app.compaction_threshold(&a.model).filter(|t| *t > 0)?;
        let used_pct = (used
            .min(threshold)
            .saturating_mul(100)
            .saturating_add(threshold / 2))
            / threshold;
        Some(100u64.saturating_sub(used_pct))
    });
    let short = left.map(|left| format!("{left}%"));
    let full = left.map(|left| format!("{left}% until compaction"));
    let ladder = [
        [cost.as_deref(), full.as_deref()],
        [cost.as_deref(), short.as_deref()],
        [cost.as_deref(), None],
        [None, short.as_deref()],
    ];
    let width = |items: &[(String, Style)]| {
        let text: usize = items.iter().map(|(t, _)| t.width()).sum();
        text + separator.width() * items.len().saturating_sub(1)
    };
    let labels_width = width(&items);
    if let Some(figures) = ladder.into_iter().find(|rung| {
        let mut total = labels_width;
        for (index, figure) in rung.iter().flatten().enumerate() {
            total += figure.width();
            if !items.is_empty() || index > 0 {
                total += separator.width();
            }
        }
        total <= room
    }) {
        items.extend(
            figures
                .into_iter()
                .flatten()
                .map(|text| (text.to_owned(), theme.secondary())),
        );
    }
    // Only the chip or a label is ever cut; cost and context drop whole.
    while width(&items) > room && items.len() > 1 {
        items.pop();
    }
    // A lone ellipsis says nothing.
    if room < 2 && width(&items) > room {
        items.clear();
    }
    if let [(text, _)] = items.as_mut_slice() {
        *text = truncate(text, room, theme.glyphs());
    }

    let mut spans = Vec::new();
    for (i, (text, style)) in items.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(separator.to_owned(), theme.secondary()));
        }
        spans.push(Span::styled(text, style));
    }
    spans
}

/// The subtree's spend: `~$` when estimated or a floor, nothing when unknown.
fn run_cost(usage: &Usage) -> Option<String> {
    let label = |amount| format::cost_with_basis(amount, usage.cost_basis);
    if let Some(total) = usage.api_equivalent_usd().filter(|t| *t > 0.0) {
        return Some(label(format!("~{}", dollars(total))));
    }
    if usage.cost_complete {
        let prefix = if usage.cost_basis == aether_core::model::CostBasis::ListPrice {
            "~"
        } else {
            ""
        };
        return Some(label(format!("{prefix}{}", dollars(usage.cost_usd))));
    }
    (usage.cost_usd > 0.0).then(|| label(format!("~{}", dollars(usage.cost_usd))))
}

fn dollars(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        return "<$0.01".to_owned();
    }
    format!("${usd:.2}")
}

fn truncate(text: &str, room: usize, glyphs: &Glyphs) -> String {
    let marker = glyphs.ellipsis.chars().next().unwrap_or('~');
    format::truncate_with_marker(text, room, marker)
}

#[cfg(test)]
mod tests {
    use aether_core::id::AgentId;
    use aether_protocol::ModelInfo;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use super::*;
    use crate::app::board::BoardFocus;
    use crate::ui::bottom::status_line::tests::{app_viewing_child, dark_theme, shows};
    use crate::ui::tests_support::screen_text;

    fn viewed(app: &App) -> AgentId {
        app.selected.expect("a viewed agent")
    }

    /// The viewed child spent $0.51 with children and used 32% of a 256k compaction threshold.
    fn busy_child() -> App {
        let mut app = app_viewing_child();
        let id = viewed(&app);
        let node = app.tree.nodes.get_mut(&id).expect("node");
        node.subtree_usage.cost_usd = 0.51;
        let model = node.agent.model.clone();
        app.context_tokens.insert(id, 82_000);
        app.models.push(ModelInfo {
            tier: None,
            aliases: Vec::new(),
            model,
            name: "Sonnet 5.5".into(),
            provider_name: "Anthropic".into(),
            capabilities: aether_core::model::CapabilityProfile {
                context_window: 1_000_000,
                compaction_tokens: Some(256_000),
                ..Default::default()
            },
            configured: true,
            auth_source: None,
            is_default: false,
        });
        app
    }

    fn draw(app: &App, theme: &Theme, width: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).expect("terminal");
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    app,
                    theme,
                    false,
                    crate::ui::layout::board_mode(width, app.board.open)
                        == crate::ui::layout::BoardMode::Overlay,
                )
            })
            .expect("draw");
        terminal
    }

    fn row(app: &App, theme: &Theme, width: u16) -> String {
        let text = screen_text(&draw(app, theme, width));
        assert_eq!(text.width(), usize::from(width), "{text:?}");
        text
    }

    /// `left`, padding, `right` and the margin, exactly `width` wide.
    fn spread(left: &str, right: &str, width: usize) -> String {
        let gap = width - 2 - left.width() - right.width() - 1;
        format!("  {left}{}{right} ", " ".repeat(gap))
    }

    fn footer_with_profile(window: u32, compaction_tokens: Option<u32>, used: u64) -> String {
        let mut app = busy_child();
        let capabilities = &mut app.models.last_mut().expect("catalogue entry").capabilities;
        capabilities.context_window = window;
        capabilities.compaction_tokens = compaction_tokens;
        app.context_tokens.insert(viewed(&app), used);
        row(&app, &dark_theme(false), 120)
    }

    #[test]
    fn the_footer_counts_to_the_compaction_threshold_not_the_window() {
        let claude = footer_with_profile(1_000_000, Some(256_000), 41_300);
        assert!(claude.contains("84% until compaction"), "{claude}");
        let codex = footer_with_profile(272_000, Some(272_000), 68_000);
        assert!(codex.contains("75% until compaction"), "{codex}");
        let configured = footer_with_profile(1_000_000, Some(100_000), 25_000);
        assert!(configured.contains("75% until compaction"), "{configured}");
        let proportional = footer_with_profile(400_000, None, 150_000);
        assert!(
            proportional.contains("50% until compaction"),
            "{proportional}"
        );
    }

    #[test]
    fn extreme_context_usage_shows_nothing_until_compaction() {
        let mut app = busy_child();
        app.context_tokens.insert(viewed(&app), u64::MAX);
        assert!(row(&app, &dark_theme(false), 120).contains("0% until compaction"));
    }

    #[test]
    fn focused_board_keeps_actions_and_whole_figures_with_quota_at_breakpoints() {
        use crate::ui::theme::tokens::{ThemeMode, ThemeSpec};
        for screen_width in [59, 60, 99, 100, 160] {
            for ascii in [false, true] {
                for mode in [ThemeMode::Dark, ThemeMode::Light, ThemeMode::Mono] {
                    for quota in [false, true] {
                        for running in [false, true] {
                            let mut app = busy_child();
                            app.board.open = true;
                            app.board.focus = BoardFocus::Board;
                            app.size = (screen_width, 40);
                            let id = viewed(&app);
                            app.tree.nodes.get_mut(&id).unwrap().agent.status = if running {
                                aether_core::agent::AgentStatus::Running
                            } else {
                                aether_core::agent::AgentStatus::Waiting {
                                    condition: aether_core::agent::WaitCondition::Inbox,
                                }
                            };
                            if quota {
                                app.quota_alert = Some("anthropic weekly limit reached".into());
                            }
                            let theme = Theme::resolve(ThemeSpec {
                                mode,
                                ascii,
                                ..Default::default()
                            });
                            let board = crate::ui::layout::board_mode(screen_width, true);
                            let width = match board {
                                crate::ui::layout::BoardMode::Docked { width } => {
                                    screen_width - width - 2
                                }
                                _ => screen_width,
                            };
                            let line = footer_line(
                                &app,
                                &theme,
                                width,
                                false,
                                board == crate::ui::layout::BoardMode::Overlay,
                            );
                            let text: String = line
                                .spans
                                .iter()
                                .map(|span| span.content.as_ref())
                                .collect();
                            assert!(line.width() <= usize::from(width), "{text:?}");
                            for action in ["enter", "esc", "stop", "remove", "pause", "interrupt"] {
                                assert!(
                                    text.contains(action),
                                    "{screen_width}/{width} {mode:?} ascii={ascii} quota={quota}: {text:?} lacks {action}"
                                );
                            }
                            if quota {
                                assert!(
                                    line.spans.iter().any(|span| !span.content.is_empty()
                                        && span.style == theme.failure()),
                                    "{text:?}"
                                );
                            }
                            if text.contains('$') {
                                assert!(text.contains("$0.51"), "{text:?}");
                            }
                            if text.contains('%') {
                                assert!(text.contains("68%"), "{text:?}");
                            }
                            if text.contains("context") {
                                assert!(text.contains("68% until compaction"), "{text:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn resting_hints_offer_the_way_back_from_a_child() {
        let app = busy_child();
        let theme = dark_theme(false);
        let right = "$0.51 · 68% until compaction";
        assert_eq!(
            row(&app, &theme, 120),
            spread(
                "? shortcuts · ctrl+g agents · alt+↑ back to aether",
                right,
                120
            )
        );
        assert_eq!(
            row(&app, &theme, 80),
            spread("? shortcuts · ctrl+g agents", right, 80)
        );
        assert_eq!(
            row(&app, &theme, 60),
            spread("? shortcuts · ctrl+g agents", right, 60)
        );
        assert_eq!(
            hints(&app),
            "? shortcuts · ctrl+g agents · alt+↑ back to aether"
        );
    }

    #[test]
    fn keys_are_bold_words_and_the_right_side_dim() {
        let app = busy_child();
        let theme = dark_theme(false);
        let terminal = draw(&app, &theme, 120);
        let buffer = terminal.backend().buffer();
        let cell = |x: u16| &buffer[(x, 0)];
        assert!(shows(cell(2), theme.title()), "{:?}", cell(2));
        assert!(shows(cell(4), theme.secondary()));
        assert!(shows(cell(16), theme.title()));
        assert!(shows(cell(119 - 1), theme.secondary()));
        assert!((0..120).all(|x| cell(x).bg == Color::Reset));
    }

    #[test]
    fn the_board_and_open_views_replace_the_resting_hints() {
        let mut app = busy_child();
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.size = (120, 40);
        assert_eq!(
            hints(&app),
            "↑↓ · enter view · esc back · ⇧x stop · ⇧d remove · ⇧p pause · ⇧i interrupt · ←→ fold · ctrl+b hide"
        );
        let theme = dark_theme(false);
        let text = row(&app, &theme, 80);
        assert!(
            text.contains("enter view") && text.contains("esc back"),
            "{text}"
        );
        assert!(
            ["⇧x stop", "⇧d remove", "⇧p pause", "⇧i interrupt"]
                .iter()
                .all(|hint| text.contains(hint)),
            "{text}"
        );
        app.theme = dark_theme(true);
        assert_eq!(
            hints(&app),
            "up/down | enter | esc | shift+x/d/p/i stop/remove/pause/interrupt | left/right fold | ctrl+b hide"
        );
    }

    /// M8: a focused board keeps its keys in view over a draft.
    #[test]
    fn a_focused_board_shows_its_keys_while_a_draft_waits() {
        let mut app = busy_child();
        app.input.set_text("typing typing");
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        let theme = dark_theme(false);
        assert!(
            row(&app, &theme, 120).starts_with("  ↑↓ · enter view · esc back"),
            "{}",
            row(&app, &theme, 120)
        );
        app.board.focus = BoardFocus::BottomPane;
        assert!(
            row(&app, &theme, 120).starts_with("   "),
            "typing hides them"
        );
    }

    #[test]
    fn a_flash_wins_and_typing_hides_the_hints() {
        let mut app = busy_child();
        let theme = dark_theme(false);
        app.input.set_text("half a thought");
        assert_eq!(
            row(&app, &theme, 80),
            spread("", "$0.51 · 68% until compaction", 80)
        );

        app.flash("effort → high (next turn)");
        let terminal = draw(&app, &theme, 80);
        assert_eq!(
            screen_text(&terminal),
            spread(
                "effort → high (next turn)",
                "$0.51 · 68% until compaction",
                80
            )
        );
        assert!(shows(&terminal.backend().buffer()[(2, 0)], theme.text()));

        // Expired flashes are gone without anyone clearing them.
        app.now += chrono::Duration::seconds(10);
        app.input.set_text("");
        assert!(row(&app, &theme, 80).starts_with("  ? shortcuts"));
    }

    #[test]
    fn the_quota_chip_is_red_and_outlives_cost_and_context() {
        let mut app = busy_child();
        app.quota_alert = Some("anthropic weekly limit reached".into());
        let theme = dark_theme(false);
        let terminal = draw(&app, &theme, 120);
        let text = screen_text(&terminal);
        assert!(
            text.ends_with("anthropic weekly limit reached · $0.51 · 68% until compaction "),
            "{text:?}"
        );
        let chip_x = u16::try_from(text.find("anthropic").expect("chip")).expect("narrow");
        assert!(shows(
            &terminal.backend().buffer()[(chip_x, 0)],
            theme.failure()
        ));

        let narrow = row(&app, &theme, 50);
        assert!(
            narrow.ends_with("anthropic weekly limit reached · $0.51 · 68% "),
            "{narrow:?}"
        );
        let tight = row(&app, &theme, 30);
        assert!(tight.ends_with("anthropic weekly limit rea… "), "{tight:?}");
    }

    #[test]
    fn the_root_has_no_way_back_and_costs_say_when_they_are_estimates() {
        let mut app = busy_child();
        app.selected = app.tree.roots.first().copied();
        assert_eq!(hints(&app), "? shortcuts · ctrl+g agents");
        // No context figure for the root, and a complete $0 still reads as spend.
        assert!(row(&app, &dark_theme(false), 80).ends_with(" $0.00 "));

        let floor = Usage {
            cost_usd: 1.5,
            cost_complete: false,
            ..Usage::default()
        };
        assert_eq!(run_cost(&floor).as_deref(), Some("~$1.50"));
        let estimated = Usage {
            estimated_cost_usd: Some(0.25),
            ..Usage::default()
        };
        assert_eq!(run_cost(&estimated).as_deref(), Some("~$0.25"));
        let unknown = Usage {
            cost_complete: false,
            ..Usage::default()
        };
        assert_eq!(run_cost(&unknown), None);
        assert_eq!(dollars(0.004), "<$0.01");
    }

    #[test]
    fn cost_figures_name_their_basis() {
        use aether_core::model::CostBasis;
        for (basis, expected) in [
            (CostBasis::ListPrice, "~$1.50 list-price estimate"),
            (CostBasis::ProviderReported, "$1.50 provider-reported"),
            (CostBasis::Mixed, "$1.50 mixed cost basis"),
        ] {
            let usage = Usage {
                cost_usd: 1.5,
                cost_basis: basis,
                ..Default::default()
            };
            assert_eq!(run_cost(&usage).as_deref(), Some(expected));
            let cost = crate::app::board::cost_of(&usage);
            assert_eq!(crate::ui::board::cost_text(cost), expected);
            assert_eq!(crate::ui::cells::text::money(cost.unwrap()), expected);
        }
    }

    #[test]
    fn details_and_reading_stay_dim_on_the_right_while_typing() {
        let mut app = busy_child();
        app.details = true;
        app.input.set_text("a draft");
        let theme = dark_theme(false);
        let line = footer_line(&app, &theme, 120, true, false);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            text.contains("↑ reading · ctrl+end to follow · details · ctrl+t hide"),
            "{text}"
        );
        for span in line
            .spans
            .iter()
            .filter(|span| span.content.contains("reading") || span.content.contains("details"))
        {
            assert_eq!(span.style, theme.secondary());
        }
        let text: String = footer_line(&app, &theme, 120, false, false)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(!text.contains("reading"), "{text}");
        assert!(text.contains("details"), "{text}");
        // Plain End edits the draft; only Ctrl+End follows again.
        let text: String = footer_line(&app, &dark_theme(true), 120, true, false)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("up reading | ctrl+end to follow"), "{text}");
    }

    #[test]
    fn the_overlay_prioritizes_navigation_and_lifecycle_keys() {
        let mut app = busy_child();
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.size = (80, 34);
        let text = row(&app, &dark_theme(false), 80);
        assert!(
            text.contains("⇧d remove") && text.contains("⇧i interrupt"),
            "{text}"
        );
        assert!(text.starts_with("  ↑↓"), "{text}");
    }

    /// At 60 columns the overlay's hints leave one column: no `…` stub stands in for the label.
    #[test]
    fn the_overlay_leaves_no_lone_ellipsis_where_its_label_was() {
        let mut app = busy_child();
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.size = (60, 40);
        for ascii in [false, true] {
            let text = row(&app, &dark_theme(ascii), 60);
            assert!(text.ends_with("interrupt   "), "{text:?}");
        }
    }

    #[test]
    fn navigation_and_lifecycle_hints_fit_the_dock_and_ascii_overlay() {
        for (screen_width, footer_width, ascii, actions) in [
            (
                80,
                80,
                false,
                "⇧x stop · ⇧d remove · ⇧p pause · ⇧i interrupt",
            ),
            (80, 80, true, "shift+x/d/p/i stop/remove/pause/interrupt"),
            (100, 58, false, "⇧x/d/p/i stop/remove/pause/interrupt"),
            (100, 58, true, "shift+x/d/p/i stop/remove/pause/interrupt"),
        ] {
            let mut app = busy_child();
            app.board.open = true;
            app.board.focus = BoardFocus::Board;
            app.size = (screen_width, 40);
            let line = footer_line(
                &app,
                &dark_theme(ascii),
                footer_width,
                false,
                screen_width < crate::ui::layout::BOARD_DOCK_MIN_COLS,
            );
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert!(
                text.contains("enter") && text.contains("esc"),
                "{screen_width} cols: {text}"
            );
            assert!(text.contains(actions), "{screen_width} cols: {text}");
            assert!(
                text.ends_with("until compaction ")
                    || text.ends_with(" 68% ")
                    || !text.contains('%'),
                "a figure shortens or drops whole: {text}"
            );
        }
    }

    #[test]
    fn slash_views_keep_their_keys_inside_the_block() {
        let mut app = busy_child();
        app.flash = None;
        app.open_view(crate::ui::views::ViewKind::Memory);
        assert_eq!(hints(&app), "");
        assert_eq!(
            row(&app, &dark_theme(false), 120),
            spread("", "$0.51 · 68% until compaction", 120)
        );
    }

    #[test]
    fn usage_figures_drop_whole_rather_than_being_cut() {
        let mut app = busy_child();
        let theme = dark_theme(false);
        let text = |app: &App, room| -> String {
            right_side(app, &theme, " · ", room, false, false)
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        assert_eq!(text(&app, 28), "$0.51 · 68% until compaction");
        assert_eq!(text(&app, 27), "$0.51 · 68%");
        assert_eq!(text(&app, 10), "$0.51");
        assert_eq!(text(&app, 4), "68%");
        assert_eq!(text(&app, 2), "");
        let id = viewed(&app);
        app.tree.nodes.get_mut(&id).expect("node").subtree_usage = Usage {
            cost_complete: false,
            ..Usage::default()
        };
        assert_eq!(text(&app, 20), "68% until compaction");
        assert_eq!(text(&app, 19), "68%");
        assert_eq!(text(&app, 2), "");
    }

    /// C-F7: the 100-column dock leaves 58 columns; the actions and a short context figure share them.
    #[test]
    fn the_narrowest_dock_keeps_every_action_and_a_short_context_figure() {
        let mut app = busy_child();
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.size = (100, 40);
        let crate::ui::layout::BoardMode::Docked { width } =
            crate::ui::layout::board_mode(100, true)
        else {
            panic!("the board docks at 100 columns");
        };
        let docked = |app: &App, theme: &Theme| -> String {
            footer_line(app, theme, 100 - width - 2, false, false)
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        let light = Theme::resolve(crate::ui::theme::tokens::ThemeSpec {
            mode: crate::ui::theme::tokens::ThemeMode::Light,
            ..Default::default()
        });
        for theme in [dark_theme(false), light] {
            assert_eq!(
                docked(&app, &theme),
                spread(
                    "enter · esc · ⇧x/d/p/i stop/remove/pause/interrupt",
                    "68%",
                    58
                )
            );
        }
        let id = viewed(&app);
        app.context_tokens.insert(id, 0);
        assert!(docked(&app, &dark_theme(false)).ends_with(" 100% "));
        // `shift+` leaves no room in ASCII: the figure drops whole; the inspector still shows it.
        let ascii = docked(&app, &dark_theme(true));
        assert!(!ascii.contains('%'), "{ascii}");
        assert!(
            ascii.contains("shift+x/d/p/i stop/remove/pause/interrupt"),
            "{ascii}"
        );
    }

    #[test]
    fn focused_dock_preserves_the_context_figure() {
        let mut app = busy_child();
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.size = (160, 44);
        for ascii in [false, true] {
            let text = row(&app, &dark_theme(ascii), 110);
            assert!(text.ends_with("68% until compaction "), "{text}");
            assert!(text.contains("enter") && text.contains("esc"), "{text}");
            assert!(
                text.contains("stop/remove/pause/interrupt") || text.contains("⇧i interrupt"),
                "{text}"
            );
        }
    }
}
