//! W7 view tests: what opening a view requests, what its keys ask the app to
//! do, and how it paints at 80 and 120 columns in unicode and ascii.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use aether_core::approval::{ApprovalRule, RuleMatch, RuleScope};
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::event::Actor;
use aether_core::id::{AgentId, ArtifactId};
use aether_core::mcp::{McpServerHealth, McpServerState};
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope};
use aether_core::model::{Effort, ModelRef};
use aether_core::provider::ProviderHealth;
use aether_core::schedule::{ScheduleAction, ScheduleStatus};
use aether_core::time::now;
use aether_core::tool::ApprovalDecision;
use aether_protocol::{AuthInfo, Command, ModelInfo, ProviderInfo};

use super::{ViewKind, open};
use crate::app::{App, Effect, MemoryAction, OpenArtifact, Then};
use crate::commands::SlashCommand;
use crate::testing::populated_app;
use crate::ui::bottom::{BottomView, ViewAction, ViewOutcome};
use crate::ui::tests_support::screen_text;
use crate::ui::theme::ColorDepth;
use crate::ui::theme::tokens::{Rgb, TerminalPalette, Theme, ThemeMode, ThemeSpec};

// ----- helpers ------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(view: &mut dyn BottomView, app: &App, code: KeyCode) -> ViewOutcome {
    view.handle_key(key(code), app)
}

/// The actions of an outcome that keeps the view open.
fn acted(outcome: ViewOutcome) -> Vec<ViewAction> {
    match outcome {
        ViewOutcome::Act(actions) => actions,
        other => panic!("expected Act, got {other:?}"),
    }
}

/// The actions of an outcome that closes the view.
fn closed(outcome: ViewOutcome) -> Vec<ViewAction> {
    match outcome {
        ViewOutcome::Close(actions) => actions,
        other => panic!("expected Close, got {other:?}"),
    }
}

fn effect(action: &ViewAction) -> &Effect {
    match action {
        ViewAction::Effect(effect) => effect,
        other => panic!("expected an effect, got {other:?}"),
    }
}

fn command(action: &ViewAction) -> &SlashCommand {
    match action {
        ViewAction::Command(command) => command,
        other => panic!("expected a command, got {other:?}"),
    }
}

fn flash(action: &ViewAction) -> &str {
    match action {
        ViewAction::Flash(text) => text,
        other => panic!("expected a flash, got {other:?}"),
    }
}

fn request(command: Command, then: Then) -> Effect {
    Effect::Request { command, then }
}

fn agent_id(app: &App) -> AgentId {
    *app.tree
        .nodes
        .keys()
        .next()
        .expect("populated app has an agent")
}

/// `populated_app` with its one agent viewed.
fn viewed_app() -> App {
    let mut app = populated_app();
    app.selected = Some(agent_id(&app));
    app
}

fn theme(ascii: bool) -> Theme {
    Theme::resolve(ThemeSpec {
        ascii,
        ..Default::default()
    })
}

/// A probed dark terminal: the tint and accents are real colours.
fn dark() -> Theme {
    Theme::resolve(ThemeSpec {
        mode: ThemeMode::Dark,
        depth: ColorDepth::Truecolor,
        palette: Some(TerminalPalette {
            fg: Rgb(0xe6, 0xed, 0xf3),
            bg: Rgb(0x0d, 0x11, 0x17),
        }),
        ..Default::default()
    })
}

fn mono() -> Theme {
    Theme::resolve(ThemeSpec {
        mode: ThemeMode::Mono,
        ..Default::default()
    })
}

fn draw(view: &dyn BottomView, app: &App, width: u16) -> Terminal<TestBackend> {
    let height = view.desired_height(width, app).clamp(1, 40);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), app, &app.theme))
        .expect("draw");
    terminal
}

/// Whether `cell` carries every attribute of `style`.
fn shows(cell: &ratatui::buffer::Cell, style: ratatui::style::Style) -> bool {
    style.fg.is_none_or(|fg| cell.fg == fg)
        && style.bg.is_none_or(|bg| cell.bg == bg)
        && cell.modifier.contains(style.add_modifier)
}

/// Rows whose `›` marks the focused cursor: the accent, bold.
fn cursor_rows(terminal: &Terminal<TestBackend>, theme: &Theme) -> Vec<u16> {
    let buffer = terminal.backend().buffer();
    let marker = theme.accent().patch(theme.title());
    (0..buffer.area.height)
        .filter(|&y| {
            let cell = &buffer[(0, y)];
            cell.symbol() == theme.glyphs().prompt && shows(cell, marker)
        })
        .collect()
}

/// The shared frame (SPEC §3.4): tinted, padded, titled at the margin,
/// hints second-last, no `1-14/16` counter, at most one cursor.
fn assert_framed(terminal: &Terminal<TestBackend>, theme: &Theme, title: &str, label: &str) {
    let text = screen_text(terminal);
    let rows: Vec<&str> = text.lines().collect();
    let buffer = terminal.backend().buffer();
    let height = rows.len();
    assert!(
        rows[0].trim().is_empty(),
        "{label}: no top padding:\n{text}"
    );
    assert!(
        rows[1].starts_with("  ") && rows[1][2..].starts_with(title),
        "{label}: title row {:?}",
        rows[1]
    );
    assert!(
        shows(&buffer[(2, 1)], theme.title()),
        "{label}: title style"
    );
    assert!(
        rows[height - 1].trim().is_empty(),
        "{label}: no bottom padding"
    );
    assert!(
        rows[height - 2].contains("esc"),
        "{label}: hints {:?}",
        rows[height - 2]
    );
    let tint = theme.tint().bg.unwrap_or_default();
    assert!(
        buffer.content().iter().all(|cell| cell.bg == tint),
        "{label}: the block is not one tint"
    );
    let counter = rows.iter().any(|row| {
        row.split_whitespace().any(|word| {
            let mut parts = word.split(['-', '/']);
            word.contains('/')
                && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
    });
    assert!(!counter, "{label}: a row counter is back:\n{text}");
    let selected = theme.selected().add_modifier;
    assert!(
        selected.is_empty()
            || buffer
                .content()
                .iter()
                .all(|c| !c.modifier.contains(selected)),
        "{label}: views mark the cursor with `›`, not selected()"
    );
    assert!(
        cursor_rows(terminal, theme).len() <= 1,
        "{label}: two cursors:\n{text}"
    );
    if theme.ascii() {
        assert!(text.is_ascii(), "{label}:\n{text}");
    }
}

/// Checks the frame at 80 and 120 columns in dark, mono and ascii; returns
/// the 80-column dark screen.
fn snapshot(app: &mut App, make: impl Fn(&App) -> Box<dyn BottomView>, title: &str) -> String {
    let mut first = None;
    for (name, look) in [("dark", dark()), ("mono", mono()), ("ascii", theme(true))] {
        app.theme = look;
        for width in [80, 120] {
            let view = make(app);
            let terminal = draw(view.as_ref(), app, width);
            assert_framed(
                &terminal,
                &app.theme,
                title,
                &format!("{title} {name} {width}"),
            );
            first.get_or_insert_with(|| screen_text(&terminal));
        }
    }
    app.theme = theme(false);
    first.unwrap_or_default()
}

// ----- fixtures -----------------------------------------------------------------

fn model(provider: &str, name: &str, configured: bool) -> ModelInfo {
    ModelInfo {
        tier: None,
        aliases: Vec::new(),
        model: ModelRef::new(provider, name),
        name: name.into(),
        provider_name: format!("{provider} cloud"),
        capabilities: Default::default(),
        configured,
        auth_source: None,
        is_default: false,
    }
}

fn auth(provider: &str, configured: bool, program: Option<&str>) -> AuthInfo {
    AuthInfo {
        provider: provider.into(),
        provider_name: format!("{provider} cloud"),
        configured,
        source: configured.then(|| "API key".into()),
        methods: vec!["api_key".into()],
        subscription: false,
        detail: program.map(|p| format!("{p}: not found")),
        program: program.map(Into::into),
    }
}

fn rule(tool: &str) -> ApprovalRule {
    ApprovalRule {
        id: aether_core::id::ApprovalRuleId::new(),
        decision: ApprovalDecision::Approved,
        matcher: RuleMatch::tool(tool),
        scope: RuleScope::Everywhere,
        created_at: now(),
        created_by: Actor::user(),
        note: Some("ran it twice".into()),
        revoked: None,
    }
}

fn memory(text: &str, forgotten: bool) -> MemoryRecord {
    let mut record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: MemoryScope::Global,
            kind: MemoryKind::Fact,
            text: text.into(),
            structured: None,
            importance: 0.5,
            confidence: 0.5,
            tags: vec!["deploys".into()],
            provenance: Default::default(),
            supersedes: None,
        },
        now(),
    );
    record.forgotten_at = forgotten.then(now);
    record
}

