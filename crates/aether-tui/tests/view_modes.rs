#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use aether_core::tool::ToolOutcome;
use aether_tui::app::transcript::{
    CallStatus, CellModel, ChildOutcome, CompactionState, NoticeKind, UserFrom,
};
use aether_tui::app::{App, AppEvent, Effect};
use aether_tui::testing::{app_with_transcript, render_to};
use aether_tui::view_modes::{self, Catalog, Category, Definition, Density, Mode, Policy};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const BRIEF: &str = r#"version = 1
label = "Brief"
description = "Coordination without tool detail"
density = "compact"
show = ["user_text", "assistant_text", "delegation", "agent_message", "completion", "status"]
"#;

fn submit(app: &mut App, command: &str) -> Vec<Effect> {
    app.input.set_text(command);
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )))
}

fn own_event(app: &mut App, kind: aether_core::event::EventKind) {
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft};
    use aether_core::id::EventId;
    let agent = app.selected.unwrap();
    let seq = app.transcripts[&agent].max_seq + 1;
    let event = Event::from_draft(
        EventDraft::new(Actor::System, kind).agent(agent),
        EventId::new(),
        seq,
        app.now,
    );
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
}

#[test]
fn mode_only_completion_preserves_board_activity() {
    use aether_core::event::EventKind;
    use aether_core::task::TaskResult;
    use aether_tui::app::board::InspectorModel;

    let mut app = app_with_transcript(&["go", "The result"]);
    let agent = app.selected.unwrap();
    let before = InspectorModel::build(&app, agent).unwrap().last_activity;
    own_event(
        &mut app,
        EventKind::TaskCompleted {
            result: TaskResult::summary("The result"),
        },
    );
    assert_eq!(app.view_mode, "standard");
    assert_eq!(
        InspectorModel::build(&app, agent).unwrap().last_activity,
        before
    );
    let transcript = &app.transcripts[&agent];
    assert_eq!(transcript.named_only.len(), 1);
    assert!(
        transcript
            .presentation_cells(true)
            .any(|cell| matches!(cell.model, CellModel::ChildResult { .. }))
    );
    assert!(
        !transcript
            .presentation_cells(false)
            .any(|cell| transcript.named_only.contains(&cell.id))
    );
    submit(&mut app, "/focus");
    assert_eq!(
        InspectorModel::build(&app, agent).unwrap().last_activity,
        before
    );
}

#[test]
fn mode_only_grouped_errors_preserve_board_activity() {
    use aether_core::event::EventKind;
    use aether_core::id::ToolCallId;
    use aether_core::tool::{SideEffectClass, ToolCall};
    use aether_tui::app::board::InspectorModel;

    let mut app = app_with_transcript(&["go", "The result"]);
    let agent = app.selected.unwrap();
    let call = ToolCallId::new();
    let requested_at = app.now;
    own_event(
        &mut app,
        EventKind::ToolRequested {
            call: ToolCall {
                id: call,
                provider_call_id: "read".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "notes.txt"}),
                agent_id: agent,
                task_id: None,
                invocation_id: None,
                resources: vec![],
                idempotency_key: None,
                requested_at,
            },
            side_effect: SideEffectClass::ReadOnly,
        },
    );
    let before_failure = InspectorModel::build(&app, agent).unwrap().last_activity;
    own_event(
        &mut app,
        EventKind::ToolSucceeded {
            call,
            outcome: ToolOutcome::error("not found"),
        },
    );
    assert_eq!(
        InspectorModel::build(&app, agent).unwrap().last_activity,
        before_failure
    );
    let transcript = &app.transcripts[&agent];
    assert_eq!(app.view_mode, "standard");
    assert_eq!(transcript.named_only.len(), 1);
    assert!(transcript.presentation_cells(true).any(|cell| matches!(
        &cell.model,
        CellModel::Called { status: CallStatus::Failed, error: Some(error), .. } if error == "not found"
    )));
    assert!(
        !transcript
            .presentation_cells(false)
            .any(|cell| transcript.named_only.contains(&cell.id))
    );
    submit(&mut app, "/focus");
    assert_eq!(
        InspectorModel::build(&app, agent).unwrap().last_activity,
        before_failure
    );
}

fn app_with_tool(outcome: ToolOutcome) -> App {
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::{EventId, ToolCallId};
    use aether_core::tool::{SideEffectClass, ToolCall};
    let mut app = app_with_transcript(&[]);
    let agent = app.selected.unwrap();
    let id = ToolCallId::new();
    let events = [
        EventKind::ToolRequested {
            call: ToolCall {
                id,
                provider_call_id: "fixture".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "secret tool"}),
                agent_id: agent,
                task_id: None,
                invocation_id: None,
                resources: vec![],
                idempotency_key: None,
                requested_at: app.now,
            },
            side_effect: SideEffectClass::ReadOnly,
        },
        EventKind::ToolSucceeded { call: id, outcome },
    ];
    for kind in events {
        let seq = app.transcripts[&agent].max_seq + 1;
        let event = Event::from_draft(
            EventDraft::new(Actor::System, kind).agent(agent),
            EventId::new(),
            seq,
            app.now,
        );
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
    }
    aether_tui::testing::push_message(
        &mut app,
        agent,
        aether_core::model::MessageRole::Assistant,
        "Visible answer",
    );
    app
}

