#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `--ascii` is a glyph-table swap: every authored decoration becomes ASCII
//! while what agents and people wrote reaches the screen unchanged.

use aether_tui::app::{App, AppEvent, TuiOptions};
use aether_tui::prefs::MouseMode;
use aether_tui::testing::{app_with_transcript, loaded_app, populated_app, render_to};
use aether_tui::ui::theme::tokens::{Theme, ThemeSpec};
use aether_tui::ui::transcript::ViewCache;
use aether_tui::ui::views::{self, ViewKind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const VIEWS: [ViewKind; 13] = [
    ViewKind::Model,
    ViewKind::Usage,
    ViewKind::Approvals,
    ViewKind::Memory,
    ViewKind::Schedules,
    ViewKind::Mcp,
    ViewKind::Doctor,
    ViewKind::Report,
    ViewKind::Artifacts,
    ViewKind::Settings,
    ViewKind::Theme,
    ViewKind::Voice,
    ViewKind::Login,
];

fn set_ascii(app: &mut App, ascii: bool) {
    app.theme = Theme::resolve(ThemeSpec {
        ascii,
        ..app.theme.spec()
    });
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.reduce(AppEvent::Key(KeyEvent::new(code, modifiers)));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

/// `text` with each piece of data blanked, so what remains is chrome.
fn chrome(text: &str, data: &[&str]) -> String {
    data.iter().fold(text.to_owned(), |text, original| {
        text.replace(original, "DATA")
    })
}

/// Draw every case in both modes; report the ASCII frames that still paint
/// non-ASCII (views only when `ascii_views`).
fn frame_matrix(views: &[Option<ViewKind>], ascii_views: bool) -> Vec<String> {
    let mut failures = Vec::new();
    for scenario in 0..3 {
        for &view in views {
            for board in [false, true] {
                let mut app = match scenario {
                    0 => App::new(TuiOptions::default()),
                    1 => loaded_app(8, 30, 6),
                    _ => populated_app(),
                };
                app.board.open = board;
                if let Some(kind) = view {
                    let opened = views::open(kind, &app);
                    app.bottom.push_view(opened);
                }
                for ascii in [false, true] {
                    set_ascii(&mut app, ascii);
                    for (width, height) in
                        [(160, 48), (100, 32), (80, 24), (40, 14), (12, 6), (1, 1)]
                    {
                        let text = render_to(&app, width, height);
                        let glyphs: String = text.chars().filter(|c| !c.is_ascii()).collect();
                        if ascii && (view.is_none() || ascii_views) && !glyphs.is_empty() {
                            failures.push(format!(
                                "scenario={scenario} {width}x{height} {view:?} board={board}: {glyphs:?}\n{text}"
                            ));
                        }
                    }
                }
            }
        }
    }
    failures
}

/// Every view is drawn in both modes for robustness; only the bare frame
/// is held to ASCII here (views: see the ignored test below).
#[test]
fn ascii_and_unicode_frame_matrix_covers_empty_loaded_and_approval_states() {
    let views: Vec<_> = std::iter::once(None)
        .chain(VIEWS.into_iter().map(Some))
        .collect();
    let failures = frame_matrix(&views, false);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn ascii_slash_views_paint_no_unicode_chrome() {
    let views: Vec<_> = VIEWS.into_iter().map(Some).collect();
    let failures = frame_matrix(&views, true);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn ascii_popups_shortcuts_and_composer() {
    for draft in [
        "/",
        "/model",
        "@",
        "a very long draft that wraps around the narrow composer",
    ] {
        let mut app = app_with_transcript(&["hello"]);
        set_ascii(&mut app, true);
        type_text(&mut app, draft);
        assert_eq!(app.input.text, draft);
        for width in [160, 80, 40, 12] {
            let frame = render_to(&app, width, 24);
            assert!(frame.is_ascii(), "{draft:?} at {width}:\n{frame}");
        }
    }
    let mut app = app_with_transcript(&["hello"]);
    set_ascii(&mut app, true);
    app.input.set_text("line one\nline two");
    for width in [160, 80, 40, 12] {
        let frame = render_to(&app, width, 24);
        assert!(frame.is_ascii(), "multi-line at {width}:\n{frame}");
    }
    app.input.set_text("");
    type_text(&mut app, "?");
    assert!(app.bottom.shortcuts);
    for width in [160, 80, 40, 12] {
        let frame = render_to(&app, width, 40);
        assert!(frame.is_ascii(), "shortcuts at {width}:\n{frame}");
    }
}

#[test]
fn ascii_preference_precedence_roundtrip_and_errors() {
    use aether_tui::prefs::UiPrefs;
    for stored in [None, Some(false), Some(true)] {
        let prefs = UiPrefs {
            ascii: stored,
            ..Default::default()
        };
        for flag in [false, true] {
            for (env, parsed) in [
                (None, None),
                (Some(""), None),
                (Some(" true "), Some(true)),
                (Some("ON"), Some(true)),
                (Some("yes"), Some(true)),
                (Some("1"), Some(true)),
                (Some("false"), Some(false)),
                (Some("OFF"), Some(false)),
                (Some("no"), Some(false)),
                (Some("0"), Some(false)),
            ] {
                assert_eq!(
                    prefs.ascii(flag, env).unwrap(),
                    flag || parsed.or(stored).unwrap_or(false)
                );
            }
        }
        assert!(
            prefs
                .ascii(false, Some("perhaps"))
                .unwrap_err()
                .contains("AETHER_ASCII")
        );
        let dir = tempfile::tempdir().unwrap();
        prefs.save(dir.path()).unwrap();
        assert_eq!(UiPrefs::load(dir.path()).unwrap(), prefs);
    }
    assert!(toml::from_str::<UiPrefs>("ascii = 'yes'").is_err());
    assert!(toml::from_str::<UiPrefs>("ascii = 1").is_err());
    assert_eq!(
        toml::from_str::<UiPrefs>("theme = 'mono'").unwrap().ascii,
        None
    );
}

#[test]
fn ascii_options_and_theme_changes_keep_interactivity() {
    let mut app = App::new(TuiOptions {
        ascii: true,
        ..Default::default()
    });
    assert!(app.theme.ascii());
    assert_eq!(
        app.options.mouse,
        MouseMode::Scroll,
        "capture defaults on independently of ASCII"
    );
    for command in ["/theme dawn", "/theme mono", "/theme dark", "/theme auto"] {
        app.input.set_text(command);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.theme.ascii(), "{command}");
        assert!(app.input.text.is_empty(), "{command} was not taken");
    }
    type_text(&mut app, "?");
    assert!(app.bottom.shortcuts);
    assert!(render_to(&app, 100, 30).is_ascii());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.bottom.shortcuts);
}

#[test]
fn ascii_preserves_unicode_variables_in_chat_tools_approvals_and_titles() {
    use aether_core::model::ModelRef;
    use aether_tui::app::transcript::{CallStatus, Cell, CellId, CellModel, CellRevision};
    let mut app = populated_app();
    set_ascii(&mut app, true);
    let id = app.selected.unwrap();
    let name = "名·→▎";
    let path = "src/界·→▎.rs";
    let output = "結果·→▎";
    let model = "模型·→▎";
    let draft = "draft 界·→▎";
    let node = app.tree.nodes.get_mut(&id).unwrap();
    node.agent.name = name.into();
    node.agent.model = ModelRef::new("mock", model);
    // A cell id the transcript's own counter never reaches.
    app.transcripts.get_mut(&id).unwrap().cells.push(Cell {
        id: CellId(u64::MAX),
        revision: CellRevision(0),
        at: aether_core::time::now(),
        model: CellModel::Exec {
            call: None,
            command: format!("cat {path}"),
            status: CallStatus::Succeeded,
            output: output.into(),
            exit: Some(0),
            duration_ms: Some(10),
            is_search: false,
            artifacts: Vec::new(),
        },
    });
    for approval in &mut app.approvals {
        approval.description = format!("read {path}");
        approval.argument = Some(path.into());
    }
    let data = [name, path, output, model];
    let staged = content_frame(&app, 200, 60);
    assert!(app.due_approval().is_some(), "{staged}");
    for original in data {
        assert!(staged.contains(original), "missing {original:?}:\n{staged}");
    }
    assert!(chrome(&staged, &data).is_ascii(), "decoration:\n{staged}");

    // Answered, the composer comes back with the person's draft in it.
    app.approvals.clear();
    app.input.set_text(draft);
    let composing = content_frame(&app, 200, 60);
    assert!(composing.contains(draft), "{composing}");
    assert!(
        chrome(&composing, &[name, path, output, model, draft]).is_ascii(),
        "decoration:\n{composing}"
    );
}

#[test]
fn ascii_chat_copy_preserves_user_glyphs_and_leading_pipe() {
    use aether_tui::app::transcript::CellModel;
    use aether_tui::ui::cells::ChatCell;
    let original = "| 界·→▎ e\u{301}";
    for ascii in [false, true] {
        let mut app = app_with_transcript(&[original]);
        set_ascii(&mut app, ascii);
        let text = content_frame(&app, 160, 40);
        assert!(text.contains(original), "{text}");
        let transcript = &app.transcripts[&app.selected.unwrap()];
        let user = transcript
            .cells
            .iter()
            .find(|cell| matches!(cell.model, CellModel::User { .. }))
            .expect("a user cell");
        assert_eq!(user.model.copy_source(), Some(original));
    }
}

#[test]
fn ascii_markdown_chrome_and_widths_preserve_literal_symbols() {
    use aether_tui::ui::cells::text;
    use aether_tui::ui::markdown;
    use ratatui::text::Span;
    use unicode_width::UnicodeWidthStr;
    let theme = Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    });
    let original = "界·→▎";
    let source = format!(
        "# heading\n\n> {original}\n\n- [x] done\n- [ ] pending\n\n---\n\n| a | b |\n|---|---|\n| {original} | value |\n\n`{original}`\n"
    );
    let lines = markdown::render(&source, 60, theme.text(), &theme);
    let rendered = text::plain(&lines).join("\n");
    assert!(rendered.contains(original), "{rendered}");
    assert!(chrome(&rendered, &[original]).is_ascii(), "{rendered}");
    for width in 0..12 {
        let cut = text::truncate(vec![Span::raw("界e\u{301}long text")], width, &theme);
        let shown: String = cut.iter().map(|s| s.content.as_ref()).collect();
        assert!(shown.width() <= width, "{width}: {shown:?}");
        assert!(chrome(&shown, &["界", "e\u{301}"]).is_ascii(), "{shown:?}");
    }
}