fn artifact(name: &str) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: ArtifactKind::CommandOutput,
        name: name.into(),
        path: Some(format!("/tmp/{name}").into()),
        summary: "cargo test output".into(),
        size_bytes: 2048,
        content_type: None,
        digest: None,
        created_by: Actor::System,
        agent_id: None,
        task_id: None,
        created_at: now(),
        evicted_at: None,
    }
}

fn provider(name: &str) -> ProviderInfo {
    ProviderInfo {
        headroom: Default::default(),
        provider: name.into(),
        name: name.into(),
        protocol: "anthropic-messages".into(),
        family: Some("claude".into()),
        base_url: String::new(),
        engine: false,
        models: 3,
        open_model_list: false,
        enabled: true,
        configured: true,
        auth_source: Some("ANTHROPIC_API_KEY".into()),
        health: ProviderHealth::idle(name),
    }
}

// ----- open_view ------------------------------------------------------------------

#[test]
fn open_view_pushes_one_view_and_requests_its_data() {
    let mut app = viewed_app();
    app.notifications.set_served(true);
    let agent = agent_id(&app);
    let run = app.tree.nodes[&agent].agent.run;
    let cases = [
        (
            ViewKind::Model,
            vec![request(Command::ListModels, Then::Models)],
        ),
        (
            ViewKind::Usage,
            vec![request(Command::Providers, Then::Usage)],
        ),
        (
            ViewKind::Approvals,
            vec![request(
                Command::ListApprovalRules {
                    include_revoked: false,
                },
                Then::ApprovalRules,
            )],
        ),
        (
            ViewKind::Schedules,
            vec![request(
                Command::ListSchedules { target: None },
                Then::Schedules,
            )],
        ),
        (
            ViewKind::Mcp,
            vec![request(
                Command::McpServers {
                    refresh: false,
                    server: None,
                },
                Then::McpServers,
            )],
        ),
        (
            ViewKind::Report,
            vec![request(Command::GetRunReport { run }, Then::RunReport(run))],
        ),
        (
            ViewKind::Login,
            vec![request(Command::AuthStatus, Then::Auth)],
        ),
        (ViewKind::Doctor, vec![]),
        (ViewKind::Settings, vec![]),
        (ViewKind::Notifications, vec![]),
        (ViewKind::Voice, vec![]),
    ];
    for (kind, expected) in cases {
        let before = app.bottom.view_stack.len();
        let effects = app.open_view(kind);
        assert_eq!(effects, expected, "{kind:?}");
        assert_eq!(app.bottom.view_stack.len(), before + 1, "{kind:?}");
    }
    assert!(app.usage_loading);
    assert_eq!(app.report.run, Some(run));
    assert!(app.report.loading);
}

#[test]
fn memory_view_asks_for_memories() {
    let mut app = viewed_app();
    let effects = app.open_view(ViewKind::Memory);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Request {
                command: Command::Memories { .. },
                then: Then::Memories
            }]
        ),
        "{effects:?}"
    );
}

#[test]
fn a_second_usage_open_does_not_race_the_first() {
    let mut app = viewed_app();
    assert_eq!(app.open_view(ViewKind::Usage).len(), 1);
    assert!(app.open_view(ViewKind::Usage).is_empty());
}

#[test]
fn report_and_artifacts_need_a_viewed_agent() {
    let mut app = populated_app();
    app.selected = None;
    assert!(app.open_view(ViewKind::Report).is_empty());
    assert_eq!(app.report.run, None);
    assert!(app.open_view(ViewKind::Artifacts).is_empty());
    assert_eq!(app.artifacts.for_agent, None);
}

#[test]
fn artifacts_open_arms_the_list_reply_for_the_viewed_agent() {
    let mut app = viewed_app();
    let agent = agent_id(&app);
    let effects = app.open_view(ViewKind::Artifacts);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Request { command: Command::ListArtifacts { query }, then: Then::Artifacts(a) }]
                if query.agent == Some(agent) && *a == agent
        ),
        "{effects:?}"
    );
    assert_eq!(app.artifacts.for_agent, Some(agent));
}

#[test]
fn artifact_pager_arms_the_bytes_reply_and_is_fullscreen() {
    let mut app = viewed_app();
    let id = ArtifactId::new();
    let effects = app.open_view(ViewKind::ArtifactPager(id));
    assert_eq!(
        effects,
        vec![request(
            Command::ReadArtifact {
                id,
                max_bytes: crate::app::ARTIFACT_VIEW_BYTES
            },
            crate::testing::artifact_bytes_then(&app, id)
        )]
    );
    assert_eq!(app.artifacts.pending_bytes, Some((id, app.artifact_reads)));
    assert!(app.bottom.top_view().expect("pushed").fullscreen());
}

#[test]
fn voice_install_starts_once_and_refuses_a_second() {
    use crate::voice::Effect as VoiceEffect;
    use crate::voice::setup::ModelKind;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = viewed_app();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    app.voice.available = true;
    let model = ModelKind::Piper;
    let effects = app.open_view(ViewKind::VoiceInstall(model));
    assert!(
        matches!(effects.as_slice(), [Effect::Voice(VoiceEffect::Setup { model: m, .. })] if *m == model),
        "{effects:?}"
    );
    assert_eq!(app.voice.install_stage(model), Some("starting"));
    assert!(app.open_view(ViewKind::VoiceInstall(model)).is_empty());
    assert!(
        app.live_flash()
            .is_some_and(|text| text.contains("already running")),
        "{:?}",
        app.flash
    );
}

// ----- picker-backed views --------------------------------------------------------

#[test]
fn model_view_applies_only_what_changed() {
    let mut app = viewed_app();
    let current = app.tree.nodes[&agent_id(&app)].agent.model.clone();
    app.models = vec![
        ModelInfo {
            tier: None,
            model: current.clone(),
            ..model("anthropic", "x", true)
        },
        model("openai", "gpt-6", true),
        model("google", "gemini", false),
    ];
    let mut view = super::model::ModelView::new(&app);
    // Enter on the current model and effort changes nothing.
    assert!(closed(press(&mut view, &app, KeyCode::Enter)).is_empty());

    let mut view = super::model::ModelView::new(&app);
    press(&mut view, &app, KeyCode::Down);
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    assert!(matches!(
        command(&actions[0]),
        SlashCommand::Model { chain, default_only: false } if chain.primary.model == "gpt-6"
    ));
    assert_eq!(actions.len(), 1);

    let mut view = super::model::ModelView::new(&app);
    press(&mut view, &app, KeyCode::Right);
    press(&mut view, &app, KeyCode::Home);
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    let first = crate::app::EFFORTS[0].0;
    if first == app.effort_for_views() {
        assert!(actions.is_empty());
    } else {
        assert_eq!(command(&actions[0]), &SlashCommand::Effort(first));
    }
}

#[test]
fn model_picker_resolves_current_and_session_default_aliases() {
    let mut app = viewed_app();
    let id = agent_id(&app);
    app.tree.nodes.get_mut(&id).expect("agent").agent.model = ModelRef::new("claude-code", "opus");
    app.options.default_model = Some(ModelRef::new("claude-code", "sonnet"));
    let mut opus = model("claude-code", "claude-opus-5-5", true);
    opus.aliases = vec!["opus".into()];
    let mut sonnet = model("claude-code", "claude-sonnet-5-5", true);
    sonnet.aliases = vec!["sonnet".into()];
    app.models = vec![sonnet, opus];
    let mut view = super::model::ModelView::new(&app);
    let screen = screen_text(&draw(&view, &app, 100));
    assert!(screen.contains("current"), "{screen}");
    assert!(screen.contains("new agents"), "{screen}");
    assert!(closed(press(&mut view, &app, KeyCode::Enter)).is_empty());

    let mut view = super::model::ModelView::new(&app);
    press(&mut view, &app, KeyCode::Down);
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    assert!(
        matches!(command(&actions[0]), SlashCommand::Model { chain, .. }
        if chain.primary == ModelRef::new("claude-code", "claude-sonnet-5-5"))
    );
}

