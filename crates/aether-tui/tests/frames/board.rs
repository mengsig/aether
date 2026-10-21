use super::*;

use aether_core::model::Usage;
use aether_tui::testing::{
    loaded_app_with_capped_subtree, loaded_app_with_capped_subtree_under_finished_child,
};
use aether_tui::ui::layout::{BoardMode, board_mode};

/// `app` drawn with the board open at `w`x`h`.
struct Docked {
    buffer: Buffer,
    /// The board's own columns, padding included, row by row.
    rows: Vec<String>,
    /// Buffer column of the board's first column.
    x: u16,
}

impl Docked {
    /// The drawn cell at board column `x` of board row `y`.
    fn cell(&self, x: u16, y: usize) -> &Cell {
        &self.buffer[(self.x + x, y as u16)]
    }
}

fn board(app: &mut App, w: u16, h: u16) -> Docked {
    app.board.open = true;
    let BoardMode::Docked { width } = board_mode(w, true) else {
        panic!("these frames dock the board");
    };
    let x = w - width;
    let buffer = draw(app, w, h);
    let rows = rows(&buffer)
        .iter()
        .map(|r| r.chars().skip(usize::from(x)).collect())
        .collect();
    Docked { buffer, rows, x }
}

fn context_window(app: &mut App, window: u32, compaction_tokens: Option<u32>) {
    use aether_protocol::ModelInfo;
    let model = app.selected_agent().expect("a viewed agent").model.clone();
    app.models.push(ModelInfo {
        tier: None,
        aliases: Vec::new(),
        name: model.model.clone(),
        provider_name: "anthropic".into(),
        model,
        capabilities: aether_core::model::CapabilityProfile {
            context_window: window,
            compaction_tokens,
            ..Default::default()
        },
        configured: true,
        auth_source: None,
        is_default: false,
    });
}

#[test]
fn root_alias_context_bar_and_footer_support_new_and_old_catalogues() {
    use aether_core::model::ModelRef;

    for ascii in [false, true] {
        let mut app = quiet(loaded_app(1, 0, 0));
        app.theme = dark(ascii);
        let id = viewed(&app);
        let node = app.tree.nodes.get_mut(&id).expect("root");
        assert!(node.agent.parent.is_none());
        node.agent.name = "assistant".into();
        node.agent.model = ModelRef::new("claude-code", "opus");
        app.context_tokens.insert(id, 82_000);
        context_window(&mut app, 1_000_000, Some(256_000));
        let catalogue = app.models.last_mut().expect("catalogue entry");
        catalogue.model.model = "claude-opus-5-5".into();
        catalogue.aliases = vec!["opus".into()];
        app.rebuild_after_tree_change();

        let docked = board(&mut app, 160, 44);
        let text = docked.rows.join("\n");
        let context = docked
            .rows
            .iter()
            .find(|row| row.trim_start().starts_with("Context"))
            .expect("context row");
        assert!(context.contains(app.theme.glyphs().bar), "{text}");
        assert!(context.contains("32%"), "{text}");
        assert!(text.contains("82k of 256k until compaction"), "{text}");
        let screen = rows(&docked.buffer).join("\n");
        assert!(screen.contains("68% until compaction"), "{screen}");

        let catalogue = app.models.last_mut().expect("catalogue entry");
        let mut legacy = serde_json::to_value(&*catalogue).expect("wire catalogue");
        legacy.as_object_mut().expect("object").remove("aliases");
        *catalogue = serde_json::from_value(legacy).expect("old daemon catalogue");
        let docked = board(&mut app, 160, 44);
        let context = docked
            .rows
            .iter()
            .find(|row| row.trim_start().starts_with("Context"))
            .expect("size-only context row");
        assert_eq!(context.trim(), "Context    82k");
        let screen = rows(&docked.buffer).join("\n");
        assert!(!screen.contains("% until compaction"), "{screen}");

        app.tree.nodes.get_mut(&id).expect("root").agent.model.model = "claude-opus-5-5".into();
        app.rebuild_after_tree_change();
        let screen = rows(&board(&mut app, 160, 44).buffer).join("\n");
        assert!(screen.contains("68% until compaction"), "{screen}");
    }
}

