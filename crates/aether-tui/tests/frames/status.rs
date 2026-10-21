use super::*;

use std::path::PathBuf;

use aether_core::model::{Effort, Usage};

const CWD: &str = "/srv/aether";

/// The viewed agent on `high` effort, launched in [`CWD`] on `main`.
fn located(mut app: App) -> App {
    let id = viewed(&app);
    app.tree
        .nodes
        .get_mut(&id)
        .expect("node")
        .agent
        .config
        .effort = Some(Effort::High);
    app.options.cwd = Some(PathBuf::from(CWD));
    app.branches.insert(PathBuf::from(CWD), Some("main".into()));
    app.rebuild_after_tree_change();
    quiet(app)
}

/// The status line sits right above the hint row.
fn status_line(rows: &[String]) -> &str {
    rows[rows.len() - 2].trim_end()
}

/// SPEC §1.1 item 5: `model effort · cwd · branch · agent`, the model chip in
/// the accent, the agent in the default fg, the rest dim.
#[test]
fn the_status_line_names_the_model_and_the_viewed_agent() {
    for ascii in [false, true] {
        for width in [80u16, 120] {
            let mut app = located(app_with_transcript(&["hello", "hello back"]));
            app.theme = dark(ascii);
            let dot = app.theme.glyphs().dot;
            let buffer = draw(&app, width, 24);
            let rows = rows(&buffer);
            let line = status_line(&rows);
            assert_eq!(
                line,
                format!("  claude-opus-5 high {dot} {CWD} {dot} main {dot} assistant"),
                "ascii={ascii} at {width}"
            );
            let y = (rows.len() - 2) as u16;
            let at = |needle: &str| &buffer[(column_of(line, needle).expect(needle), y)];
            let theme = &app.theme;
            assert!(
                shows(at("claude-opus-5"), theme.accent()),
                "the model chip is not accent"
            );
            assert!(
                shows(at("assistant"), theme.text()),
                "the agent is not default fg"
            );
            assert!(shows(at(CWD), theme.secondary()), "the cwd is not dim");
            assert!(
                shows(at("main"), theme.secondary()),
                "the branch is not dim"
            );
        }
    }
}

/// A viewed child reads as a breadcrumb back to its parent, and the hint row
/// says how to get there.
#[test]
fn a_viewed_child_shows_its_parent_in_the_status_line_and_the_hints() {
    for ascii in [false, true] {
        let mut app = quiet(loaded_app(3, 0, 0));
        app.theme = dark(ascii);
        let child = app
            .tree
            .nodes
            .values()
            .find(|n| n.agent.name == "worker 0")
            .map(|n| n.agent.id)
            .expect("a child");
        app.select(child);
        app.flash = None;
        let rows = screen(&app, 120, 24);
        let (crumb, up) = if ascii {
            ("<", "alt+up")
        } else {
            ("‹", "alt+↑")
        };
        let line = status_line(&rows);
        assert!(
            line.ends_with(&format!("worker 0 {crumb} orchestrator")),
            "ascii={ascii}: {line:?}"
        );
        let hint = rows.last().expect("a hint row");
        assert!(
            hint.contains(&format!("{up} back to orchestrator")),
            "ascii={ascii}: {hint:?}"
        );
    }
}

/// Items leave right to left as the width shrinks - branch, cwd, then the
/// model - and the agent's name is the last thing standing.
#[test]
fn the_status_line_drops_right_to_left_and_keeps_the_agent() {
    let app = located(app_with_transcript(&["hello", "hello back"]));
    let cases: [(u16, &str); 4] = [
        (60, "  claude-opus-5 high · /srv/aether · main · assistant"),
        (50, "  claude-opus-5 high · /srv/aether · assistant"),
        (40, "  claude-opus-5 high · assistant"),
        (24, "  assistant"),
    ];
    for (width, expected) in cases {
        let rows = screen(&app, width, 24);
        assert_eq!(status_line(&rows), expected, "at {width} columns");
    }
}