/// Recent choices lead the picker, aliases included, without
/// reordering the catalogue an alias shared by two entries resolves against.
#[test]
fn model_picker_lists_recent_choices_first_without_reordering_the_catalogue() {
    let mut app = viewed_app();
    let mut first = model("p", "alpha", true);
    first.aliases = vec!["fast".into()];
    let mut second = model("p", "bravo", true);
    second.aliases = vec!["fast".into(), "b".into()];
    app.models = vec![first, second, model("p", "charlie", true)];
    app.recent_models = vec!["p/charlie".into(), "p/b".into()];
    let screen = screen_text(&draw(&super::model::ModelView::new(&app), &app, 100));
    let at = |name: &str| {
        screen
            .find(name)
            .unwrap_or_else(|| panic!("{name}: {screen}"))
    };
    assert!(at("p/charlie") < at("p/bravo"), "{screen}");
    assert!(at("p/bravo") < at("p/alpha"), "{screen}");
    let shared = app.model_info(&ModelRef::new("p", "fast")).expect("alias");
    assert_eq!(shared.model, ModelRef::new("p", "alpha"));
}

#[test]
fn model_view_says_how_to_sign_in_instead_of_switching() {
    let mut app = viewed_app();
    app.models = vec![model("google", "gemini", false)];
    let mut view = super::model::ModelView::new(&app);
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    assert!(flash(&actions[0]).contains("/login google"), "{actions:?}");
    assert!(closed(press(&mut view, &app, KeyCode::Esc)).is_empty());
}

#[test]
fn model_view_filters_by_typing_and_never_types_into_the_draft() {
    let mut app = viewed_app();
    app.models = vec![
        model("anthropic", "opus", true),
        model("openai", "gpt", true),
    ];
    let mut view = super::model::ModelView::new(&app);
    for c in "gpt".chars() {
        assert!(matches!(
            press(&mut view, &app, KeyCode::Char(c)),
            ViewOutcome::Consumed
        ));
    }
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    assert!(
        matches!(command(&actions[0]), SlashCommand::Model { chain, .. } if chain.primary.model == "gpt")
    );
    let ctrl = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert!(matches!(view.handle_key(ctrl, &app), ViewOutcome::Ignored));
}

/// Two Sonnets that differ only at the end, behind a long provider name (sweep 8).
fn sibling_models(app: &mut App) {
    app.models = ["claude-sonnet-5-5", "claude-sonnet-5", "claude-opus-5-5"]
        .into_iter()
        .map(|name| ModelInfo {
            provider_name: "Claude Code (subscription)".into(),
            ..model("claude-code", name, true)
        })
        .collect();
}

#[test]
fn model_view_keeps_ids_whole_before_provider_names() {
    let mut app = viewed_app();
    sibling_models(&mut app);
    // 79 columns: the bottom pane at 120 with the board docked.
    let view = open(ViewKind::Model, &app);
    let screen = screen_text(&draw(view.as_ref(), &app, 79));
    for id in [
        "claude-code/claude-sonnet-5-5 ",
        "claude-code/claude-sonnet-5 ",
        "claude-code/claude-opus-5-5 ",
    ] {
        assert!(screen.contains(id), "{id}:\n{screen}");
    }
}

#[test]
fn model_view_names_the_selected_model_in_full_however_narrow() {
    let mut app = viewed_app();
    sibling_models(&mut app);
    let mut view = super::model::ModelView::new(&app);
    press(&mut view, &app, KeyCode::Down);
    let screen = screen_text(&draw(&view, &app, 79));
    let rows: Vec<&str> = screen.lines().map(str::trim_end).collect();
    assert!(
        rows.contains(&"  claude-code/claude-sonnet-5")
            && rows.iter().any(|row| row.contains("budget support unknown")
                && row.contains("Claude Code (subscription)")),
        "{screen}"
    );

    // Too narrow for any id: rows lose the middle, keeping the distinguishing end.
    let screen = screen_text(&draw(&view, &app, 48));
    let rows: Vec<&str> = screen.lines().map(str::trim_end).collect();
    for cut in [
        "  claude-c…net-5-5 ",
        "› claude-c…onnet-5 ",
        "  claude-c…pus-5-5 ",
    ] {
        assert!(
            rows.iter().any(|row| row.starts_with(cut)),
            "{cut}:\n{screen}"
        );
    }
    let at = rows
        .iter()
        .position(|row| *row == "  claude-code/claude-sonnet-5");
    assert!(at.is_some(), "the selected id is not whole:\n{screen}");
    assert_eq!(
        rows[at.unwrap() + 1],
        "  budget support unknown · Claude Code"
    );
    assert_eq!(rows[at.unwrap() + 2], "  (subscription)");
}

#[test]
fn effort_starts_on_the_agents_effort() {
    let mut app = viewed_app();
    let agent = agent_id(&app);
    app.tree
        .nodes
        .get_mut(&agent)
        .expect("agent")
        .agent
        .config
        .effort = Some(Effort::Low);
    assert_eq!(app.effort_for_views(), Effort::Low);
}

#[test]
fn login_view_signs_in_or_says_what_to_install() {
    let mut app = viewed_app();
    app.auth = vec![
        auth("openai", false, None),
        auth("claude-code", false, Some("claude")),
    ];
    let mut view = super::login::LoginView::default();
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    assert_eq!(
        command(&actions[0]),
        &SlashCommand::Login {
            provider: Some("openai".into())
        }
    );
    let mut view = super::login::LoginView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('2')));
    assert!(flash(&actions[0]).contains("not installed"), "{actions:?}");
}

#[test]
fn approvals_revoke_toggle_and_close() {
    let mut app = viewed_app();
    app.approval_rules = vec![rule("bash"), rule("write_file")];
    let second = app.approval_rules[1].id;
    let mut view = super::approvals::ApprovalsView::default();
    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Char('d')));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::RevokeApprovalRule {
                id: second,
                reason: None
            },
            Then::RuleRevoked
        )
    );
    let actions = acted(press(&mut view, &app, KeyCode::Char('y')));
    assert_eq!(command(&actions[0]), &SlashCommand::Yolo);
    // Letters are hotkeys: they never filter or reach the draft.
    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('z')),
        ViewOutcome::Consumed
    ));
    assert!(closed(press(&mut view, &app, KeyCode::Esc)).is_empty());

    app.approval_rules.clear();
    let mut view = super::approvals::ApprovalsView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('d')));
    assert!(flash(&actions[0]).contains("no rule"));
}

#[test]
fn memory_pin_forget_twice_and_restore() {
    let mut app = viewed_app();
    app.memories = vec![
        memory("Deploys go out on Tuesdays", false),
        memory("Old fact", true),
    ];
    let (first, second) = (app.memories[0].id, app.memories[1].id);
    let mut view = super::memory::MemoryView::default();

    let actions = acted(press(&mut view, &app, KeyCode::Char('p')));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::PinMemory {
                id: first,
                pinned: true
            },
            Then::MemoryMutated(MemoryAction::Pinned)
        )
    );

    let actions = acted(press(&mut view, &app, KeyCode::Char('f')));
    assert!(flash(&actions[0]).contains("f again"));
    let actions = acted(press(&mut view, &app, KeyCode::Char('f')));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::ForgetMemory {
                id: first,
                reason: None
            },
            Then::MemoryMutated(MemoryAction::Forgotten)
        )
    );

    // Any other key disarms the pending forget.
    press(&mut view, &app, KeyCode::Char('f'));
    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Char('f')));
    assert!(flash(&actions[0]).contains("f again"));

    let actions = acted(press(&mut view, &app, KeyCode::Char('u')));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::RestoreMemory { id: second },
            Then::MemoryMutated(MemoryAction::Restored)
        )
    );
    press(&mut view, &app, KeyCode::Up);
    let actions = acted(press(&mut view, &app, KeyCode::Char('u')));
    assert!(flash(&actions[0]).contains("not forgotten"));
}

#[test]
fn schedules_pause_cancel_twice_history_and_refresh() {
    let mut app = viewed_app();
    let agent = agent_id(&app);
    let active = crate::testing::schedule(agent);
    let mut paused = crate::testing::schedule(agent);
    paused.status = ScheduleStatus::Paused { next_due: now() };
    let mut done = crate::testing::schedule(agent);
    done.status = ScheduleStatus::Cancelled { at: now() };
    let ids = [active.id, paused.id, done.id];
    app.schedules.jobs = vec![active, paused, done];
    let mut view = super::schedules::SchedulesView::default();

    let update = |id, action| {
        request(
            Command::UpdateSchedule {
                id,
                action,
                idempotency_key: None,
            },
            Then::ScheduleChanged,
        )
    };
    let actions = acted(press(&mut view, &app, KeyCode::Char('p')));
    assert_eq!(effect(&actions[0]), &update(ids[0], ScheduleAction::Pause));

    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('c')),
        ViewOutcome::Consumed
    ));
    let actions = acted(press(&mut view, &app, KeyCode::Char('c')));
    assert_eq!(effect(&actions[0]), &update(ids[0], ScheduleAction::Cancel));

    let actions = acted(press(&mut view, &app, KeyCode::Down));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::ScheduleHistory {
                id: ids[1],
                limit: 100
            },
            Then::ScheduleHistory(ids[1])
        )
    );
    let actions = acted(press(&mut view, &app, KeyCode::Char('p')));
    assert_eq!(effect(&actions[0]), &update(ids[1], ScheduleAction::Resume));

    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Char('p')));
    assert!(flash(&actions[0]).contains("finished"));

    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    assert_eq!(
        effect(&actions[0]),
        &request(Command::ListSchedules { target: None }, Then::Schedules)
    );
    // At the last row Down does not move, so no history is asked for again.
    assert!(matches!(
        press(&mut view, &app, KeyCode::Down),
        ViewOutcome::Consumed
    ));
}