/// SPEC §2.1: children hang under their parent, and the inspector lists the
/// viewed agent's own children.
#[test]
fn the_board_shows_the_whole_tree_and_the_inspector_its_children() {
    let mut app = quiet(loaded_app_with_capped_subtree(4));
    let board = board(&mut app, 160, 44).rows;
    let text = board.join("\n");
    let row_of = |name: &str| {
        board
            .iter()
            .position(|r| r.split_whitespace().any(|w| w == name))
            .unwrap_or_else(|| panic!("{name} is not on the board:\n{text}"))
    };
    let (root, child, grandchild) = (
        row_of("orchestrator"),
        row_of("worker"),
        row_of("sub-worker"),
    );
    assert!(root < child && child < grandchild, "{text}");
    let indent = |row: usize, name: &str| column_of(&board[row], name).expect("found");
    assert!(
        indent(root, "orchestrator") < indent(child, "worker"),
        "{text}"
    );
    assert!(
        indent(child, "worker") < indent(grandchild, "sub-worker"),
        "{text}"
    );

    let children = board
        .iter()
        .position(|r| r.trim() == "Children")
        .unwrap_or_else(|| panic!("no children section:\n{text}"));
    assert!(
        board[children + 1].contains("worker") && !board[children + 1].contains("sub-worker"),
        "the direct child belongs under Children:\n{text}"
    );
}

/// A finished intermediate does not fold away a running
/// grandchild - only a subtree with nothing live in it collapses.
#[test]
fn a_running_grandchild_under_a_finished_child_stays_on_the_board() {
    let mut app = quiet(loaded_app_with_capped_subtree_under_finished_child(4));
    let docked = board(&mut app, 160, 44);
    let text = docked.rows.join("\n");
    let worker = docked
        .rows
        .iter()
        .position(|r| r.split_whitespace().any(|w| w == "worker"))
        .unwrap_or_else(|| panic!("the finished child is missing:\n{text}"));
    assert!(
        docked.rows.iter().any(|r| r.contains("sub-worker")),
        "the running grandchild folded away with its finished parent:\n{text}"
    );
    let done = app.theme.glyphs().done;
    let x = column_of(&docked.rows[worker], done).unwrap_or_else(|| {
        panic!(
            "the finished child has no {done}: {:?}",
            docked.rows[worker]
        )
    });
    assert!(
        shows(docked.cell(x, worker), app.theme.success()),
        "a finished agent's glyph is not success()"
    );
}

/// SPEC §1.2: in a short terminal the inspector drops its low-priority rows,
/// and the context and the model are not among them.
#[test]
fn a_short_board_keeps_the_context_and_the_model() {
    let mut app = quiet(loaded_app(20, 0, 0));
    let id = viewed(&app);
    app.context_tokens.insert(id, 12_345);
    let board = board(&mut app, 120, 22).rows;
    let text = board.join("\n");
    let row = |label: &str| board.iter().find(|r| r.trim_start().starts_with(label));
    let context = row("Context").unwrap_or_else(|| panic!("no context row:\n{text}"));
    assert!(context.contains("12.3k"), "{context:?}");
    assert!(row("Model").is_some(), "no model row:\n{text}");
}