#[test]
fn strict_schema_rejects_bad_definitions_as_a_whole() {
    let good = Definition::parse(BRIEF).unwrap();
    assert_eq!(good.density, Density::Compact);
    assert!(Definition::parse("version=1\nshow=[]").is_ok());
    for text in [
        "",
        "version=2\nshow=[]",
        "version=1",
        "version=1\nshow=['bogus']",
        "version=1\nshow=['tool','tool']",
        "version=1\nshow=[]\nfuture=true",
        "version=1\nshow=[]\ndensity='tiny'",
        "version=1\nshow=[]\n[scope]\nfoo='x'",
        "version=1\nshow=[]\n[scope]",
        "version=1\nshow=[]\n[scope]\nrole=' '",
        "version=1\nshow=[]\nlabel='bad\nlabel'",
        "version=1\nshow=[]\nlabel=\"bad\\nlabel\"",
        "version=1\nshow=[]\nlabel=\"\\u001b\"",
        "version=1\nshow=[]\ndescription=\"\\u2028\"",
        "version='1'\nshow=[]",
    ] {
        assert!(Definition::parse(text).is_err(), "{text}");
    }
    assert!(Definition::parse(&format!("version=1\nshow=[]\nlabel='{}'", "x".repeat(65))).is_err());
}

#[test]
fn scope_is_exact_and_anded() {
    let d = Definition::parse(&format!(
        "{BRIEF}\n[scope]\nname='coordinator'\nrole='supervisor'"
    ))
    .unwrap();
    assert!(d.matches(Some("coordinator"), Some("supervisor")));
    assert!(!d.matches(Some("Coordinator"), Some("supervisor")));
    assert!(!d.matches(Some("coordinator"), Some("worker")));
    assert!(!d.matches(None, Some("supervisor")));
}

#[test]
fn discovery_keeps_valid_files_and_persistent_errors() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(view_modes::discover(temp.path()), Catalog::default());
    let dir = temp.path().join("modes");
    std::fs::create_dir(&dir).unwrap();
    for (name, text) in [
        ("brief.toml", BRIEF),
        ("bad.toml", "nope"),
        ("focus.toml", BRIEF),
        ("q.toml", BRIEF),
        ("reload.toml", BRIEF),
        ("Bad.toml", BRIEF),
        ("ignore.txt", "nope"),
    ] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    std::fs::write(
        dir.join("huge.toml"),
        format!("{BRIEF}#{}", "x".repeat(65537 - BRIEF.len() - 1)),
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("brief.toml"), dir.join("link.toml")).unwrap();
    std::fs::create_dir(dir.join("directory.toml")).unwrap();
    let catalog = view_modes::discover(temp.path());
    assert_eq!(
        catalog
            .ordered()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["standard", "focus", "brief"]
    );
    let diagnostics = catalog
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for name in [
        "bad.toml",
        "focus.toml",
        "q.toml",
        "reload.toml",
        "Bad.toml",
        "huge.toml",
        "directory.toml",
    ] {
        assert!(diagnostics.contains(name), "{diagnostics}");
    }
    assert!(!diagnostics.contains("ignore.txt"));
    assert!(diagnostics.contains("exceeds 64 KiB"));
    #[cfg(unix)]
    assert!(diagnostics.contains("link.toml"));
}

#[test]
fn file_count_limit_rejects_external_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("modes");
    std::fs::create_dir(&dir).unwrap();
    for index in 0..129 {
        std::fs::write(dir.join(format!("mode{index}.toml")), BRIEF).unwrap();
    }
    let catalog = view_modes::discover(temp.path());
    assert_eq!(catalog.modes.len(), 2);
    assert!(
        catalog.diagnostics[0]
            .reason
            .contains("external catalog rejected")
    );
}

#[test]
fn invalid_definitions_are_visible_after_attach_and_remain_in_the_picker() {
    let mut app = app_with_transcript(&[]);
    app.view_modes.diagnostics.push(view_modes::Diagnostic {
        source: "broken.toml".into(),
        reason: "unknown field future".into(),
    });
    app.reduce(AppEvent::Connected);
    assert!(render_to(&app, 120, 30).contains("view mode notice"));
    submit(&mut app, "/mode");
    let screen = render_to(&app, 120, 30);
    assert!(screen.contains("broken.toml"));
    assert!(screen.contains("unknown field future"));
}

#[test]
fn launch_selection_obeys_precedence_without_saving() {
    let catalog = Catalog::default();
    let resolve = |flag, env: Option<&str>, saved| {
        catalog
            .resolve(flag, env.map(std::ffi::OsStr::new), saved)
            .unwrap()
            .0
    };
    assert_eq!(resolve(None, None, None), "standard");
    assert_eq!(resolve(None, None, Some("focus")), "focus");
    assert_eq!(resolve(None, Some("standard"), Some("focus")), "standard");
    assert_eq!(resolve(Some("focus"), Some("missing"), None), "focus");
    assert_eq!(resolve(None, Some("  "), Some("focus")), "focus");
    assert!(catalog.resolve(Some("missing"), None, None).is_err());
    assert!(
        catalog
            .resolve(None, Some(std::ffi::OsStr::new("missing")), None)
            .is_err()
    );
    assert!(
        catalog
            .resolve(None, None, Some("missing"))
            .unwrap()
            .1
            .is_some()
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(
            catalog
                .resolve(None, Some(std::ffi::OsStr::from_bytes(&[255])), None)
                .is_err()
        );
    }
}

fn exec(status: CallStatus) -> CellModel {
    CellModel::Exec {
        call: None,
        command: "secret tool".into(),
        status,
        output: "output".into(),
        exit: None,
        duration_ms: None,
        is_search: false,
        artifacts: vec![],
    }
}