#[test]
fn artifacts_open_the_pager_or_report_dropped_bytes() {
    let mut app = viewed_app();
    let mut evicted = artifact("old.log");
    evicted.evicted_at = Some(now());
    app.artifacts.list = vec![artifact("test.log"), evicted];
    let id = app.artifacts.list[0].id;
    let mut view = super::artifacts::ArtifactsView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    assert!(
        matches!(&actions[..], [ViewAction::Open(ViewKind::ArtifactPager(a))] if *a == id),
        "{actions:?}"
    );
    let actions = acted(press(&mut view, &app, KeyCode::Char('2')));
    assert!(flash(&actions[0]).contains("retention"), "{actions:?}");
}

#[test]
fn voice_toggles_and_install_confirmation() {
    use crate::voice::Action;
    let mut app = viewed_app();
    let mut view = super::voice::VoiceView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    assert_eq!(command(&actions[0]), &SlashCommand::Voice(Action::On));
    app.voice.enabled = true;
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    assert_eq!(command(&actions[0]), &SlashCommand::Voice(Action::Off));
    let actions = acted(press(&mut view, &app, KeyCode::Char('2')));
    assert_eq!(
        command(&actions[0]),
        &SlashCommand::Voice(Action::RepliesOn)
    );
    let actions = acted(press(&mut view, &app, KeyCode::Char('3')));
    assert_eq!(command(&actions[0]), &SlashCommand::Voice(Action::Check));

    let actions = acted(press(&mut view, &app, KeyCode::Char('4')));
    let [ViewAction::Push(confirm)] = &actions[..] else {
        panic!("install opens the confirmation: {actions:?}");
    };
    assert!(!confirm.fullscreen());
}

#[test]
fn voice_confirm_installs_only_when_nothing_is_missing() {
    use crate::voice::setup::{ModelKind, missing_prerequisites};
    let app = viewed_app();
    let model = ModelKind::Tiny;
    let mut confirm = super::voice::VoiceConfirmView::new(model);
    let outcome = press(&mut confirm, &app, KeyCode::Enter);
    if missing_prerequisites(model).is_empty() {
        let actions = closed(outcome);
        assert!(
            matches!(&actions[..], [ViewAction::Open(ViewKind::VoiceInstall(m))] if *m == model)
        );
    } else {
        assert!(matches!(outcome, ViewOutcome::Consumed));
    }
    let mut confirm = super::voice::VoiceConfirmView::new(model);
    assert!(closed(press(&mut confirm, &app, KeyCode::Esc)).is_empty());
}

// ----- body views -----------------------------------------------------------------

#[test]
fn mcp_reconnects_on_r() {
    let app = viewed_app();
    let mut view = super::mcp::McpView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    assert_eq!(
        effect(&actions[0]),
        &request(
            Command::McpServers {
                refresh: true,
                server: None
            },
            Then::McpServers
        )
    );
    assert_eq!(actions.len(), 1);
    assert!(closed(press(&mut view, &app, KeyCode::Esc)).is_empty());
}

#[test]
fn view_requests_arm_the_loading_state_they_imply() {
    let mut app = viewed_app();
    let mut view = super::mcp::McpView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    app.apply_view_actions(actions);
    assert!(app.mcp_refreshing);

    let mut view = super::usage::UsageView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    app.apply_view_actions(actions);
    assert!(app.usage_loading);

    let mut view = super::schedules::SchedulesView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    app.apply_view_actions(actions);
    assert!(app.schedules.loading);
}

#[test]
fn usage_refreshes_once_at_a_time() {
    let mut app = viewed_app();
    let mut view = super::usage::UsageView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    assert_eq!(
        effect(&actions[0]),
        &request(Command::Providers, Then::Usage)
    );
    app.usage_loading = true;
    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('r')),
        ViewOutcome::Consumed
    ));
}

#[test]
fn body_views_scroll_close_and_pass_chords_on() {
    let app = viewed_app();
    let views: Vec<Box<dyn BottomView>> = vec![
        Box::new(super::doctor::DoctorView::default()),
        Box::new(super::usage::UsageView::default()),
        Box::new(super::mcp::McpView::default()),
        Box::new(super::report::ReportView::default()),
    ];
    for mut view in views {
        assert!(matches!(
            press(view.as_mut(), &app, KeyCode::Down),
            ViewOutcome::Consumed
        ));
        assert!(matches!(
            press(view.as_mut(), &app, KeyCode::Char('x')),
            ViewOutcome::Consumed
        ));
        let chord = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(view.handle_key(chord, &app), ViewOutcome::Ignored));
        assert!(matches!(
            view.handle_key(key(KeyCode::F(2)), &app),
            ViewOutcome::Ignored
        ));
        assert!(closed(press(view.as_mut(), &app, KeyCode::Esc)).is_empty());
    }
}

#[test]
fn report_opens_the_pager_and_refreshes_its_run() {
    let mut app = viewed_app();
    let run = app.tree.nodes[&agent_id(&app)].agent.run;
    let mut view = super::report::ReportView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    let [ViewAction::Push(pager)] = &actions[..] else {
        panic!("enter pushes the pager: {actions:?}");
    };
    assert!(pager.fullscreen());
    // Without a run on screen there is nothing to refresh.
    assert!(matches!(
        press(&mut view, &app, KeyCode::Char('r')),
        ViewOutcome::Consumed
    ));
    app.report.run = Some(run);
    let actions = acted(press(&mut view, &app, KeyCode::Char('r')));
    assert_eq!(
        effect(&actions[0]),
        &request(Command::GetRunReport { run }, Then::RunReport(run))
    );
}

#[test]
fn settings_steps_emit_the_typed_commands() {
    let app = viewed_app();
    let mut view = super::settings::SettingsView::default();
    // Theme: the step from whatever is current is a theme command.
    let actions = acted(press(&mut view, &app, KeyCode::Right));
    assert!(
        matches!(command(&actions[0]), SlashCommand::SetTheme(_)),
        "{actions:?}"
    );
    let actions = acted(press(&mut view, &app, KeyCode::Left));
    assert!(matches!(command(&actions[0]), SlashCommand::SetTheme(_)));
    // Enter on the theme row opens every theme.
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    assert_eq!(command(&actions[0]), &SlashCommand::Theme);

    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    let reduce = !app.motion.reduced();
    assert_eq!(command(&actions[0]), &SlashCommand::Motion(Some(reduce)));

    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Right));
    assert!(!actions.is_empty());

    press(&mut view, &app, KeyCode::Down);
    let actions = acted(press(&mut view, &app, KeyCode::Right));
    assert!(flash(&actions[0]).contains("--ascii"));
    assert!(closed(press(&mut view, &app, KeyCode::Esc)).is_empty());
}

#[test]
fn settings_theme_and_mouse_steps_are_typed_commands() {
    use crate::prefs::MouseMode;
    use crate::ui::theme::ThemeChoice;
    let mut app = viewed_app();
    let mut view = super::settings::SettingsView::default();
    let all = ThemeChoice::builtins();
    for (at, current) in all.iter().enumerate() {
        app.theme_choice = current.clone();
        let next = all[(at + 1) % all.len()].clone();
        let actions = acted(press(&mut view, &app, KeyCode::Right));
        assert_eq!(
            command(&actions[0]),
            &SlashCommand::SetTheme(next),
            "{current}"
        );
    }
    // A theme file steps on from auto.
    app.theme_choice = "ember".parse().unwrap();
    let actions = acted(press(&mut view, &app, KeyCode::Right));
    assert_eq!(
        command(&actions[0]),
        &SlashCommand::SetTheme(all[1].clone())
    );
    press(&mut view, &app, KeyCode::Down);
    press(&mut view, &app, KeyCode::Down);
    for (mode, next) in [
        (MouseMode::Off, MouseMode::Scroll),
        (MouseMode::Scroll, MouseMode::Off),
        (MouseMode::Full, MouseMode::Off),
    ] {
        app.mouse = mode;
        let actions = acted(press(&mut view, &app, KeyCode::Right));
        assert_eq!(
            command(&actions[0]),
            &SlashCommand::Mouse(Some(next)),
            "{mode:?}"
        );
    }
}

