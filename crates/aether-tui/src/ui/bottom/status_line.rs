//! Status line (SPEC §1.1 item 5): `model effort · cwd · branch · child ‹ parent`,
//! dim except the accent model chip; drops right-to-left, agent name last. Owner: W1.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::format;
use crate::ui::theme::tokens::Theme;

/// Lines the bottom pane up with the transcript gutter.
const INDENT: &str = "  ";

struct Item {
    text: String,
    style: Style,
    /// The viewed agent and its paused badge outlive every other item as the width shrinks.
    agent: bool,
}

pub fn render(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.is_empty() {
        return;
    }
    let line = status_line(app, theme, area.width);
    frame
        .buffer_mut()
        .set_line(area.x, area.y, &line, area.width);
}

fn status_line(app: &App, theme: &Theme, width: u16) -> Line<'static> {
    let glyphs = theme.glyphs();
    let separator = format!(" {} ", glyphs.dot);
    let budget = usize::from(width).saturating_sub(INDENT.width());
    let mut items = items(app, theme);
    while line_width(&items, &separator) > budget {
        // Rightmost first, skipping the agent; a lone item is truncated instead.
        match items.iter().rposition(|item| !item.agent) {
            Some(at) if items.len() > 1 => {
                items.remove(at);
            }
            _ => break,
        }
    }
    if let [only] = items.as_mut_slice() {
        let marker = glyphs.ellipsis.chars().next().unwrap_or('~');
        only.text = format::truncate_with_marker(&only.text, budget, marker);
    }

    let mut spans = vec![Span::raw(INDENT)];
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(separator.clone(), theme.secondary()));
        }
        spans.push(Span::styled(item.text, item.style));
    }
    Line::from(spans)
}

/// Model chip, cwd, branch and the agent breadcrumb, left to right.
fn items(app: &App, theme: &Theme) -> Vec<Item> {
    let agent = app.selected_agent();
    let mut items = Vec::new();
    if let Some(aether_client::Endpoint::Ssh(remote)) = &app.options.endpoint {
        items.push(Item {
            text: format!(
                "{} · {} · {}",
                remote.name(),
                if app.connected {
                    "connected"
                } else {
                    "reconnecting"
                },
                remote.target()
            ),
            style: theme.secondary(),
            agent: false,
        });
    }

    let chip = match agent {
        Some(agent) => Some(match agent.config.effort {
            Some(effort) => format!("{} {effort}", agent.model.model),
            None => agent.model.model.clone(),
        }),
        None => app.options.default_model.as_ref().map(|m| {
            // Before the first turn, the rate a send will be billed at.
            match app.model_info(m).map(|info| info.capabilities.cost) {
                Some(Some(cost)) => format!(
                    "{} · ${}/${} per Mtok",
                    m.model, cost.input_per_mtok, cost.output_per_mtok
                ),
                Some(None) => format!("{} · no list price", m.model),
                None => m.model.clone(),
            }
        }),
    };
    if let Some(text) = chip {
        items.push(Item {
            text,
            style: theme.accent(),
            agent: false,
        });
    }

    let cwd = agent
        .and_then(|a| a.scope.workspace_root.as_deref())
        .or(app.active_workspace());
    if let Some(cwd) = cwd {
        let marker = theme.glyphs().ellipsis.chars().next().unwrap_or('~');
        items.push(Item {
            text: format::short_path_with_marker(cwd, marker),
            style: theme.secondary(),
            agent: false,
        });
        if let Some(branch) = app.branch_at(cwd) {
            items.push(Item {
                text: branch.to_owned(),
                style: theme.secondary(),
                agent: false,
            });
        }
    }

    let waiting = app.undelivered.len() + app.unreadable.len();
    if waiting > 0 {
        items.push(Item {
            text: format!("{waiting} unconfirmed /outbox"),
            style: theme.failure(),
            agent: false,
        });
    }
    if !app.view_modes.diagnostics.is_empty() {
        items.push(Item {
            text: format!("{} mode notices /mode", app.view_modes.diagnostics.len()),
            style: theme.failure(),
            agent: false,
        });
    }
    if let Some(text) = app.mode_label() {
        items.push(Item {
            text,
            style: theme.secondary(),
            agent: false,
        });
    }
    if let Some(agent) = agent {
        if agent.paused {
            items.push(Item {
                text: "PAUSED".to_owned(),
                style: theme.paused(),
                agent: true,
            });
        }
        let parent = agent
            .parent
            .and_then(|id| app.tree.nodes.get(&id))
            .map(|node| app.agent_display_name(node.agent.id));
        let crumb = if theme.ascii() { "<" } else { "‹" };
        let text = match parent {
            Some(parent) => format!("{} {crumb} {parent}", app.agent_display_name(agent.id)),
            None => app.agent_display_name(agent.id),
        };
        items.push(Item {
            text,
            style: theme.text(),
            agent: true,
        });
    }
    items
}