/// An unknown cost prints nothing (never "not reported", never a bare `—`),
/// and a wide name does not push a known cost off the board's right edge.
#[test]
fn tree_rows_keep_wide_names_and_omit_an_unknown_cost() {
    for ascii in [false, true] {
        let mut app = quiet(loaded_app(1, 0, 0));
        app.theme = dark(ascii);
        let id = viewed(&app);
        let node = app.tree.nodes.get_mut(&id).expect("node");
        node.agent.name = "測試-agent".into();
        node.agent.stats.usage = Usage {
            input_tokens: 12_000,
            cost_complete: false,
            ..Usage::default()
        };
        app.rebuild_after_tree_change();
        let docked = board(&mut app, 100, 30);
        let rows = &docked.rows;
        let text = rows.join("\n");
        assert!(!text.contains("not reported"), "{text}");
        // A wide glyph fills two cells; the second draws as a blank.
        let y = rows
            .iter()
            .position(|r| r.contains("-agent"))
            .unwrap_or_else(|| panic!("the name is gone:\n{text}"));
        let row = &rows[y];
        let x = column_of(row, "-agent").expect("found") - 4;
        assert_eq!(
            (docked.cell(x, y).symbol(), docked.cell(x + 2, y).symbol()),
            ("測", "試"),
            "ascii={ascii}: {row:?}"
        );
        let end = row.trim_end();
        assert!(
            !end.ends_with('—') && !end.ends_with(" -") && !end.contains('$'),
            "ascii={ascii}: {row:?}"
        );

        let node = app.tree.nodes.get_mut(&id).expect("node");
        node.agent.stats.usage.cost_complete = true;
        node.agent.stats.usage.cost_usd = 0.42;
        app.rebuild_after_tree_change();
        let docked = board(&mut app, 100, 30);
        let row = docked
            .rows
            .iter()
            .find(|r| r.contains("-agent"))
            .expect("the name row");
        assert!(row.trim_end().ends_with("$0.42"), "ascii={ascii}: {row:?}");
        // Board padding is two columns; the cost ends right inside it.
        assert_eq!(
            row.trim_end().chars().count(),
            row.chars().count() - 2,
            "ascii={ascii}: the cost is not flush right: {row:?}"
        );
    }
}

/// SPEC §2.2: context pressure is a bar - accent fill on a faint track - with
/// the share beside it, bold from 85%.
#[test]
fn the_inspector_shows_context_pressure_as_a_bar() {
    for ascii in [false, true] {
        for (used, percent, bold) in [(75_000u64, "75%", false), (90_000, "90%", true)] {
            let mut app = quiet(loaded_app(1, 0, 0));
            app.theme = dark(ascii);
            context_window(&mut app, 1_000_000, Some(100_000));
            let id = viewed(&app);
            app.context_tokens.insert(id, used);
            let docked = board(&mut app, 160, 44);
            let board = &docked.rows;
            let text = board.join("\n");
            let y = board
                .iter()
                .position(|r| r.trim_start().starts_with("Context"))
                .unwrap_or_else(|| panic!("no context row:\n{text}"));
            let row = &board[y];
            assert!(row.trim_end().ends_with(percent), "{row:?}");
            let detail = format!("{}k of 100k until compaction", used / 1_000);
            assert!(board[y + 1].contains(&detail), "{:?}", board[y + 1]);

            let theme = &app.theme;
            let start = column_of(row, theme.glyphs().bar).expect("a bar");
            assert!(
                shows(docked.cell(start, y), theme.accent()),
                "ascii={ascii}: the fill is not accent"
            );
            let track = (start..row.chars().count() as u16)
                .map(|x| docked.cell(x, y))
                .find(|c| !shows(c, theme.accent()))
                .expect("a track");
            assert!(
                shows(track, theme.track()),
                "ascii={ascii}: the track is not track()"
            );
            let pct = docked.cell(column_of(row, percent).expect("found"), y);
            assert_eq!(
                pct.modifier.contains(theme.title().add_modifier),
                bold,
                "ascii={ascii}: {percent} bold should be {bold}"
            );
            if ascii {
                assert!(text.is_ascii(), "{:?} {text}", non_ascii(&text));
            }
        }
    }
}