#[test]
fn ascii_board_inspector_over_every_status() {
    use aether_core::agent::{AgentStatus, AgentStatusKind, BlockReason, WaitCondition};
    use aether_core::model::{CapabilityProfile, Usage};
    let statuses = [
        AgentStatus::Created,
        AgentStatus::Ready,
        AgentStatus::Running,
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        },
        AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: "waiting".into(),
            },
        },
        AgentStatus::Completed { summary: None },
        AgentStatus::Failed {
            error: "failed".into(),
        },
        AgentStatus::Cancelled {
            reason: Some("cancelled".into()),
        },
        AgentStatus::Imported {
            was: AgentStatusKind::Completed,
        },
    ];
    for ascii in [false, true] {
        for status in &statuses {
            let mut app = loaded_app(4, 30, 2);
            set_ascii(&mut app, ascii);
            let id = app.selected.unwrap();
            let node = app.tree.nodes.get_mut(&id).unwrap();
            node.agent.status = status.clone();
            node.agent.stats.usage = Usage {
                cost_usd: 0.2,
                cost_complete: false,
                ..Default::default()
            };
            app.models.push(aether_protocol::ModelInfo {
                aliases: Vec::new(),
                model: node.agent.model.clone(),
                name: "model".into(),
                provider_name: "mock".into(),
                capabilities: CapabilityProfile {
                    context_window: 100_000,
                    ..Default::default()
                },
                configured: true,
                auth_source: None,
                is_default: false,
            });
            app.context_tokens.insert(id, 75_000);
            app.board.open = true;
            let text = render_to(&app, 160, 48);
            assert!(text.contains("75%"), "{status:?}: no context bar\n{text}");
            assert!(text.contains("75k of 100k"), "{status:?}:\n{text}");
            if ascii {
                assert!(text.is_ascii(), "{status:?}:\n{text}");
                assert!(text.contains("===-"), "{status:?}: bar\n{text}");
            } else {
                assert!(text.contains("━━━"), "{status:?}: bar\n{text}");
            }
        }
    }
}