#[test]
fn filtering_never_suppresses_decisions_or_failures() {
    let empty = Definition::parse("version=1\nshow=[]").unwrap();
    let p = Policy {
        definition: Some(&empty),
    };
    for status in [CallStatus::Running, CallStatus::Succeeded] {
        assert!(!p.shows(&exec(status)));
    }
    for status in [
        CallStatus::Awaiting,
        CallStatus::Failed,
        CallStatus::Denied,
        CallStatus::Cancelled,
    ] {
        assert!(p.shows(&exec(status)));
    }
    for model in [
        CellModel::Error {
            message: "bad".into(),
            cause: None,
            hint: None,
        },
        CellModel::Compaction(CompactionState::Failed {
            error: "bad".into(),
        }),
        CellModel::Notice {
            kind: NoticeKind::Store,
            text: "disk bad".into(),
        },
        CellModel::Question {
            asker: aether_core::id::AgentId::new(),
            asker_name: "coordinator".into(),
            text: "Proceed?".into(),
            state: aether_tui::app::transcript::QuestionState::Pending,
            seq: 1,
        },
    ] {
        assert!(p.shows(&model));
        assert!(!p.compact(&model));
    }
    assert!(!p.shows(&CellModel::Assistant {
        text: "prose".into()
    }));
    let focus = Catalog::default();
    let p = Policy {
        definition: Some(&focus.modes["focus"].definition),
    };
    assert!(p.shows(&CellModel::Sent {
        to: "tester".into(),
        text: "done".into()
    }));
    assert!(p.shows_category(Category::AssistantText));
    assert!(!p.shows_category(Category::Reasoning));
    assert_eq!(
        view_modes::category(&CellModel::User {
            from: UserFrom::Message {
                from: "tester".into()
            },
            text: "done".into()
        }),
        Category::AgentMessage
    );
}

#[test]
fn every_category_can_be_selected_independently() {
    use aether_tui::app::transcript::{ApprovalScope, MemoryNote};
    let agent = aether_core::id::AgentId::new();
    let at = chrono::Utc::now();
    let models = [
        (
            Category::UserText,
            CellModel::User {
                from: UserFrom::Operator,
                text: "hello".into(),
            },
        ),
        (
            Category::AssistantText,
            CellModel::Assistant {
                text: "answer".into(),
            },
        ),
        (
            Category::Delegation,
            CellModel::Spawned {
                agent,
                name: "tester".into(),
                role: "review".into(),
                model: "mock".into(),
                effort: None,
                objective: "verify".into(),
            },
        ),
        (
            Category::AgentMessage,
            CellModel::Sent {
                to: "tester".into(),
                text: "ready".into(),
            },
        ),
        (
            Category::Completion,
            CellModel::ChildResult {
                agent,
                name: "tester".into(),
                role: "review".into(),
                outcome: ChildOutcome::Finished,
                elapsed_ms: 0,
                cost_with_children: None,
                result: "done".into(),
                descendants: vec![],
            },
        ),
        (
            Category::Status,
            CellModel::Waiting {
                on: vec![],
                since: at,
                ended: None,
            },
        ),
        (
            Category::Question,
            CellModel::Question {
                asker: agent,
                asker_name: "tester".into(),
                text: "ready?".into(),
                state: aether_tui::app::transcript::QuestionState::Pending,
                seq: 1,
            },
        ),
        (
            Category::Approval,
            CellModel::ApprovalRecord {
                approval: aether_core::id::ApprovalId::new(),
                approved: false,
                what: "run command".into(),
                scope: ApprovalScope::Once,
            },
        ),
        (
            Category::Error,
            CellModel::Error {
                message: "failed".into(),
                cause: None,
                hint: None,
            },
        ),
        (Category::Tool, exec(CallStatus::Succeeded)),
        (
            Category::Reasoning,
            CellModel::Reasoning {
                title: None,
                text: "thought".into(),
                duration_ms: None,
            },
        ),
        (
            Category::Memory,
            CellModel::Memory(MemoryNote::Recalled { names: vec![] }),
        ),
        (
            Category::Compaction,
            CellModel::Compaction(CompactionState::Active),
        ),
        (
            Category::Usage,
            CellModel::Separator {
                elapsed_ms: 0,
                cost: None,
                agents: 1,
                finished_at: at,
            },
        ),
    ];
    let mut definition = Definition::parse("version=1\nshow=[]").unwrap();
    for (expected, _) in &models {
        definition.show = vec![*expected];
        let policy = Policy {
            definition: Some(&definition),
        };
        for (category, model) in &models {
            assert_eq!(view_modes::category(model), *category);
            assert_eq!(
                policy.shows(model),
                category == expected
                    || matches!(
                        category,
                        Category::Question | Category::Approval | Category::Error
                    ),
                "{expected:?}: {category:?}"
            );
        }
    }
}

#[test]
fn slash_toggle_settings_persistence_and_reload() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app_with_transcript(&["hello"]);
    app.options.prefs_dir = Some(temp.path().into());
    app.options.definitions_dir = Some(temp.path().into());
    app.view_modes.modes.insert(
        "brief".into(),
        Mode {
            name: "brief".into(),
            source: "fixture".into(),
            definition: Definition::parse(BRIEF).unwrap(),
        },
    );
    submit(&mut app, "/brief");
    assert_eq!(app.view_mode, "brief");
    assert_eq!(
        aether_tui::prefs::UiPrefs::load(temp.path())
            .unwrap()
            .view_mode
            .as_deref(),
        Some("brief")
    );
    submit(&mut app, "/brief");
    assert_eq!(app.view_mode, "standard");
    submit(&mut app, "/mode brief");
    submit(&mut app, "/mode brief");
    assert_eq!(app.view_mode, "brief");
    assert!(matches!(
        submit(&mut app, "/mode reload").as_slice(),
        [Effect::ReloadViewModes(_)]
    ));
    assert!(submit(&mut app, "/mode reload").is_empty());
    app.reduce(AppEvent::ViewModesLoaded(Catalog::default()));
    assert_eq!(app.view_mode, "standard");
    assert!(!app.view_modes.diagnostics.is_empty());
    assert_eq!(
        aether_tui::prefs::UiPrefs::load(temp.path())
            .unwrap()
            .view_mode
            .as_deref(),
        Some("brief")
    );
    submit(&mut app, "/settings");
    assert!(render_to(&app, 120, 40).contains("View mode"));
}