/// The operator rule: a suspended agent's quota stays visible, with its
/// provider and when it will try again.
#[test]
fn the_inspector_says_when_a_quota_blocked_agent_tries_again() {
    use aether_core::agent::{BlockReason, ExhaustionClass};
    let mut app = quiet(loaded_app(1, 0, 0));
    let id = viewed(&app);
    let model = app.tree.nodes[&id].agent.model.clone();
    let next_probe: aether_core::time::Timestamp = "2026-10-11T18:30:00Z".parse().unwrap();
    set_status(
        &mut app,
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                model,
                class: ExhaustionClass::UsageLimit,
                since: "2026-10-11T12:00:00Z".parse().unwrap(),
                reported_reset: None,
                next_probe,
                probes: 2,
                detail: "rate limited".into(),
            },
        },
    );
    // The widest board: the whole quota line fits on one row.
    let text = board(&mut app, 200, 44).rows.join("\n");
    assert!(text.contains("Blocked"), "{text}");
    let at = next_probe.with_timezone(&chrono::Local).format("%H:%M");
    assert!(
        text.contains(&format!("anthropic usage limit reached · retrying at {at}")),
        "{text}"
    );
}

/// The same frame under `--ascii`: the quota line's separator is chrome too.
#[test]
fn a_quota_blocked_agent_reads_in_ascii() {
    use aether_core::agent::{BlockReason, ExhaustionClass};
    let mut app = quiet(loaded_app(1, 0, 0));
    app.theme = dark(true);
    let id = viewed(&app);
    let model = app.tree.nodes[&id].agent.model.clone();
    set_status(
        &mut app,
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                model,
                class: ExhaustionClass::QuotaExhausted,
                since: "2026-10-11T12:00:00Z".parse().unwrap(),
                reported_reset: None,
                next_probe: "2026-10-11T18:30:00Z".parse().unwrap(),
                probes: 2,
                detail: "rate limited".into(),
            },
        },
    );
    let text = board(&mut app, 200, 44).rows.join("\n");
    assert!(text.contains("quota exhausted"), "{text}");
    assert!(text.is_ascii(), "{:?} {text}", non_ascii(&text));
}

/// SPEC §2.1-§2.2: an agent waiting on the operator is marked in the attention
/// hue on the board, and the inspector says what it wants.
#[test]
fn an_agent_waiting_on_an_approval_is_marked_on_the_board() {
    for ascii in [false, true] {
        let mut app = quiet(populated_app());
        app.theme = dark(ascii);
        // Stands in for the next tree refresh; see the ignored test below.
        app.rebuild_after_tree_change();
        let docked = board(&mut app, 160, 44);
        let board = &docked.rows;
        let text = board.join("\n");
        let mark = app.theme.glyphs().needs_you;
        let y = board
            .iter()
            .position(|r| r.contains(&format!("{mark} assistant")))
            .unwrap_or_else(|| panic!("ascii={ascii}: no needs-you row:\n{text}"));
        let x = column_of(&board[y], mark).expect("found");
        assert!(
            shows(
                docked.cell(x, y),
                app.theme.attention().patch(app.theme.title())
            ),
            "ascii={ascii}: the marker is not attention bold"
        );
        assert!(text.contains("Needs you"), "{text}");
        assert!(text.contains("Wants to run ls"), "{text}");
        if ascii {
            assert!(text.is_ascii(), "{:?} {text}", non_ascii(&text));
        }
    }
}

/// The approval event alone marks the board: the operator should not wait
/// for an unrelated tree refresh to see who needs them.
#[test]
fn an_approval_event_marks_the_board_without_a_tree_refresh() {
    let mut app = quiet(populated_app());
    let rows = board(&mut app, 160, 44).rows;
    let mark = app.theme.glyphs().needs_you;
    assert!(
        rows.iter()
            .any(|r| r.contains(&format!("{mark} assistant"))),
        "no needs-you row:\n{}",
        rows.join("\n")
    );
}
