#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Every frame the TUI can draw: it renders at any size, it never paints
//! outside itself, and the layout rules of the redesign (SPEC §1-§3) hold.
//! This is what `scripts/tui_capture.sh` checks by eye, checked by machine.

use aether_core::agent::AgentStatus;
use aether_core::id::AgentId;
use aether_tui::app::{App, AppEvent};
use aether_tui::testing::{app_with_transcript, loaded_app, populated_app};
use aether_tui::ui;
use aether_tui::ui::theme::ColorDepth;
use aether_tui::ui::theme::tokens::{Rgb, TerminalPalette, Theme, ThemeMode, ThemeSpec};
use aether_tui::ui::transcript::ViewCache;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Style};

/// Widths and heights worth caring about: the reference size, a laptop
/// terminal, the docking threshold either side, and something absurd.
const SIZES: [(u16, u16); 7] = [
    (160, 44),
    (120, 40),
    (100, 30),
    (99, 30),
    (80, 24),
    (60, 18),
    (30, 8),
];

/// One frame of `app` at `w`x`h`, as the buffer a terminal would show.
fn draw(app: &App, w: u16, h: u16) -> Buffer {
    draw_with(app, &mut ViewCache::default(), w, h)
}

fn draw_with(app: &App, cache: &mut ViewCache, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    terminal
        .draw(|frame| ui::render(frame, app, cache))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn rows(buffer: &Buffer) -> Vec<String> {
    let width = buffer.area.width as usize;
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|c| c.symbol()).collect())
        .collect()
}

/// The distinct non-ASCII characters in `text`, for a failure message.
fn non_ascii(text: &str) -> String {
    let mut found: Vec<char> = text.chars().filter(|c| !c.is_ascii()).collect();
    found.sort_unstable();
    found.dedup();
    found.into_iter().collect()
}

/// `app` drawn at `w`x`h`, one string per row.
fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
    rows(&draw(app, w, h))
}

/// Index of the first row containing `needle`.
fn row_with(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(needle))
}

/// Column (in cells, not bytes) where `needle` starts in `row`.
fn column_of(row: &str, needle: &str) -> Option<u16> {
    let byte = row.find(needle)?;
    Some(row[..byte].chars().count() as u16)
}

/// Whether a drawn cell shows `style`'s colours and at least its modifiers;
/// unset colours read as `Reset`, and a tinted block may add a background.
fn shows(cell: &Cell, style: Style) -> bool {
    cell.fg == style.fg.unwrap_or(Color::Reset)
        && style.bg.is_none_or(|bg| cell.bg == bg)
        && cell.modifier.contains(style.add_modifier)
}

fn theme(mode: ThemeMode, ascii: bool) -> Theme {
    Theme::resolve(ThemeSpec {
        mode,
        ascii,
        ..ThemeSpec::default()
    })
}

/// A dark truecolor terminal, so the tint and accent are real colours to compare.
fn dark(ascii: bool) -> Theme {
    Theme::resolve(ThemeSpec {
        mode: ThemeMode::Dark,
        depth: ColorDepth::Truecolor,
        palette: Some(TerminalPalette {
            fg: Rgb(0xe6, 0xed, 0xf3),
            bg: Rgb(0x0d, 0x11, 0x17),
        }),
        ascii,
        ..ThemeSpec::default()
    })
}

/// The viewed agent's id.
fn viewed(app: &App) -> AgentId {
    app.selected.expect("a viewed agent")
}

/// Puts the viewed agent in `status` and lets the board catch up.
fn set_status(app: &mut App, status: AgentStatus) {
    let id = viewed(app);
    app.tree
        .nodes
        .get_mut(&id)
        .expect("the viewed node")
        .agent
        .status = status;
    app.rebuild_after_tree_change();
}

/// Hints only: the fixtures connect, and `connected` flashes over them.
fn quiet(mut app: App) -> App {
    app.flash = None;
    app
}

/// Puts the due approval card on screen and lets it be read, so answer keys count.
fn read_card(app: &mut App) {
    app.reduce(AppEvent::Resize(app.size.0, app.size.1));
    app.now += chrono::Duration::milliseconds(aether_tui::app::approval_flow::MIN_VISIBLE_MS);
}