#[test]
fn theme_view_lists_built_ins_then_files_and_starts_on_the_current_theme() {
    use crate::ui::theme::{ThemeChoice, files};
    let dir = tempfile::tempdir().unwrap();
    let themes = files::dir(dir.path());
    std::fs::create_dir_all(&themes).unwrap();
    for name in ["ember", "nord"] {
        std::fs::write(themes.join(format!("{name}.toml")), "").unwrap();
    }
    let mut app = viewed_app();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    app.theme_choice = "gruvbox".parse().unwrap();
    let mut view = super::theme::ThemeView::new(&app);
    let text = screen_text(&draw(&view, &app, 120));
    assert!(text.contains("now gruvbox"), "{text}");
    assert!(text.contains("Gruvbox, follows the terminal"), "{text}");
    assert!(text.contains("ember.toml"), "{text}");
    assert!(text.contains("hidden by the built-in"), "{text}");
    // Starts on gruvbox, so one step down is the next built-in.
    press(&mut view, &app, KeyCode::Down);
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    let all = ThemeChoice::builtins();
    let at = all.iter().position(|c| *c == app.theme_choice).unwrap();
    assert_eq!(
        command(&actions[0]),
        &SlashCommand::SetTheme(all[at + 1].clone())
    );
    assert!(closed(press(&mut view, &app, KeyCode::Esc)).is_empty());
    // A pinned variant is not listed: the cursor starts on its family.
    app.theme_choice = "gruvbox-light".parse().unwrap();
    let mut view = super::theme::ThemeView::new(&app);
    let actions = closed(press(&mut view, &app, KeyCode::Enter));
    assert_eq!(
        command(&actions[0]),
        &SlashCommand::SetTheme("gruvbox".parse().unwrap())
    );
}

#[test]
fn pager_scrolls_within_the_rendered_text_and_closes_on_esc() {
    let mut app = viewed_app();
    let a = artifact("test.log");
    let id = a.id;
    let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
    app.artifacts.open = Some(OpenArtifact {
        artifact: a,
        text,
        truncated: false,
    });
    let mut pager = super::pager::PagerView::new(super::pager::PagerSource::Artifact(id));
    assert!(pager.fullscreen());
    let draw_at = |pager: &super::pager::PagerView, app: &App| {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).expect("terminal");
        terminal
            .draw(|frame| pager.render(frame, frame.area(), app, &app.theme))
            .expect("draw");
        screen_text(&terminal)
    };
    let screen = draw_at(&pager, &app);
    assert!(screen.lines().nth(1).unwrap_or("").contains("test.log"));
    assert!(screen.contains("  line 1 "), "{screen}");
    press(&mut pager, &app, KeyCode::End);
    let screen = draw_at(&pager, &app);
    assert!(screen.contains("line 100"), "{screen}");
    // 12 rows less six of frame leave six for text.
    assert!(screen.contains("lines 95–100 of 100"), "{screen}");
    for _ in 0..5 {
        press(&mut pager, &app, KeyCode::Down);
    }
    assert!(
        draw_at(&pager, &app).contains("lines 95–100 of 100"),
        "Down stops at the end"
    );
    assert!(closed(press(&mut pager, &app, KeyCode::Esc)).is_empty());
    // `q` no longer closes anything.
    assert!(matches!(
        press(&mut pager, &app, KeyCode::Char('q')),
        ViewOutcome::Consumed
    ));
}

#[test]
fn pager_wheel_scrolls_without_opening_or_closing_views() {
    let mut app = viewed_app();
    let a = artifact("wheel.log");
    let id = a.id;
    app.artifacts.open = Some(OpenArtifact {
        artifact: a,
        text: (1..=100).map(|n| format!("line {n}\n")).collect(),
        truncated: false,
    });
    app.size = (80, 12);
    app.bottom.push_view(Box::new(super::pager::PagerView::new(
        super::pager::PagerSource::Artifact(id),
    )));
    let mut cache = crate::ui::transcript::ViewCache::default();
    let before = crate::testing::render_with(&app, &mut cache, 80, 12);
    let r = cache.regions.view;
    let mouse = |kind| crossterm::event::MouseEvent {
        kind,
        column: r.x,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(
        app.dispatch_mouse(
            mouse(crossterm::event::MouseEventKind::ScrollDown),
            &mut cache
        )
        .is_empty()
    );
    let after = crate::testing::render_with(&app, &mut cache, 80, 12);
    assert_ne!(after, before);
    assert!(after.contains("line 4"), "{after}");
    assert!(
        app.dispatch_mouse(
            mouse(crossterm::event::MouseEventKind::ScrollUp),
            &mut cache
        )
        .is_empty()
    );
    assert_eq!(
        crate::testing::render_with(&app, &mut cache, 80, 12),
        before
    );
    assert_eq!(app.bottom.view_stack.len(), 1);
}

#[test]
fn the_theme_picker_follows_the_wheel() {
    let mut app = viewed_app();
    app.size = (80, 30);
    app.bottom
        .push_view(Box::new(super::theme::ThemeView::new(&app)));
    let mut cache = crate::ui::transcript::ViewCache::default();
    let before = crate::testing::render_with(&app, &mut cache, 80, 30);
    let r = cache.regions.view;
    let mut wheel = |app: &mut App, kind| {
        let mouse = crossterm::event::MouseEvent {
            kind,
            column: r.x,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.dispatch_mouse(mouse, &mut cache).is_empty());
        crate::testing::render_with(app, &mut cache, 80, 30)
    };
    let down = wheel(&mut app, crossterm::event::MouseEventKind::ScrollDown);
    assert_ne!(down, before, "the cursor moved");
    let up = wheel(&mut app, crossterm::event::MouseEventKind::ScrollUp);
    assert_eq!(up, before);
    assert_eq!(app.bottom.view_stack.len(), 1);
}

#[test]
fn voice_confirm_and_install_ignore_the_wheel() {
    use crate::voice::setup::ModelKind;
    let mut app = viewed_app();
    app.size = (40, 20);
    let views: Vec<Box<dyn BottomView>> = vec![
        Box::new(super::voice::VoiceConfirmView::new(ModelKind::Tiny)),
        Box::new(super::voice::VoiceInstallView::new(ModelKind::Tiny)),
    ];
    for view in views {
        app.bottom.view_stack = vec![view];
        let mut cache = crate::ui::transcript::ViewCache::default();
        let before = crate::testing::render_with(&app, &mut cache, 40, 20);
        let r = cache.regions.view;
        for kind in [
            crossterm::event::MouseEventKind::ScrollDown,
            crossterm::event::MouseEventKind::ScrollUp,
        ] {
            assert!(
                app.dispatch_mouse(
                    crossterm::event::MouseEvent {
                        kind,
                        column: r.x,
                        row: r.y,
                        modifiers: KeyModifiers::NONE,
                    },
                    &mut cache
                )
                .is_empty()
            );
            assert_eq!(
                crate::testing::render_with(&app, &mut cache, 40, 20),
                before
            );
        }
        assert_eq!(app.bottom.view_stack.len(), 1);
    }
}

#[test]
fn pager_waits_for_bytes_and_explains_dropped_ones() {
    let mut app = viewed_app();
    let mut gone = artifact("gone.log");
    gone.evicted_at = Some(now());
    let gone_id = gone.id;
    app.artifacts.list = vec![gone];
    let render = |source| {
        let pager = super::pager::PagerView::new(source);
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).expect("terminal");
        terminal
            .draw(|frame| pager.render(frame, frame.area(), &app, &app.theme))
            .expect("draw");
        screen_text(&terminal)
    };
    let waiting = render(super::pager::PagerSource::Artifact(ArtifactId::new()));
    assert!(waiting.contains("downloading"), "{waiting}");
    let dropped = render(super::pager::PagerSource::Artifact(gone_id));
    assert!(dropped.contains("retention"), "{dropped}");
}

// ----- snapshots ------------------------------------------------------------------

/// An app with something in every list, so each view paints real rows.
fn stocked_app() -> App {
    let mut app = viewed_app();
    let agent = agent_id(&app);
    let current = app.tree.nodes[&agent].agent.model.clone();
    app.models = vec![
        ModelInfo {
            tier: None,
            model: current,
            ..model("anthropic", "x", true)
        },
        model("google", "gemini", false),
    ];
    app.auth = vec![auth("openai", true, None), auth("google", false, None)];
    app.approval_rules = vec![rule("bash"), rule("write_file")];
    app.memories = vec![
        memory("Deploys go out on Tuesdays", false),
        memory("Old", true),
    ];
    app.schedules.jobs = vec![crate::testing::schedule(agent)];
    app.mcp_servers = vec![McpServerHealth {
        state: McpServerState::Failed,
        error: Some("spawn failed".into()),
        ..McpServerHealth::unknown("github", "stdio")
    }];
    app.providers = vec![provider("anthropic")];
    app.artifacts.list = vec![artifact("test.log")];
    app.notifications.set_served(true);
    app.notifications
        .apply(notification("Merge #231 into next?"));
    app
}