/// SPEC §1.1 item 6: hints on the left, `$run · NN% until compaction` on the
/// right. Keys are bold, their words dim.
#[test]
fn the_hint_row_shows_the_keys_the_run_cost_and_the_context_left() {
    use aether_protocol::ModelInfo;
    for ascii in [false, true] {
        for width in [80u16, 120] {
            let mut app = quiet(app_with_transcript(&["hello", "hello back"]));
            app.theme = dark(ascii);
            let id = viewed(&app);
            let node = app.tree.nodes.get_mut(&id).expect("node");
            node.subtree_usage = Usage {
                cost_usd: 1.84,
                cost_complete: true,
                ..Usage::default()
            };
            let model = node.agent.model.clone();
            app.models.push(ModelInfo {
                tier: None,
                aliases: Vec::new(),
                name: model.model.clone(),
                provider_name: "anthropic".into(),
                model,
                capabilities: aether_core::model::CapabilityProfile {
                    context_window: 1_000_000,
                    compaction_tokens: Some(256_000),
                    ..Default::default()
                },
                configured: true,
                auth_source: None,
                is_default: false,
            });
            app.context_tokens.insert(id, 36_000);
            let dot = app.theme.glyphs().dot;
            let buffer = draw(&app, width, 24);
            let rows = rows(&buffer);
            let hint = rows.last().expect("a hint row").as_str();
            assert!(
                hint.starts_with(&format!("  ? shortcuts {dot} ctrl+g agents")),
                "ascii={ascii} at {width}: {hint:?}"
            );
            assert!(
                hint.trim_end()
                    .ends_with(&format!("$1.84 {dot} 86% until compaction")),
                "ascii={ascii} at {width}: {hint:?}"
            );
            let y = (rows.len() - 1) as u16;
            let at = |needle: &str| &buffer[(column_of(hint, needle).expect(needle), y)];
            assert!(shows(at("?"), app.theme.title()), "the key is not bold");
            assert!(
                shows(at("shortcuts"), app.theme.secondary()),
                "its words are not dim"
            );
            assert!(
                shows(at("$1.84"), app.theme.secondary()),
                "the cost is not dim"
            );

            // Typing hides the hints; what the run costs stays.
            app.input.set_text("draft");
            let rows = screen(&app, width, 24);
            let hint = rows.last().expect("a hint row");
            assert!(!hint.contains("shortcuts"), "hints while typing: {hint:?}");
            assert!(
                hint.contains("$1.84"),
                "the cost left with the hints: {hint:?}"
            );
        }
    }
}

/// The narrow hint row gives up hints before the cost: what the run is
/// spending is the one thing on it a person cannot see elsewhere.
#[test]
fn a_narrow_hint_row_keeps_the_cost() {
    let mut app = quiet(app_with_transcript(&["hello", "hello back"]));
    let id = viewed(&app);
    app.tree.nodes.get_mut(&id).expect("node").subtree_usage = Usage {
        cost_usd: 1.84,
        cost_complete: true,
        ..Usage::default()
    };
    for width in [60u16, 40, 30, 20] {
        let rows = screen(&app, width, 24);
        let hint = rows.last().expect("a hint row");
        assert!(hint.contains("$1.84"), "at {width} columns: {hint:?}");
    }
}

/// A cost nobody billed is an estimate, and says so with `~`.
#[test]
fn an_estimated_run_cost_is_marked_as_one() {
    let hint_for = |usage: Usage| {
        let mut app = quiet(app_with_transcript(&["hello", "hello back"]));
        let id = viewed(&app);
        app.tree.nodes.get_mut(&id).expect("node").subtree_usage = usage;
        screen(&app, 120, 24).last().expect("a hint row").clone()
    };
    let estimated = hint_for(Usage {
        input_tokens: 39_500,
        cache_read_tokens: 3_210_000,
        cost_complete: false,
        estimated_cost_usd: Some(1.36),
        ..Usage::default()
    });
    assert!(estimated.contains("~$1.36"), "{estimated:?}");
    let billed = hint_for(Usage {
        cost_usd: 0.5,
        cost_complete: true,
        ..Usage::default()
    });
    assert!(
        billed.contains("$0.50") && !billed.contains("~$"),
        "{billed:?}"
    );
}

/// The operator rule: quota exhaustion is never silent. The chip is the
/// last thing the hint row gives up, after the context and the cost.
#[test]
fn a_quota_chip_survives_every_width() {
    let chip = "anthropic weekly limit reached";
    for ascii in [false, true] {
        for width in [160u16, 120, 80, 50] {
            let mut app = quiet(app_with_transcript(&["hello", "hello back"]));
            app.theme = dark(ascii);
            app.quota_alert = Some(chip.into());
            let buffer = draw(&app, width, 24);
            let rows = rows(&buffer);
            let hint = rows.last().expect("a hint row");
            let x = column_of(hint, chip)
                .unwrap_or_else(|| panic!("at {width}: the quota chip is gone: {hint:?}"));
            assert!(
                shows(&buffer[(x, (rows.len() - 1) as u16)], app.theme.failure()),
                "at {width}: the chip is not in failure()"
            );
        }
    }
}