// Wide graphemes occupy a continuation cell; it is not an extra data space.
fn content_frame(app: &App, width: u16, height: u16) -> String {
    use ratatui::{Terminal, backend::TestBackend};
    use unicode_width::UnicodeWidthStr;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| aether_tui::ui::render(frame, app, &mut ViewCache::default()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..height {
        let mut x = 0;
        while x < width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x += symbol.width().max(1) as u16;
        }
        text.push('\n');
    }
    text
}

#[test]
fn ascii_costs_and_title_have_safe_defaults() {
    use aether_tui::app::transcript::Cost;
    use aether_tui::ui::board::cost_text;
    use aether_tui::ui::cells::text::money;
    // An unknown price prints nothing, so no dash glyph can leak either.
    assert_eq!(cost_text(None), "");
    for estimated in [false, true] {
        let cost = Cost {
            usd: 0.4,
            estimated,
            basis: Default::default(),
        };
        let tilde = if estimated { "~" } else { "" };
        assert_eq!(cost_text(Some(cost)), format!("{tilde}$0.40"));
        assert_eq!(money(cost), format!("{tilde}$0.40"));
    }
}

#[test]
fn ascii_terminal_title_is_ascii_while_agents_run() {
    let mut app = populated_app();
    set_ascii(&mut app, true);
    for tick in 0..20 {
        app.tick = tick;
        let title = app.terminal_title();
        assert!(title.contains("running"), "{title}");
        assert!(title.is_ascii(), "{title}");
    }
}