fn notification(title: &str) -> aether_core::notification::Notification {
    use aether_core::notification::{Notification, NotificationKind, NotificationPost};
    let post = NotificationPost {
        kind: NotificationKind::Question,
        title: title.into(),
        body: String::new(),
        key: None,
        link: None,
        actions: Vec::new(),
        free_text: true,
    };
    Notification::new("archon".into(), post, now())
}

#[test]
fn snapshots_paint_with_theme_tokens_at_80_and_120() {
    let mut app = stocked_app();
    let cases: [(ViewKind, &str, &str); 13] = [
        (ViewKind::Model, "Model and effort", "gemini"),
        (ViewKind::Usage, "Usage", "anthropic"),
        (ViewKind::Approvals, "Approvals", "allow bash"),
        (ViewKind::Memory, "Memory", "Deploys go out"),
        (ViewKind::Schedules, "Schedules", "Read docs"),
        (ViewKind::Notifications, "Notifications", "Merge #231"),
        (ViewKind::Mcp, "MCP servers", "spawn failed"),
        (ViewKind::Doctor, "Doctor", "daemon"),
        (ViewKind::Report, "Report", "reading the log"),
        (ViewKind::Artifacts, "Artifacts", "test.log"),
        (ViewKind::Settings, "Settings", "Theme"),
        (ViewKind::Voice, "Voice", "Speak questions"),
        (ViewKind::Login, "Sign in", "openai cloud"),
    ];
    for (kind, title, content) in cases {
        let screen = snapshot(&mut app, |app| open(kind, app), title);
        assert!(screen.contains(content), "{kind:?}:\n{screen}");
    }
}

#[test]
fn snapshot_pickers_mark_exactly_the_cursor_row() {
    let mut app = stocked_app();
    for kind in [
        ViewKind::Approvals,
        ViewKind::Memory,
        ViewKind::Schedules,
        ViewKind::Notifications,
        ViewKind::Artifacts,
        ViewKind::Settings,
        ViewKind::Voice,
        ViewKind::Login,
        ViewKind::Model,
    ] {
        for look in [dark(), mono(), theme(true)] {
            app.theme = look;
            let view = open(kind, &app);
            let terminal = draw(view.as_ref(), &app, 80);
            let rows = cursor_rows(&terminal, &app.theme);
            assert_eq!(
                rows.len(),
                1,
                "{kind:?}: {rows:?}\n{}",
                screen_text(&terminal)
            );
            // The label beside the marker is bold too, in the text colour.
            let label = &terminal.backend().buffer()[(5, rows[0])];
            assert!(
                label.modifier.contains(app.theme.title().add_modifier),
                "{kind:?}"
            );
        }
    }
}

#[test]
fn snapshot_settings_shows_the_overrides_path() {
    let mut app = stocked_app();
    app.options.prefs_dir = Some(std::path::PathBuf::from("/home/me/.config/aether"));
    let screen = snapshot(
        &mut app,
        |_| Box::new(super::settings::SettingsView::default()),
        "Settings",
    );
    assert!(screen.contains("6. View mode standard"), "{screen}");
    app.view_mode = "focus".into();
    let focused = screen_text(&draw(&super::settings::SettingsView::default(), &app, 80));
    assert!(focused.contains("6. View mode focus"), "{focused}");
    assert!(
        screen.contains("Overrides  /home/me/.config/aether/config.toml"),
        "{screen}"
    );
    assert!(
        screen.contains("Saved to   /home/me/.config/aether/ui.toml"),
        "{screen}"
    );
}

#[test]
fn snapshot_pager_is_fullscreen_text() {
    let mut app = stocked_app();
    let a = artifact("build.log");
    let id = a.id;
    app.artifacts.open = Some(OpenArtifact {
        artifact: a,
        text: "compiling aether\nfinished".into(),
        truncated: true,
    });
    for look in [dark(), mono(), theme(true)] {
        app.theme = look;
        for width in [80, 120] {
            let pager = super::pager::PagerView::new(super::pager::PagerSource::Artifact(id));
            let mut terminal = Terminal::new(TestBackend::new(width, 10)).expect("terminal");
            terminal
                .draw(|frame| pager.render(frame, frame.area(), &app, &app.theme))
                .expect("draw");
            assert_framed(&terminal, &app.theme, "build.log", "pager");
            let text = screen_text(&terminal);
            let rows: Vec<&str> = text.lines().collect();
            assert!(
                rows[1].contains("first"),
                "a truncated artifact says so: {text}"
            );
            assert!(!rows[1].contains("commandoutput"), "{text}");
            assert_eq!(rows[3].trim_end(), "  compiling aether");
        }
    }
}

#[test]
fn empty_lists_say_so_instead_of_painting_nothing() {
    let mut app = viewed_app();
    app.theme = theme(false);
    let cases = [
        (
            ViewKind::Approvals,
            "No remembered decisions · answer an approval with a or p",
        ),
        (
            ViewKind::Memory,
            "No memories yet · /remember <fact> saves one",
        ),
        (ViewKind::Artifacts, "No artifacts yet"),
    ];
    for (kind, says) in cases {
        let view = open(kind, &app);
        let text = screen_text(&draw(view.as_ref(), &app, 80));
        assert!(text.contains(says), "{kind:?}:\n{text}");
        let hints = text.lines().rev().nth(1).unwrap_or("");
        assert_eq!(hints.trim(), "esc back", "{kind:?}: only the way out");
    }
    let view = open(ViewKind::Mcp, &app);
    let text = screen_text(&draw(view.as_ref(), &app, 80));
    assert!(text.contains("No MCP servers configured"), "{text}");
    let view = open(ViewKind::Schedules, &app);
    let text = screen_text(&draw(view.as_ref(), &app, 80));
    assert!(text.contains("aether schedule create"), "{text}");
}

#[test]
fn memory_enter_opens_full_details_including_forgotten_record_provenance() {
    let mut app = viewed_app();
    let mut record = memory("First line\nfull multiline memory body", true);
    record.provenance.actor = Some(aether_core::event::Actor::connected_user(
        aether_core::event::OperatorOrigin::LocalSocket {
            connection: "operator".into(),
        },
    ));
    record.provenance.task = Some(aether_core::id::TaskId::new());
    record.provenance.event = Some(aether_core::id::EventId::new());
    record.provenance.artifact = Some(aether_core::id::ArtifactId::new());
    record.provenance.agent = app.selected;
    record.provenance.source = Some(aether_core::memory::MemorySource {
        file: "notes/project.md".into(),
        commit: Some("0123456789abcdef".into()),
    });
    record.supersedes = Some(aether_core::id::MemoryId::new());
    record.superseded_by = Some(aether_core::id::MemoryId::new());
    record.retrieval_count = 7;
    record.last_retrieved_at = Some(now());
    let id = record.id;
    app.memories.push(record);
    let mut view = super::memory::MemoryView::default();
    let actions = acted(press(&mut view, &app, KeyCode::Enter));
    let [ViewAction::Push(pager)] = &actions[..] else {
        panic!("{actions:?}");
    };
    assert!(pager.fullscreen());
    for width in [80, 120] {
        let backend = ratatui::backend::TestBackend::new(width, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| pager.render(frame, frame.area(), &app, &app.theme))
            .expect("render");
        let screen = crate::ui::tests_support::screen_text(&terminal);
        for text in [
            "full multiline memory body",
            "Importance:",
            "Confidence:",
            "Retrievals: 7",
            "Actor: user:local_socket#operator",
            "Task:",
            "Event:",
            "Artifact:",
            "Agent:",
            "notes/project.md",
            "0123456789abcdef",
            "Supersedes:",
            "Superseded by:",
            "Forgotten:",
        ] {
            assert!(screen.contains(text), "missing {text}: {screen}");
        }
        assert!(screen.contains(&id.to_string()));
    }
}

// ----- chords -------------------------------------------------------------------

fn chord(c: char, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), modifiers)
}

/// `view` open over `app`, then `keys` through the full dispatch.
fn dispatch(app: &mut App, view: Box<dyn BottomView>, keys: &[KeyEvent]) -> Vec<Effect> {
    app.bottom.view_stack.clear();
    app.bottom.push_view(view);
    keys.iter().flat_map(|key| app.dispatch_key(*key)).collect()
}