/// Transient feedback takes the hints' place until it has been read.
#[test]
fn a_flash_replaces_the_hints_until_it_expires() {
    let mut app = quiet(app_with_transcript(&["hello", "hello back"]));
    app.flash = Some(aether_tui::app::Flash {
        text: "copied 12 lines".into(),
        until: app.now + chrono::Duration::seconds(2),
    });
    let hint = screen(&app, 120, 24).last().expect("a hint row").clone();
    assert!(
        hint.contains("copied 12 lines") && !hint.contains("shortcuts"),
        "{hint:?}"
    );
    app.now += chrono::Duration::seconds(3);
    let hint = screen(&app, 120, 24).last().expect("a hint row").clone();
    assert!(
        !hint.contains("copied") && hint.contains("? shortcuts"),
        "{hint:?}"
    );
}

#[test]
fn a_disk_hold_stays_visible_until_status_clears_it() {
    use aether_core::disk::LowDisk;
    use aether_protocol::Reply;
    use aether_tui::app::Then;

    for mode in [ThemeMode::Dark, ThemeMode::Light, ThemeMode::Mono] {
        for ascii in [false, true] {
            for width in [80, 120] {
                for board_open in [false, true] {
                    let mut app = quiet(populated_app());
                    app.theme = theme(mode, ascii);
                    app.board.open = board_open;
                    app.input.set_text("draft");
                    app.flash = Some(aether_tui::app::Flash {
                        text: "temporary feedback".into(),
                        until: app.now + chrono::Duration::seconds(3),
                    });
                    let mut status = app.status.clone().expect("status");
                    status.low_disk = Some(LowDisk {
                        path: "/isolated/store".into(),
                        available_bytes: 1 << 30,
                        floor_bytes: 5 << 30,
                        since: app.now,
                    });
                    app.reduce(AppEvent::Reply {
                        then: Then::Status,
                        result: Ok(Reply::Status(status.clone())),
                    });
                    let buffer = draw(&app, width, 24);
                    let rows = rows(&buffer);
                    let footer_width =
                        ui::bottom_width(width, ui::layout::board_mode(width, board_open));
                    let row: String = rows
                        .last()
                        .expect("hint row")
                        .chars()
                        .take(usize::from(footer_width))
                        .collect();
                    let row = row.as_str();
                    assert_eq!(
                        row.trim_end(),
                        format!(
                            "  {} Disk low: new turns held - 1.0 GiB free / 5.0 GiB floor",
                            app.theme.glyphs().needs_you
                        )
                    );
                    let x = column_of(row, "Disk low").expect("disk warning");
                    assert!(shows(&buffer[(x, 23)], app.theme.title()));
                    if ascii {
                        assert!(row.is_ascii());
                    }
                    if mode == ThemeMode::Mono {
                        for x in 0..width {
                            assert_eq!(buffer[(x, 23)].fg, Color::Reset);
                        }
                    }
                    status.low_disk = None;
                    app.reduce(AppEvent::Reply {
                        then: Then::Status,
                        result: Ok(Reply::Status(status)),
                    });
                    assert!(!screen(&app, width, 24).last().unwrap().contains("Disk low"));
                }
            }
        }
    }
}

#[test]
fn disk_hold_survives_find_and_the_fullscreen_pager() {
    use aether_core::disk::LowDisk;
    use aether_tui::ui::views::pager::{PagerSource, PagerView};

    for width in [80, 120] {
        let mut app = quiet(populated_app());
        app.board.open = false;
        app.status.as_mut().expect("status").low_disk = Some(LowDisk {
            path: "/isolated/store".into(),
            available_bytes: 1 << 30,
            floor_bytes: 5 << 30,
            since: app.now,
        });
        app.reduce(ctrl('f'));
        assert!(app.bottom.find.is_some());
        assert!(
            screen(&app, width, 24)
                .last()
                .unwrap()
                .contains("Disk low: new turns held")
        );
        app.bottom
            .view_stack
            .push(Box::new(PagerView::new(PagerSource::Report)));
        assert!(
            screen(&app, width, 24)
                .last()
                .unwrap()
                .contains("Disk low: new turns held")
        );
    }
}