fn key(code: KeyCode) -> AppEvent {
    AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(c: char) -> AppEvent {
    AppEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// Remembered decisions long enough that a narrow view has to elide them.
fn approval_rules() -> Vec<aether_core::approval::ApprovalRule> {
    use aether_core::approval::{ApprovalRule, RuleMatch, RuleScope};
    use aether_core::tool::ApprovalDecision;
    [
        (ApprovalDecision::Approved, RuleScope::Everywhere),
        (
            ApprovalDecision::Denied,
            RuleScope::Subtree {
                root: aether_core::id::AgentId::new(),
            },
        ),
    ]
    .into_iter()
    .map(|(decision, scope)| ApprovalRule {
        id: aether_core::id::ApprovalRuleId::new(),
        decision,
        matcher: RuleMatch {
            argument: Some("git status --porcelain --untracked-files=all".into()),
            ..RuleMatch::tool("bash")
        },
        scope,
        created_at: aether_core::time::now(),
        created_by: aether_core::event::Actor::user(),
        note: Some("read-only git, kept so the agent stops asking every turn".into()),
        revoked: None,
    })
    .collect()
}

/// One server that answers and one that does not, with names, transports,
/// tool names and an error long enough that a narrow view has to elide them.
fn mcp_servers() -> Vec<aether_core::mcp::McpServerHealth> {
    use aether_core::mcp::{McpServerHealth, McpServerState, McpToolInfo, qualified_name};
    use aether_core::tool::SideEffectClass;
    let tool = |name: &str, class, declared| McpToolInfo {
        name: name.to_owned(),
        qualified_name: qualified_name("notes-and-issues", name),
        side_effect: class,
        declared,
    };
    vec![
        McpServerHealth {
            tools: vec![
                tool("read_note", SideEffectClass::ReadOnly, true),
                tool("write_note", SideEffectClass::Mutation, false),
            ],
            state: McpServerState::Ready,
            ..McpServerHealth::unknown(
                "notes-and-issues",
                "stdio: npx -y @modelcontextprotocol/server-filesystem /srv/data",
            )
        },
        McpServerHealth {
            state: McpServerState::Failed,
            error: Some(
                "connection refused after 5s: no process is listening on 127.0.0.1:9100".into(),
            ),
            ..McpServerHealth::unknown("issues", "http://127.0.0.1:9100/mcp")
        },
    ]
}

/// A memory and the one it superseded, with text, tags and provenance long
/// enough that a narrow view has to elide them, and a chain to resolve.
fn memory_records() -> Vec<aether_core::memory::MemoryRecord> {
    use aether_core::memory::{
        MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope, MemorySource,
    };
    let old = MemoryRecord {
        id: aether_core::id::MemoryId::new(),
        scope: MemoryScope::Repository {
            id: "aether".into(),
        },
        kind: MemoryKind::Lesson,
        text: "the sqlite store serializes every write behind one mutex, so a slow migration blocks the whole daemon".into(),
        structured: None,
        importance: 0.6,
        confidence: 0.8,
        tags: vec!["sqlite".into(), "migrations".into()],
        provenance: MemoryProvenance {
            source: Some(MemorySource {
                file: ".aether/knowledge/store.md".into(),
                commit: Some("abcdef1234567890".into()),
            }),
            ..MemoryProvenance::default()
        },
        supersedes: None,
        superseded_by: None,
        created_at: aether_core::time::now(),
        last_retrieved_at: None,
        proposed_at: None,
        accepted_at: None,
        retrieval_count: 0,
        pinned: false,
        forgotten_at: None,
        retracted: None,
    };
    let mut new = old.clone();
    new.id = aether_core::id::MemoryId::new();
    new.text = "the sqlite store now runs migrations on a dedicated connection, off the write mutex, so a slow migration no longer blocks live traffic".into();
    new.supersedes = Some(old.id);
    new.pinned = true;
    let mut old_superseded = old;
    old_superseded.superseded_by = Some(new.id);
    vec![new, old_superseded]
}

/// `populated_app` with something in every list a slash view shows.
fn stocked_app() -> App {
    use aether_core::model::ModelRef;
    use aether_protocol::{AuthInfo, ModelInfo};
    let mut app = quiet(populated_app());
    let agent = viewed(&app);
    let current = app.tree.nodes[&agent].agent.model.clone();
    app.models = [current, ModelRef::new("google", "gemini")]
        .into_iter()
        .map(|model| ModelInfo {
            tier: None,
            aliases: Vec::new(),
            name: model.model.clone(),
            provider_name: format!("{} cloud", model.provider),
            model,
            capabilities: Default::default(),
            configured: true,
            auth_source: None,
            is_default: false,
        })
        .collect();
    app.auth = vec![AuthInfo {
        provider: "openai".into(),
        provider_name: "openai cloud".into(),
        configured: true,
        source: Some("API key".into()),
        methods: vec!["api_key".into()],
        subscription: false,
        detail: None,
        program: None,
    }];
    app.approval_rules = approval_rules();
    app.mcp_servers = mcp_servers();
    app.memories = memory_records();
    app.schedules.jobs = vec![aether_tui::testing::schedule(agent)];
    app.artifacts.for_agent = Some(agent);
    app.artifacts.list = vec![artifact("cargo-test-output-with-a-long-name.log")];
    app
}

fn artifact(name: &str) -> aether_core::artifact::Artifact {
    use aether_core::artifact::{Artifact, ArtifactKind};
    Artifact {
        id: aether_core::id::ArtifactId::new(),
        kind: ArtifactKind::CommandOutput,
        name: name.into(),
        path: Some(format!("/store/{name}").into()),
        summary: "full output of cargo test (84000 chars), kept out of the model's context".into(),
        size_bytes: 86_016,
        content_type: Some("text/plain".into()),
        digest: None,
        created_by: aether_core::event::Actor::System,
        agent_id: None,
        task_id: None,
        created_at: aether_core::time::now(),
        evicted_at: None,
    }
}

#[path = "frames/layout.rs"]
mod layout;

#[path = "frames/status.rs"]
mod status;

#[path = "frames/composer.rs"]
mod composer;

#[path = "frames/board.rs"]
mod board;

#[path = "frames/views.rs"]
mod views;