#[test]
fn ctrl_c_closes_schedules_and_never_cancels_a_timer() {
    let mut app = viewed_app();
    app.schedules.jobs = vec![crate::testing::schedule(agent_id(&app))];
    let ctrl_c = chord('c', KeyModifiers::CONTROL);
    let view = Box::new(super::schedules::SchedulesView::default());
    let effects = dispatch(&mut app, view, &[ctrl_c, ctrl_c]);
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Request {
                command: Command::UpdateSchedule { .. },
                ..
            }
        )),
        "{effects:?}"
    );
    assert!(app.bottom.view_stack.is_empty(), "the first ctrl+c closes");
}

#[test]
fn modified_letters_never_act_in_list_views() {
    let mut app = viewed_app();
    app.schedules.jobs = vec![crate::testing::schedule(agent_id(&app))];
    app.approval_rules = vec![rule("bash")];
    app.memories = vec![memory("Deploys go out on Tuesdays", false)];
    let ctrl = KeyModifiers::CONTROL;
    let alt = KeyModifiers::ALT;
    let cases: Vec<(Box<dyn BottomView>, Vec<KeyEvent>)> = vec![
        (
            Box::new(super::approvals::ApprovalsView::default()),
            vec![chord('y', ctrl), chord('d', ctrl), chord('x', alt)],
        ),
        (
            Box::new(super::schedules::SchedulesView::default()),
            vec![chord('p', ctrl), chord('c', alt), chord('c', alt)],
        ),
        (
            Box::new(super::memory::MemoryView::default()),
            vec![chord('p', ctrl), chord('f', alt), chord('f', alt)],
        ),
    ];
    for (view, keys) in cases {
        let name = format!("{view:?}");
        let effects = dispatch(&mut app, view, &keys);
        assert!(effects.is_empty(), "{name}: {effects:?}");
        assert_eq!(app.bottom.view_stack.len(), 1, "{name} stays open");
    }
    // The unmodified letter still acts.
    let view = Box::new(super::approvals::ApprovalsView::default());
    let effects = dispatch(&mut app, view, &[key(KeyCode::Char('d'))]);
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Request {
                command: Command::RevokeApprovalRule { .. },
                ..
            }
        )),
        "{effects:?}"
    );
}

#[test]
fn ctrl_c_closes_the_model_view_from_the_effort_column() {
    let mut app = viewed_app();
    app.models = vec![model("anthropic", "opus", true)];
    let view = Box::new(super::model::ModelView::new(&app));
    let keys = [key(KeyCode::Right), chord('c', KeyModifiers::CONTROL)];
    let effects = dispatch(&mut app, view, &keys);
    assert!(effects.is_empty(), "{effects:?}");
    assert!(app.bottom.view_stack.is_empty());
}

#[test]
fn schedule_next_fire_dates_and_target_names() {
    let mut app = viewed_app();
    app.now = "2026-10-08T12:00:00Z".parse().unwrap();
    let root = agent_id(&app);
    let mut second = app.tree.nodes[&root].clone();
    let id = AgentId::new();
    second.agent.id = id;
    second.agent.created_at += chrono::Duration::seconds(1);
    second.children.clear();
    app.tree.roots.push(id);
    app.tree.nodes.insert(id, second);
    let name = format!("{} 2", app.tree.nodes[&root].agent.name);
    let mut job = crate::testing::schedule(id);
    let due = app.now + chrono::Duration::days(1);
    job.status = ScheduleStatus::Active { next_due: due };
    app.schedules.jobs = vec![job];
    let view = super::schedules::SchedulesView::default();
    let text = screen_text(&draw(&view, &app, 120));
    let expected = due
        .with_timezone(&chrono::Local)
        .format("%b %d %H:%M:%S")
        .to_string();
    assert!(text.contains(&format!("next {expected}")), "{text}");
    let next_year = app.now + chrono::Duration::days(400);
    app.schedules.jobs[0].status = ScheduleStatus::Active {
        next_due: next_year,
    };
    let text = screen_text(&draw(&view, &app, 120));
    let expected = next_year
        .with_timezone(&chrono::Local)
        .format("%b %d %Y %H:%M:%S")
        .to_string();
    assert!(text.contains(&format!("next {expected}")), "{text}");
    assert!(text.contains(&format!("to {name}")), "{text}");
    app.schedules.jobs[0].status = ScheduleStatus::Active { next_due: app.now };
    let text = screen_text(&draw(&view, &app, 120));
    assert!(
        text.contains(&format!("next {}", crate::format::clock(app.now))),
        "{text}"
    );
}

#[test]
fn help_approval_rule_names_its_persistent_scope() {
    let mut app = viewed_app();
    app.approvals.clear();
    app.board.open = false;
    app.bottom.shortcuts = true;
    let text = crate::testing::render_to(&app, 120, 50);
    assert!(text.contains("agent+subagents"), "{text}");
    assert!(!text.contains("once/session"), "{text}");
}

#[test]
fn snapshot_workspace_picker_lists_projects_and_marks_the_current_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for dir in ["aether/.git", "murals/.git", "code/site/.git"] {
        std::fs::create_dir_all(tmp.path().join(dir)).expect("mkdir");
    }
    let home = crate::workspace::canonical(tmp.path());
    let mut app = stocked_app();
    let root = app.tree.roots[0];
    let node = app.tree.nodes.get_mut(&root).expect("a root");
    node.agent.scope.workspace_root = Some(home.join("murals"));
    app.selected = Some(root);
    app.options.home = Some(home.clone());
    let screen = snapshot(&mut app, |app| open(ViewKind::Workspace, app), "Workspace");
    assert!(screen.contains("where agents work"), "{screen}");
    for row in ["aether", "~/aether", "murals", "~/code/site"] {
        assert!(screen.contains(row), "{row}:\n{screen}");
    }
    app.theme = dark();
    let view = open(ViewKind::Workspace, &app);
    let terminal = draw(view.as_ref(), &app, 80);
    let rows = cursor_rows(&terminal, &app.theme);
    let text = screen_text(&terminal);
    let marked = text.lines().nth(usize::from(rows[0])).unwrap_or_default();
    assert!(
        marked.contains("murals") && marked.trim_end().ends_with("current"),
        "{text}"
    );
}

#[test]
fn doctor_reconnect_hint_preserves_the_selected_socket() {
    let mut app = viewed_app();
    app.connected = false;
    app.options.socket_path = Some("/private/operator's $socket".into());
    app.options.explicit_socket = true;
    for theme in [dark(), mono()] {
        app.theme = theme;
        let view = super::doctor::DoctorView::default();
        let text = screen_text(&draw(&view, &app, 160));
        assert!(
            text.contains("`aether --socket '/private/operator'\"'\"'s $socket' daemon`"),
            "{text}"
        );
        assert!(!text.contains("`aether daemon`"), "{text}");
    }
}

#[test]
fn doctor_provider_status_columns_use_display_width() {
    let mut app = populated_app();
    app.auth = ["猫", "a", "e\u{301}", "👩‍💻"]
        .into_iter()
        .map(|name| {
            let mut info = auth(name, true, None);
            info.provider_name = name.into();
            info
        })
        .collect();
    let rows = super::doctor::provider_lines(&app, &app.theme);
    let columns: Vec<_> = rows
        .iter()
        .map(|line| unicode_width::UnicodeWidthStr::width(line.spans[0].content.as_ref()))
        .collect();
    assert_eq!(columns, [10; 4]);
}

#[test]
fn cached_report_title_belongs_to_its_run_after_selection_changes() {
    let mut app = populated_app();
    let selected = app.selected.expect("selected agent");
    let selected_name = app.agent_display_name(selected);
    let events: Vec<aether_core::event::Event> = serde_json::from_str(include_str!(
        "../../../../aether-core/tests/fixtures/run_log.json"
    ))
    .expect("fixture log");
    let run = events
        .iter()
        .find_map(|event| event.run)
        .expect("fixture run");
    let mut report = aether_core::report::build(run, &events, now());
    report
        .body
        .agents
        .iter_mut()
        .find(|agent| agent.depth == 0)
        .expect("root")
        .name = "cached root".into();
    app.report.run = Some(run);
    app.report.report = Some(Box::new(report));
    let (title, _) = super::report::report_lines(&app, &app.theme);
    let title: String = title.iter().map(|span| span.content.as_ref()).collect();
    assert!(title.contains("cached root"), "{title}");
    assert!(!title.contains(&selected_name), "{title}");

    let root = app
        .report
        .report
        .as_mut()
        .expect("cached report")
        .body
        .agents
        .iter_mut()
        .find(|agent| agent.depth == 0)
        .expect("root");
    root.id = selected;
    let (title, _) = super::report::report_lines(&app, &app.theme);
    let title: String = title.iter().map(|span| span.content.as_ref()).collect();
    assert!(title.contains(&selected_name), "{title}");

    app.report
        .report
        .as_mut()
        .expect("cached report")
        .body
        .agents
        .clear();
    let (title, _) = super::report::report_lines(&app, &app.theme);
    let title: String = title.iter().map(|span| span.content.as_ref()).collect();
    assert!(
        title.contains(&super::short_id(&app.theme, &run)),
        "{title}"
    );
}