#[test]
fn ascii_registry_descriptions_cannot_add_uncovered_unicode_chrome() {
    use aether_tui::commands::{COMMANDS, KEYS};
    for command in COMMANDS {
        assert_eq!(command.display_about(false), command.about);
        assert!(command.display_about(true).is_ascii(), "{}", command.name);
    }
    for key in KEYS {
        assert_eq!(key.display_keys(false), key.keys);
        assert_eq!(key.display_action(false), key.action);
        assert!(key.display_keys(true).is_ascii(), "{}", key.keys);
        assert!(key.display_action(true).is_ascii(), "{}", key.action);
    }
}

#[test]
fn ascii_find_bar_keeps_matched_data() {
    let mut app = app_with_transcript(&[]);
    set_ascii(&mut app, true);
    let original = "needle·→▎";
    aether_tui::testing::push_user_message(
        &mut app,
        &format!("{} {original} {}", "prefix ".repeat(30), "tail ".repeat(40)),
    );
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(&mut app, "needle");
    let find = app.bottom.find.as_ref().expect("Ctrl+F opens the find bar");
    assert_eq!(find.query, "needle");
    assert_eq!(find.matches.len(), 1, "{:?}", find.matches);
    for width in [180, 80, 40] {
        let text = content_frame(&app, width, 44);
        assert!(text.contains("needle"), "{text}");
        assert!(
            chrome(&text, &[original, "·", "→", "▎"]).is_ascii(),
            "{text}"
        );
    }
}

#[test]
fn ascii_populated_report_preserves_unicode_data_without_unicode_decoration() {
    use aether_core::{event::Event, model::Usage, report};
    let events: Vec<Event> = serde_json::from_str(include_str!(
        "../../aether-core/tests/fixtures/run_log.json"
    ))
    .unwrap();
    let run = events.iter().find_map(|event| event.run).unwrap();
    let mut report = report::build(run, &events, aether_core::time::now());
    report.body.usage = Usage {
        cost_usd: 0.4,
        cost_complete: false,
        ..Default::default()
    };
    let mut app = populated_app();
    set_ascii(&mut app, true);
    app.report.report = Some(Box::new(report.clone()));
    let opened = views::open(ViewKind::Report, &app);
    app.bottom.push_view(opened);
    for width in [160, 100, 80, 40, 12] {
        let text = render_to(&app, width, 44);
        assert!(text.is_ascii(), "{width}:\n{text}");
    }
    let original = "界·→≥▎";
    report.body.objective = Some(original.into());
    report.body.agents[0].name = original.into();
    report.body.agents[0].outcome = report::Ended::Completed {
        summary: Some(original.into()),
    };
    report.body.failures[0].error = original.into();
    app.report.report = Some(Box::new(report));
    let text = content_frame(&app, 240, 60);
    assert!(text.matches(original).count() >= 4, "{text}");
    assert!(chrome(&text, &[original]).is_ascii(), "{text}");
    assert!(text.contains(">= $0.40 (some cost unknown)"), "{text}");
}