fn line_width(items: &[Item], separator: &str) -> usize {
    let text: usize = items.iter().map(|item| item.text.width()).sum();
    text + separator.width() * items.len().saturating_sub(1)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
    use aether_core::id::{AgentId, RunId};
    use aether_core::model::{Effort, ModelRef};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use ratatui::style::Color;

    use super::*;
    use crate::app::TuiOptions;
    use crate::ui::tests_support::screen_text;
    use crate::ui::theme::ColorDepth;
    use crate::ui::theme::tokens::{Rgb, TerminalPalette, ThemeSpec};

    pub(crate) fn dark_theme(ascii: bool) -> Theme {
        Theme::resolve(ThemeSpec {
            depth: ColorDepth::Truecolor,
            palette: Some(TerminalPalette {
                fg: Rgb(0xe6, 0xed, 0xf3),
                bg: Rgb(0x0d, 0x11, 0x17),
            }),
            ascii,
            ..ThemeSpec::default()
        })
    }

    fn agent(name: &str, parent: Option<AgentId>, model: &str, effort: Effort, cwd: &str) -> Agent {
        let at = aether_core::time::now();
        Agent {
            id: AgentId::new(),
            parent,
            role: Role::new("worker"),
            name: name.into(),
            scope: Scope {
                workspace_root: Some(PathBuf::from(cwd)),
                ..Scope::default()
            },
            model: ModelRef::new("anthropic", model),
            fallbacks: Vec::new(),
            status: AgentStatus::Ready,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            archived: false,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig {
                effort: Some(effort),
                ..AgentConfig::default()
            },
            stats: AgentStats::default(),
            run: RunId::new(),
            created_at: at,
            updated_at: at,
        }
    }

    /// Whether a drawn cell shows exactly `style` (unset colours read as `Reset`).
    pub(crate) fn shows(cell: &Cell, style: Style) -> bool {
        cell.fg == style.fg.unwrap_or(Color::Reset)
            && cell.bg == style.bg.unwrap_or(Color::Reset)
            && cell.modifier == style.add_modifier
    }

    /// Root `aether` with a child `implementer`; the child is viewed.
    pub(crate) fn app_viewing_child() -> App {
        let mut app = App::new(TuiOptions {
            cwd: Some(PathBuf::from("/work/aether")),
            ..TuiOptions::default()
        });
        let root = agent("aether", None, "opus-5.5", Effort::High, "/work/aether");
        let child = agent(
            "implementer",
            Some(root.id),
            "sonnet-5.5",
            Effort::Medium,
            "/work/aether/wt/impl",
        );
        let (root_id, child_id) = (root.id, child.id);
        for (agent, children) in [(root, vec![child_id]), (child, vec![])] {
            app.tree.nodes.insert(
                agent.id,
                aether_protocol::TreeNode {
                    agent,
                    children,
                    task: None,
                    waiting_on: vec![],
                    lease: None,
                    subtree_usage: Default::default(),
                    hidden_usage: None,
                },
            );
        }
        app.tree.roots = vec![root_id];
        app.selected = Some(child_id);
        app
    }

    fn draw(app: &App, theme: &Theme, width: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).expect("terminal");
        terminal
            .draw(|frame| render(frame, frame.area(), app, theme))
            .expect("draw");
        terminal
    }

    fn row(app: &App, theme: &Theme, width: u16) -> String {
        screen_text(&draw(app, theme, width)).trim_end().to_owned()
    }

    #[test]
    fn items_drop_right_to_left_and_the_agent_goes_last() {
        let app = app_viewing_child();
        let theme = dark_theme(false);
        assert_eq!(
            row(&app, &theme, 120),
            "  sonnet-5.5 medium · /work/aether/wt/impl · implementer ‹ aether"
        );
        assert_eq!(
            row(&app, &theme, 80),
            "  sonnet-5.5 medium · /work/aether/wt/impl · implementer ‹ aether"
        );
        assert_eq!(
            row(&app, &theme, 60),
            "  sonnet-5.5 medium · implementer ‹ aether"
        );
        assert_eq!(row(&app, &theme, 40), "  implementer ‹ aether");
        assert_eq!(row(&app, &theme, 16), "  implementer ‹…");
    }

    #[test]
    fn a_paused_badge_survives_narrow_widths_and_clears_on_resume() {
        let mut app = app_viewing_child();
        let theme = dark_theme(false);
        let child = app.selected.expect("child");
        app.tree.nodes.get_mut(&child).expect("node").agent.paused = true;
        for width in [80, 60, 40] {
            let line = row(&app, &theme, width);
            assert!(line.contains("PAUSED"), "{width}: {line:?}");
            assert!(line.contains("implementer"), "{width}: {line:?}");
        }
        app.tree.nodes.get_mut(&child).expect("node").agent.paused = false;
        assert!(!row(&app, &theme, 80).contains("PAUSED"));
    }

    #[test]
    fn ascii_swaps_the_separators() {
        let app = app_viewing_child();
        assert_eq!(
            row(&app, &dark_theme(true), 80),
            "  sonnet-5.5 medium | /work/aether/wt/impl | implementer < aether"
        );
        assert_eq!(row(&app, &dark_theme(true), 16), "  implementer <~");
    }

    #[test]
    fn the_chip_is_accent_the_rest_dim_and_the_agent_default() {
        let mut app = app_viewing_child();
        app.selected = app.tree.roots.first().copied();
        for theme in [dark_theme(false), Theme::default()] {
            let terminal = draw(&app, &theme, 120);
            assert_eq!(
                screen_text(&terminal).trim_end(),
                "  opus-5.5 high · /work/aether · aether"
            );
            let buffer = terminal.backend().buffer();
            let cell = |x: u16| &buffer[(x, 0)];
            assert!(shows(cell(2), theme.accent()), "{:?}", cell(2));
            assert!(shows(cell(14), theme.accent()));
            assert!(shows(cell(16), theme.secondary()));
            assert!(shows(cell(18), theme.secondary()));
            assert!(shows(cell(33), theme.text()));
            assert!((0..120).all(|x| cell(x).bg == Color::Reset));
        }
    }

    #[test]
    fn without_a_viewed_agent_it_shows_the_session_defaults() {
        let mut app = App::new(TuiOptions {
            cwd: Some(PathBuf::from("/work/aether")),
            default_model: Some(ModelRef::new("anthropic", "opus-5.5")),
            ..TuiOptions::default()
        });
        app.selected = None;
        assert_eq!(
            row(&app, &dark_theme(false), 80),
            "  opus-5.5 · /work/aether"
        );
    }

    #[test]
    fn the_session_default_shows_its_price_once_the_catalogue_is_known() {
        let mut app = App::new(TuiOptions {
            default_model: Some(ModelRef::new("anthropic", "claude-opus-5-5")),
            ..TuiOptions::default()
        });
        app.selected = None;
        app.models = vec![aether_protocol::ModelInfo {
            tier: None,
            aliases: Vec::new(),
            model: ModelRef::new("anthropic", "claude-opus-5-5"),
            name: "Opus".into(),
            provider_name: "Anthropic".into(),
            capabilities: aether_core::model::CapabilityProfile {
                cost: Some(aether_core::model::ModelCost {
                    input_per_mtok: 5.0,
                    output_per_mtok: 25.0,
                    ..Default::default()
                }),
                ..Default::default()
            },
            configured: true,
            auth_source: None,
            is_default: true,
        }];
        let text = row(&app, &dark_theme(false), 80);
        assert!(text.contains("$5/$25 per Mtok"), "{text}");
    }
}