#[test]
fn settings_edit_keys_and_view_mode_without_losing_either_preference() {
    use aether_tui::keys::KeyAction;
    use aether_tui::prefs::UiPrefs;
    use aether_tui::ui::views::ViewKind;

    let temp = tempfile::tempdir().unwrap();
    let mut app = app_with_transcript(&["hello"]);
    app.options.prefs_dir = Some(temp.path().into());
    app.options.definitions_dir = Some(temp.path().into());
    submit(&mut app, "/focus");
    submit(&mut app, "/settings");
    let press =
        |app: &mut App, code| app.reduce(AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.bottom.top_view().and_then(|v| v.kind()),
        Some(ViewKind::Keys)
    );
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::F(6));
    assert!(app.keys.matches(
        KeyAction::Agents,
        KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE)
    ));
    let prefs = UiPrefs::load(temp.path()).unwrap();
    assert_eq!(prefs.view_mode.as_deref(), Some("focus"));
    assert_eq!(prefs.keys["agents"].as_str(), Some("f6"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('6'));
    press(&mut app, KeyCode::Left);
    assert_eq!(app.view_mode, "standard");
    press(&mut app, KeyCode::Right);
    assert_eq!(app.view_mode, "focus");
    let prefs = UiPrefs::load(temp.path()).unwrap();
    assert_eq!(prefs.view_mode.as_deref(), Some("focus"));
    assert_eq!(prefs.keys["agents"].as_str(), Some("f6"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.bottom.top_view().and_then(|v| v.kind()),
        Some(ViewKind::Modes)
    );
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('?'));
    let help = render_to(&app, 120, 70);
    assert!(help.contains("View modes"), "{help}");
    assert!(help.contains("F6"), "{help}");
    assert!(help.contains("/focus"), "{help}");
}

#[test]
fn picker_cursor_remains_usable_when_reload_removes_rows() {
    let mut app = app_with_transcript(&[]);
    app.view_modes.modes.insert(
        "brief".into(),
        Mode {
            name: "brief".into(),
            source: "fixture".into(),
            definition: Definition::parse(BRIEF).unwrap(),
        },
    );
    submit(&mut app, "/mode");
    for _ in 0..5 {
        app.reduce(AppEvent::Key(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
    }
    app.reduce(AppEvent::ViewModesLoaded(Catalog::default()));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.view_mode, "focus");
}

#[test]
fn compact_rows_are_single_line_and_grapheme_safe() {
    let theme = aether_tui::ui::cells::plain_theme();
    let model = CellModel::Sent {
        to: "tester".into(),
        text: "👩‍💻 界 é\nnext".into(),
    };
    for width in 0..32 {
        let line = aether_tui::ui::cells::compact::line(&model, width, &theme)
            .unwrap()
            .to_string();
        assert!(unicode_width::UnicodeWidthStr::width(line.as_str()) <= usize::from(width));
        assert!(!line.contains('\n'));
        assert!(!line.ends_with('\u{200d}'));
    }
    let completion = CellModel::ChildResult {
        agent: aether_core::id::AgentId::new(),
        name: "tester".into(),
        role: "review".into(),
        outcome: ChildOutcome::Finished,
        elapsed_ms: 0,
        cost_with_children: None,
        result: "CI passed".into(),
        descendants: vec![],
    };
    assert_eq!(
        aether_tui::ui::cells::compact::line(&completion, 80, &theme)
            .unwrap()
            .to_string(),
        "• tester finished: CI passed"
    );
    let ascii = aether_tui::ui::theme::Theme::resolve(aether_tui::ui::theme::ThemeSpec {
        ascii: true,
        reduced_motion: true,
        ..Default::default()
    });
    let line = aether_tui::ui::cells::compact::line(&completion, 20, &ascii)
        .unwrap()
        .to_string();
    assert!(line.is_ascii());
    assert!(line.starts_with("* "));
    assert!(line.ends_with('~'));
}

#[test]
fn compact_notices_convert_only_authored_ascii_separators() {
    use aether_tui::app::transcript::{NoticeSeparator, NoticeText};
    let theme = aether_tui::ui::theme::Theme::resolve(aether_tui::ui::theme::ThemeSpec {
        ascii: true,
        ..Default::default()
    });
    for (separator, expected) in [
        (NoticeSeparator::Dot, "* Continued | summary carried over"),
        (
            NoticeSeparator::Arrow,
            "* Continued -> summary carried over",
        ),
    ] {
        let model = CellModel::Notice {
            kind: NoticeKind::Handoff,
            text: NoticeText::joined(&["Continued", "summary carried over"], separator),
        };
        let line = aether_tui::ui::cells::compact::line(&model, 80, &theme)
            .unwrap()
            .to_string();
        assert!(line.is_ascii(), "{line}");
        assert_eq!(line, expected);
    }
    let model = CellModel::Notice {
        kind: NoticeKind::Handoff,
        text: NoticeText::joined(&["User's · → 界", "continued"], NoticeSeparator::Dot),
    };
    assert_eq!(
        aether_tui::ui::cells::compact::line(&model, 80, &theme)
            .unwrap()
            .to_string(),
        "* User's · → 界 | continued"
    );
}

#[test]
fn compact_messages_strip_whole_terminal_sequences_with_bounded_work() {
    let theme = aether_tui::ui::cells::plain_theme();
    for text in [
        "\u{1b}[31mResult\u{1b}[0m",
        "\u{1b}]8;;https://example.com\u{7}Result\u{1b}]8;;\u{7}",
        "\u{1b}]title\u{1b}\\Result",
        "\u{9b}31mResult\u{9b}0m",
        "Result\u{1b}[31",
        "Result\u{1b}]unfinished",
        "Result\u{1b}",
    ] {
        let model = CellModel::Sent {
            to: "tester".into(),
            text: text.into(),
        };
        let line = aether_tui::ui::cells::compact::line(&model, 80, &theme)
            .unwrap()
            .to_string();
        assert_eq!(line, "• To tester: Result", "{text:?}");
    }
    for text in [
        format!("\u{1b}]{}\u{7}Result", "payload".repeat(1_000_000)),
        format!("\u{1b}[{}mResult", "1;".repeat(1_000_000)),
        format!("e{}Result", "\u{301}".repeat(1_000_000)),
    ] {
        let model = CellModel::Sent {
            to: "tester".into(),
            text,
        };
        let line = aether_tui::ui::cells::compact::line(&model, 80, &theme)
            .unwrap()
            .to_string();
        assert_eq!(line, "• To tester: …");
    }
}

#[test]
fn named_mode_capture_hides_tools_and_details_reveals_retained_data() {
    let mut app = app_with_tool(ToolOutcome::ok("output"));
    submit(&mut app, "/focus");
    for width in [40, 80, 120, 160] {
        let frame = render_to(&app, width, 30);
        assert!(!frame.contains("secret tool"), "{frame}");
    }
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert!(render_to(&app, 120, 30).contains("secret tool"));
}

#[test]
fn rebound_details_key_reveals_retained_cells_in_a_named_mode() {
    let mut app = app_with_tool(ToolOutcome::ok("output"));
    app.keys
        .assign(aether_tui::keys::KeyAction::Details, "f2".parse().unwrap())
        .unwrap();
    submit(&mut app, "/focus");
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert!(!app.details);
    assert!(!render_to(&app, 120, 30).contains("secret tool"));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::F(2),
        KeyModifiers::NONE,
    )));
    assert!(app.details);
    assert!(render_to(&app, 120, 30).contains("secret tool"));
}