#[test]
fn model_picker_shows_runtime_budget_support_without_model_name_rules() {
    let mut app = stocked_app();
    for (support, expected) in [
        (Some(false), "token caps only"),
        (Some(true), "token + dollar caps"),
        (None, "budget support unknown"),
    ] {
        let mut info = model("custom", "arbitrary-model", true);
        if let Some(support) = support {
            info.capabilities
                .extensions
                .insert("aether.cost_budget".into(), support.into());
        }
        app.models = vec![info];
        for width in [59, 80, 120] {
            let view = super::model::ModelView::new(&app);
            let text = screen_text(&draw(&view, &app, width));
            assert!(text.contains(expected), "{text}");
        }
    }
}

fn lost_send(key: &str, content: &str) -> aether_client::Unresolved {
    let command = serde_json::from_value(serde_json::json!({
        "cmd": "send_message",
        "to": AgentId::new(),
        "content": content,
        "idempotency_key": key,
    }))
    .expect("a keyed send");
    aether_client::Unresolved {
        command,
        horizon: Some("epoch:3".into()),
        sent_at: now(),
        unconfirmed: Some("the host has dropped receipts recorded after it was sent".into()),
    }
}

fn settling(action: &ViewAction) -> (&str, aether_client::Settlement) {
    match effect(action) {
        Effect::Outbox {
            settle: Some((key, choice)),
        } => (key, *choice),
        other => panic!("expected a settlement, got {other:?}"),
    }
}

#[test]
fn outbox_decisions_wait_for_enter_and_name_the_highlighted_operation() {
    use aether_client::Settlement;
    let mut app = viewed_app();
    app.undelivered = vec![
        lost_send("ssh-1", "deploy now"),
        lost_send("ssh-2", "roll back"),
    ];
    let mut view = open(ViewKind::Outbox, &app);
    let text = screen_text(&draw(view.as_ref(), &app, 120));
    assert!(
        text.contains("deploy now") && text.contains("roll back"),
        "{text}"
    );
    assert!(text.contains("dropped receipts"), "{text}");

    press(view.as_mut(), &app, KeyCode::Down);
    assert!(matches!(
        press(view.as_mut(), &app, KeyCode::Char('r')),
        ViewOutcome::Consumed
    ));
    let armed = screen_text(&draw(view.as_ref(), &app, 120));
    assert!(armed.contains("it may run a second time"), "{armed}");
    let actions = acted(press(view.as_mut(), &app, KeyCode::Enter));
    assert_eq!(settling(&actions[0]), ("ssh-2", Settlement::SendAgain));

    // Anything but Enter lets an armed decision go, and nothing is sent.
    press(view.as_mut(), &app, KeyCode::Char('x'));
    assert!(matches!(
        press(view.as_mut(), &app, KeyCode::Esc),
        ViewOutcome::Consumed
    ));
    assert!(matches!(
        press(view.as_mut(), &app, KeyCode::Enter),
        ViewOutcome::Consumed | ViewOutcome::Ignored
    ));
    press(view.as_mut(), &app, KeyCode::Char('d'));
    let actions = acted(press(view.as_mut(), &app, KeyCode::Enter));
    assert_eq!(settling(&actions[0]), ("ssh-2", Settlement::MarkDelivered));
    press(view.as_mut(), &app, KeyCode::Up);
    press(view.as_mut(), &app, KeyCode::Delete);
    let actions = acted(press(view.as_mut(), &app, KeyCode::Enter));
    assert_eq!(settling(&actions[0]), ("ssh-1", Settlement::Discard));
    assert!(closed(press(view.as_mut(), &app, KeyCode::Esc)).is_empty());
}

#[test]
fn an_unreadable_outbox_file_is_listed_and_can_only_be_discarded() {
    use aether_client::Settlement;
    let mut app = viewed_app();
    app.unreadable = vec![aether_client::Unreadable {
        key: "garbage".into(),
        path: "/outbox/garbage.json".into(),
        reason: "unreadable: cannot be decoded (/outbox/garbage.json)".into(),
    }];
    let mut view = open(ViewKind::Outbox, &app);
    let text = screen_text(&draw(view.as_ref(), &app, 120));
    assert!(text.contains("unreadable file garbage"), "{text}");
    assert!(text.contains("discard only"), "{text}");

    for refused in ['d', 'r'] {
        let actions = acted(press(view.as_mut(), &app, KeyCode::Char(refused)));
        assert!(flash(&actions[0]).contains("can only be discarded"));
    }
    press(view.as_mut(), &app, KeyCode::Char('x'));
    let actions = acted(press(view.as_mut(), &app, KeyCode::Enter));
    assert_eq!(settling(&actions[0]), ("garbage", Settlement::Discard));
}

#[test]
fn outbox_with_nothing_waiting_offers_only_the_way_out() {
    let mut app = viewed_app();
    let effects = app.open_view(ViewKind::Outbox);
    assert_eq!(effects, vec![Effect::Outbox { settle: None }]);
    let mut view = open(ViewKind::Outbox, &app);
    let text = screen_text(&draw(view.as_ref(), &app, 80));
    assert!(text.contains("Nothing is waiting on a decision"), "{text}");
    let actions = acted(press(view.as_mut(), &app, KeyCode::Char('r')));
    assert!(flash(&actions[0]).contains("no operation"));
}

#[test]
fn login_status_keeps_daemon_failure_detail_for_cli_and_api_providers() {
    for provider in ["claude-code", "anthropic"] {
        let info = aether_protocol::AuthInfo {
            provider: provider.into(),
            provider_name: provider.into(),
            configured: false,
            source: None,
            methods: Vec::new(),
            subscription: false,
            detail: Some("config directory is group-writable; chmod 700 config".into()),
            program: None,
        };
        let status = super::login::status(&info, &crate::ui::theme::tokens::Theme::default());
        assert!(status.contains("not signed in"), "{status}");
        assert!(status.contains(info.detail.as_deref().unwrap()), "{status}");
    }
}

#[test]
fn usage_view_shows_remaining_credit_and_reset_time() {
    let mut app = viewed_app();
    let mut row = provider("custom-subscription");
    row.health.usage = aether_core::provider::ProviderUsage::from_engine_payload(
        &serde_json::json!({"primary":{"usedPercent":38,"windowDurationMins":300,"resetsAt":1893598200}}),
        now(),
    );
    app.providers = vec![row];
    let text = screen_text(&draw(&super::usage::UsageView::default(), &app, 160));
    assert!(
        text.contains("5h: 62% left, resets 15:30 UTC (2030-01-02)"),
        "{text}"
    );
    assert!(text.contains("custom-subscription"), "{text}");
}

/// Coloured swatch dots on the Nord and Gruvbox rows of `/theme` in `app`.
fn palette_row_dots(app: &App) -> Vec<usize> {
    let view = super::theme::ThemeView::new(app);
    let terminal = draw(&view, app, 120);
    let buffer = terminal.backend().buffer();
    let dot = app.theme.glyphs().running;
    let reset = ratatui::buffer::Cell::default().fg;
    let width = buffer.area.width;
    (0..buffer.area.height)
        .filter_map(|y| {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            (row.contains("Nord, follows") || row.contains("Gruvbox, follows")).then(|| {
                (0..width)
                    .map(|x| &buffer[(x, y)])
                    .filter(|cell| cell.symbol() == dot && cell.fg != reset)
                    .count()
            })
        })
        .collect()
}

fn mono_app(no_color_theme: bool) -> App {
    let mut app = viewed_app();
    app.theme = Theme::resolve(ThemeSpec {
        mode: ThemeMode::Mono,
        depth: ColorDepth::Truecolor,
        ..Default::default()
    });
    app.theme_choice = "mono".parse().unwrap();
    app.options.no_color_theme = no_color_theme;
    app
}

#[test]
fn theme_picker_swatches_keep_their_own_colours_while_mono_is_chosen() {
    let counts = palette_row_dots(&mono_app(false));
    assert_eq!(counts.len(), 2, "expected the Nord and Gruvbox rows");
    assert!(counts.iter().all(|&n| n > 0), "{counts:?}");
}

#[test]
fn theme_picker_swatches_stay_colourless_when_no_color_chose_mono() {
    assert_eq!(palette_row_dots(&mono_app(true)), [0, 0]);
}