#[test]
fn named_projection_rebuilds_after_eviction_followed_by_more_events() {
    use aether_core::event::EventKind;
    use aether_core::model::{ContentBlock, MessageRole};
    use aether_tui::app::transcript::ScrollIntent;
    use aether_tui::testing::render_with;
    use aether_tui::ui::transcript::ViewCache;

    let mut app = app_with_transcript(&[]);
    app.view_mode = "focus".into();
    let agent = app.selected.unwrap();
    let reply = |seq| EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![if seq <= 2_501 {
            ContentBlock::Thinking {
                text: "hidden reasoning".into(),
                signature: None,
                redacted: false,
            }
        } else {
            ContentBlock::text(format!("window reply {seq}"))
        }],
        invocation: None,
    };
    for seq in 1..=5_000 {
        own_event(&mut app, reply(seq));
    }
    let mut cache = ViewCache::default();
    app.scroll_log.push(agent, ScrollIntent::Top);
    assert!(render_with(&app, &mut cache, 120, 30).contains("window reply 2502"));
    for seq in 5_001..=5_003 {
        own_event(&mut app, reply(seq));
    }
    app.scroll_log.push(agent, ScrollIntent::Latest);
    let screen = render_with(&app, &mut cache, 120, 30);
    assert!(screen.contains("window reply 5003"), "{screen}");
    app.scroll_log.push(agent, ScrollIntent::Top);
    let screen = render_with(&app, &mut cache, 120, 30);
    assert!(screen.contains("window reply 2502"), "{screen}");
}

#[test]
fn supplemental_completion_can_be_recorded_again_after_its_window_is_evicted() {
    use aether_core::event::EventKind;
    use aether_core::model::{ContentBlock, MessageRole};
    use aether_core::task::TaskResult;

    let mut app = app_with_transcript(&[]);
    let agent = app.selected.unwrap();
    own_event(
        &mut app,
        EventKind::TaskCompleted {
            result: TaskResult::summary("old completion"),
        },
    );
    let old = *app.transcripts[&agent].named_only.iter().next().unwrap();
    for _ in 0..5_001 {
        own_event(
            &mut app,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("later activity")],
                invocation: None,
            },
        );
    }
    assert!(app.transcripts[&agent].cell(old).is_none());
    assert!(app.transcripts[&agent].named_only.is_empty());
    own_event(
        &mut app,
        EventKind::TaskCompleted {
            result: TaskResult::summary("new completion"),
        },
    );
    let transcript = &app.transcripts[&agent];
    assert_eq!(transcript.named_only.len(), 1);
    assert!(transcript.presentation_cells(true).any(|cell|
        matches!(&cell.model, CellModel::ChildResult { result, .. } if result == "new completion")));
}

#[test]
fn cached_projection_follows_owner_scope_changes() {
    let mut app = app_with_tool(ToolOutcome::ok("output"));
    let agent = app.selected.unwrap();
    app.view_modes.modes.insert(
        "brief".into(),
        Mode {
            name: "brief".into(),
            source: "fixture".into(),
            definition: Definition::parse(&format!("{BRIEF}\n[scope]\nrole='supervisor'")).unwrap(),
        },
    );
    submit(&mut app, "/mode brief");
    let mut cache = aether_tui::ui::transcript::ViewCache::default();
    for (role, hidden) in [
        ("supervisor", true),
        ("worker", false),
        ("supervisor", true),
    ] {
        app.tree.nodes.get_mut(&agent).unwrap().agent.role.0 = role.into();
        app.names_version += 1;
        let screen = aether_tui::testing::render_with(&app, &mut cache, 120, 30);
        assert_eq!(!screen.contains("secret tool"), hidden, "{screen}");
    }
}

#[test]
fn mode_status_preserves_the_owner_at_the_end_of_the_status_line() {
    let mut app = app_with_transcript(&[]);
    submit(&mut app, "/focus");
    for width in [40, 80, 120] {
        let frame = render_to(&app, width, 30);
        let status = frame
            .lines()
            .find(|line| line.contains("claude-opus-5") && line.contains("focus"))
            .unwrap();
        assert!(status.trim_end().ends_with("assistant"), "{status}");
    }
}

#[test]
fn external_mode_is_in_completion_and_tab_uses_the_same_resolver() {
    use aether_tui::ui::bottom::popup::{self, Popup, PopupItem, PopupKind};
    let mut app = app_with_transcript(&[]);
    app.view_modes.modes.insert(
        "brief".into(),
        Mode {
            name: "brief".into(),
            source: "fixture".into(),
            definition: Definition::parse(BRIEF).unwrap(),
        },
    );
    let mut popup = Popup::new(PopupKind::Slash);
    popup.query = "brief".into();
    assert!(
        matches!(popup::items(&popup, &app).first(), Some(PopupItem::Mode { name, .. }) if name == "brief")
    );
    app.input.set_text("/brief");
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Tab,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.view_mode, "brief");
}

#[test]
fn mode_completion_uses_local_enter_and_tab_when_send_is_rebound() {
    for code in [KeyCode::Enter, KeyCode::Tab] {
        let mut app = app_with_transcript(&[]);
        app.view_modes.modes.insert(
            "brief".into(),
            Mode {
                name: "brief".into(),
                source: "fixture".into(),
                definition: Definition::parse(BRIEF).unwrap(),
            },
        );
        app.keys
            .assign(aether_tui::keys::KeyAction::Send, "f6".parse().unwrap())
            .unwrap();
        for c in "/bri".chars() {
            app.reduce(AppEvent::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
        assert!(app.bottom.popup.is_some());
        app.reduce(AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        if code == KeyCode::Tab {
            assert_eq!(app.view_mode, "standard");
            assert_eq!(app.input.text, "/brief");
            app.reduce(AppEvent::Key(KeyEvent::new(
                KeyCode::F(6),
                KeyModifiers::NONE,
            )));
        }
        assert_eq!(app.view_mode, "brief");
        assert!(app.input.text.is_empty());
        assert!(app.bottom.popup.is_none());
    }
}

#[test]
fn find_excludes_hidden_cells_and_details_invalidates_the_index() {
    let mut app = app_with_tool(ToolOutcome::ok("output"));
    submit(&mut app, "/focus");
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    for c in "secret".chars() {
        app.reduce(AppEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    assert!(app.bottom.find.as_ref().unwrap().matches.is_empty());
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert_eq!(app.bottom.find.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn finding_an_already_visible_failure_keeps_hidden_categories_filtered() {
    use aether_tui::ui::cells::ChatCell as _;
    let mut app = app_with_tool(ToolOutcome::error(format!(
        "{}needle evidence\n{}",
        "filler\n".repeat(10),
        "filler\n".repeat(10)
    )));
    let transcript = &app.transcripts[&app.selected.unwrap()];
    let failure = transcript
        .cells
        .iter()
        .find(|c| matches!(c.model, CellModel::Exec { .. }))
        .unwrap();
    assert!(
        !failure
            .model
            .plain_lines(1000)
            .join("\n")
            .contains("needle")
    );
    app.view_modes.modes.insert(
        "tools".into(),
        Mode {
            name: "tools".into(),
            source: "fixture".into(),
            definition: Definition::parse("version=1\nshow=['assistant_text','tool']").unwrap(),
        },
    );
    submit(&mut app, "/mode tools");
    let screen = render_to(&app, 120, 40);
    assert!(screen.contains("needle evidence"), "{screen}");
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    app.reduce(AppEvent::Paste("needle".into()));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert_eq!(app.bottom.find.as_ref().unwrap().matches.len(), 1);
    assert!(
        !app.details,
        "the failure was already fully visible in this mode"
    );
}

const CALM: &str = r#"version = 1
label = "Calm"
description = "Archon coordination and decisions without tool detail"
density = "compact"
show = ["user_text", "assistant_text", "delegation", "agent_message", "completion", "status", "question", "approval", "error"]

[scope]
role = "archon-orchestrator"
"#;

fn enter_calm(app: &mut App) {
    let agent = app.selected.unwrap();
    app.tree.nodes.get_mut(&agent).unwrap().agent.role.0 = "archon-orchestrator".into();
    app.names_version += 1;
    app.view_modes.modes.insert(
        "calm".into(),
        Mode {
            name: "calm".into(),
            source: "fixture".into(),
            definition: Definition::parse(CALM).unwrap(),
        },
    );
    submit(app, "/mode calm");
    assert_eq!(app.view_mode, "calm");
    assert!(app.policy(Some(agent)).definition.is_some());
}

fn failed_tool(app: &mut App, name: &str, input: serde_json::Value, error: &str) {
    use aether_core::event::EventKind;
    use aether_core::id::ToolCallId;
    use aether_core::tool::{SideEffectClass, ToolCall};
    let agent = app.selected.unwrap();
    let id = ToolCallId::new();
    let call = ToolCall {
        id,
        provider_call_id: "fixture".into(),
        name: name.into(),
        input,
        agent_id: agent,
        task_id: None,
        invocation_id: None,
        resources: vec![],
        idempotency_key: None,
        requested_at: app.now,
    };
    own_event(
        app,
        EventKind::ToolRequested {
            call,
            side_effect: SideEffectClass::ReadOnly,
        },
    );
    let outcome = ToolOutcome::error(error);
    own_event(app, EventKind::ToolSucceeded { call: id, outcome });
}

fn failed_shell(app: &mut App, command: &str, output: &str) {
    failed_tool(
        app,
        "bash",
        serde_json::json!({ "command": command }),
        output,
    );
}

fn say(app: &mut App, text: &str) {
    let agent = app.selected.unwrap();
    let role = aether_core::model::MessageRole::Assistant;
    aether_tui::testing::push_message(app, agent, role, text);
}

fn lines_with<'a>(screen: &'a str, needle: &str) -> Vec<&'a str> {
    screen
        .lines()
        .filter(|line| line.contains(needle))
        .map(str::trim)
        .collect()
}

/// A shell loop that exits 1 after several hundred lines of usage text.
fn app_with_a_long_failed_shell_call() -> App {
    let mut app = app_with_transcript(&[]);
    let usage: String = (0..400).map(|n| format!("usage line {n}\n")).collect();
    let output = format!("Exit code 1\nargument required when using the --repo flag\n{usage}");
    failed_shell(&mut app, "gh pr view 62 -R example-org/engine", &output);
    say(&mut app, "Visible answer");
    app
}

#[test]
fn a_failed_shell_call_is_one_line_in_a_mode_without_tools() {
    let mut app = app_with_a_long_failed_shell_call();
    enter_calm(&mut app);
    for width in [40, 120] {
        let screen = render_to(&app, width, 40);
        assert_eq!(
            lines_with(&screen, "failed"),
            ["• Command failed: Exit code 1"],
            "{screen}"
        );
        for hidden in ["gh pr view", "argument required", "usage line"] {
            assert!(!screen.contains(hidden), "{hidden} in {screen}");
        }
        assert!(screen.contains("Visible answer"), "{screen}");
    }
    // Nothing left the transcript: details draw the whole call again.
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let screen = render_to(&app, 120, 40);
    assert!(screen.contains("usage line 380"), "{screen}");
    assert!(screen.contains("usage line 399"), "{screen}");
}

#[test]
fn the_standard_mode_draws_a_failed_shell_call_as_before() {
    let mut app = app_with_a_long_failed_shell_call();
    enter_calm(&mut app);
    submit(&mut app, "/mode standard");
    let screen = render_to(&app, 120, 40);
    let call: Vec<&str> = screen
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("• Ran"))
        .take(7)
        .collect();
    assert_eq!(
        call,
        [
            "• Ran gh pr view 62 -R example-org/engine",
            "└ Exit code 1",
            "argument required when using the --repo flag",
            "… +397 lines (ctrl+t to view)",
            "usage line 397",
            "usage line 398",
            "usage line 399",
        ],
        "{screen}"
    );
}

#[test]
fn an_agent_error_is_drawn_whole_in_a_mode_without_tools() {
    use aether_core::event::EventKind;
    let mut app = app_with_transcript(&["go"]);
    let error = "provider exploded\nfirst cause line\nsecond cause line".to_owned();
    own_event(&mut app, EventKind::AgentFailed { error });
    enter_calm(&mut app);
    let screen = render_to(&app, 120, 40);
    for shown in ["provider exploded", "first cause line", "second cause line"] {
        assert!(screen.contains(shown), "{shown} not in {screen}");
    }
}

#[test]
fn consecutive_failed_calls_share_one_line_until_something_else_is_shown() {
    let mut app = app_with_transcript(&[]);
    for n in 0..5 {
        failed_shell(&mut app, &format!("probe {n}"), &format!("refused {n}"));
    }
    say(&mut app, "Trying another way");
    failed_shell(&mut app, "probe 9", "refused 9");
    enter_calm(&mut app);
    let screen = render_to(&app, 120, 40);
    assert_eq!(
        lines_with(&screen, "failed"),
        ["• Command failed: refused 0", "• Command failed: refused 9"],
        "{screen}"
    );
    assert!(!screen.contains("probe"), "{screen}");
    app.reduce(AppEvent::Resize(120, 40));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    app.reduce(AppEvent::Paste("refused".into()));
    assert_eq!(app.bottom.find.as_ref().unwrap().matches.len(), 2);
    assert!(!app.details, "both matches are on shown rows");
}

#[test]
fn failed_grouped_calls_are_one_line_each_in_a_mode_without_tools() {
    let mut app = app_with_transcript(&[]);
    let path = serde_json::json!({ "path": "notes.txt" });
    failed_tool(
        &mut app,
        "read_file",
        path.clone(),
        "not found\nsearched: /srv",
    );
    say(&mut app, "Editing instead");
    let edit = serde_json::json!({ "path": "notes.txt", "old_string": "secret before", "new_string": "secret after" });
    failed_tool(
        &mut app,
        "edit_file",
        edit,
        "old_string not found\nin notes.txt",
    );
    enter_calm(&mut app);
    let screen = render_to(&app, 120, 40);
    assert_eq!(
        lines_with(&screen, "failed"),
        [
            "• read_file failed: not found",
            "• edit_file failed: old_string not found"
        ],
        "{screen}"
    );
    for hidden in ["searched: /srv", "in notes.txt", "secret"] {
        assert!(!screen.contains(hidden), "{hidden} in {screen}");
    }
}

#[test]
fn compact_find_prefers_shown_text_and_reveals_matches_after_narrowing() {
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::{AgentId, EventId, MessageId};
    use aether_core::model::ContentBlock;
    let mut app = app_with_transcript(&[]);
    app.start_at(400, 40);
    app.board.open = false;
    let agent = app.selected.unwrap();
    let event = Event::from_draft(
        EventDraft::new(
            Actor::System,
            EventKind::MessageSent {
                message: MessageId::new(),
                from: Actor::agent(agent),
                to: AgentId::new(),
                content: vec![ContentBlock::text(format!(
                    "{}needle evidence",
                    "filler ".repeat(30)
                ))],
            },
        )
        .agent(agent),
        EventId::new(),
        1,
        app.now,
    );
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
    submit(&mut app, "/focus");
    assert!(render_to(&app, 400, 40).contains("needle evidence"));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    app.reduce(AppEvent::Paste("needle".into()));
    assert_eq!(app.bottom.find.as_ref().unwrap().matches.len(), 1);
    assert!(
        !app.details,
        "shown coordination text does not need details"
    );
    app.reduce(AppEvent::Resize(80, 40));
    assert!(aether_tui::ui::bottom::find::current_needs_details(
        app.bottom.find.as_ref().unwrap()
    ));
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(app.details);
    app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Esc,
        KeyModifiers::NONE,
    )));
    assert!(render_to(&app, 80, 40).contains("needle"));
}

#[test]
fn compact_coordination_snapshot() {
    let agent = aether_core::id::AgentId::new();
    let models = [
        CellModel::Spawned {
            agent,
            name: "tester".into(),
            role: "review".into(),
            model: "mock".into(),
            effort: None,
            objective: "Verify the readiness fix".into(),
        },
        CellModel::Sent {
            to: "tester".into(),
            text: "Include the isolated test".into(),
        },
        CellModel::User {
            from: UserFrom::Message {
                from: "tester".into(),
            },
            text: "Isolated test passed; full CI next".into(),
        },
        CellModel::ChildResult {
            agent,
            name: "tester".into(),
            role: "review".into(),
            outcome: ChildOutcome::Finished,
            elapsed_ms: 1000,
            cost_with_children: None,
            result: "CI passed; ready for approval".into(),
            descendants: vec![],
        },
    ];
    let theme = aether_tui::ui::cells::plain_theme();
    let actual = models
        .iter()
        .map(|m| {
            aether_tui::ui::cells::compact::line(m, 80, &theme)
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(actual, include_str!("snapshots/brief.txt").trim_end());
}

#[test]
fn mode_reload_banner_prioritizes_fallback_and_keeps_parse_details() {
    let mut app = app_with_transcript(&[]);
    app.view_mode = "brief".into();
    let mut catalog = Catalog::default();
    let reason = Definition::parse("version = [").unwrap_err();
    catalog.diagnostics.push(view_modes::Diagnostic {
        source: "/private/config/modes/broken.toml".into(),
        reason: reason.clone(),
    });
    app.reduce(AppEvent::ViewModesLoaded(catalog));
    let banner = app.live_flash().unwrap();
    assert!(
        banner.starts_with("active mode brief not found; using standard"),
        "{banner}"
    );
    assert!(banner.contains("broken.toml:"), "{banner}");
    assert!(!banner.contains("\n"), "{banner}");
    assert_eq!(app.view_modes.diagnostics[1].reason, reason);
    submit(&mut app, "/mode");
    assert!(render_to(&app, 120, 40).contains("broken.toml"));
}

#[test]
fn startup_banner_prioritizes_missing_saved_mode_over_file_errors() {
    let mut app = app_with_transcript(&[]);
    app.view_modes.diagnostics.push(view_modes::Diagnostic {
        source: "/private/modes/bad.toml".into(),
        reason: "TOML parse error at line 1\nunknown field future".into(),
    });
    let (_, notice) = app.view_modes.resolve(None, None, Some("brief")).unwrap();
    app.view_modes.diagnostics.extend(notice);
    app.reduce(AppEvent::Connected);
    let banner = app.live_flash().unwrap();
    assert!(
        banner.starts_with("active mode brief not found; using standard"),
        "{banner}"
    );
    assert!(
        banner.contains("bad.toml: unknown field future"),
        "{banner}"
    );
    assert!(!banner.contains("/private/"), "{banner}");
}

#[test]
fn mode_banner_bounds_long_errors_and_uses_ascii_marker() {
    use aether_tui::ui::theme::tokens::{Theme, ThemeSpec};
    let mut app = app_with_transcript(&[]);
    app.theme = Theme::resolve(ThemeSpec {
        ascii: true,
        ..Default::default()
    });
    let reason = format!("unknown field {}", "x".repeat(200));
    app.view_modes.diagnostics.push(view_modes::Diagnostic {
        source: "broken.toml".into(),
        reason: reason.clone(),
    });
    app.reduce(AppEvent::Connected);
    let banner = app.live_flash().unwrap();
    assert!(banner.is_ascii(), "{banner}");
    assert!(banner.len() < 200, "{banner}");
    assert_eq!(app.view_modes.diagnostics[0].reason, reason);
}

#[test]
fn fallback_banner_keeps_standard_visible_for_long_valid_mode_names() {
    let mut app = app_with_transcript(&[]);
    let name = "x".repeat(48);
    assert!(view_modes::valid_name(&name));
    let (_, notice) = app.view_modes.resolve(None, None, Some(&name)).unwrap();
    app.view_modes.diagnostics.extend(notice);
    app.reduce(AppEvent::Connected);
    let banner = app.live_flash().unwrap();
    assert!(
        banner.starts_with(&format!("active mode {name} not found; using standard")),
        "{banner}"
    );
}
