//! Shared W5 test fixtures: mock c's fleet on a fixed clock.

use std::collections::BTreeMap;

use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::id::{AgentId, ApprovalId, RunId, ToolCallId};
use aether_core::model::{Effort, ModelRef, Usage};
use aether_core::time::Timestamp;
use aether_core::tool::{ApprovalRequest, ApprovalStatus, SideEffectClass};
use aether_protocol::{AgentTree, TreeNode};
use chrono::{Duration, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::app::approval_flow::MIN_VISIBLE_MS;
use crate::app::transcript::{Cell, CellId, CellModel, CellRevision};
use crate::app::{App, TuiOptions};
use crate::ui::tests_support::screen_text;

/// Mock c: `aether` (viewed) → `implementer` → `tester`, plus `researcher`.
pub(crate) struct Fleet {
    pub app: App,
    pub root: AgentId,
    pub implementer: AgentId,
    pub researcher: AgentId,
    pub tester: AgentId,
}

pub(crate) fn t0() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0)
        .single()
        .expect("a valid instant")
}

pub(crate) fn at(ms: i64) -> Timestamp {
    t0() + Duration::milliseconds(ms)
}

pub(crate) fn fleet() -> Fleet {
    let root = agent("aether", "orchestrator", None, "opus-5.5", Effort::High);
    let implementer = agent(
        "implementer",
        "worker",
        Some(root.id),
        "sonnet-5.5",
        Effort::Medium,
    );
    let researcher = agent(
        "researcher",
        "explorer",
        Some(root.id),
        "sonnet-5.5",
        Effort::Medium,
    );
    let tester = agent(
        "tester",
        "verifier",
        Some(implementer.id),
        "haiku-4.5",
        Effort::Low,
    );
    let (root_id, implementer_id, researcher_id, tester_id) =
        (root.id, implementer.id, researcher.id, tester.id);
    let mut app = App::new(TuiOptions::default());
    app.tree = AgentTree {
        roots: vec![root_id],
        nodes: BTreeMap::new(),
    };
    insert(&mut app, root, vec![implementer_id, researcher_id]);
    insert(&mut app, implementer, vec![tester_id]);
    insert(&mut app, researcher, vec![]);
    insert(&mut app, tester, vec![]);
    app.selected = Some(root_id);
    app.size = (120, 40);
    app.now = t0();
    Fleet {
        app,
        root: root_id,
        implementer: implementer_id,
        researcher: researcher_id,
        tester: tester_id,
    }
}

/// A running agent, created (and running since) ten minutes before [`t0`].
pub(crate) fn agent(
    name: &str,
    role: &str,
    parent: Option<AgentId>,
    model: &str,
    effort: Effort,
) -> Agent {
    let created = t0() - Duration::minutes(10);
    Agent {
        id: AgentId::new(),
        parent,
        role: Role::new(role),
        name: name.to_owned(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", model),
        fallbacks: Vec::new(),
        status: AgentStatus::Running,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: Some(created),
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
        created_at: created,
        updated_at: created,
    }
}

fn insert(app: &mut App, agent: Agent, children: Vec<AgentId>) {
    app.tree.nodes.insert(
        agent.id,
        TreeNode {
            agent,
            children,
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
}

/// Add `agent` under `parent` in the tree.
pub(crate) fn add_child(app: &mut App, parent: AgentId, agent: Agent) -> AgentId {
    let id = agent.id;
    insert(app, agent, vec![]);
    app.tree
        .nodes
        .get_mut(&parent)
        .expect("parent in tree")
        .children
        .push(id);
    id
}

pub(crate) fn node_mut(app: &mut App, agent: AgentId) -> &mut Agent {
    &mut app.tree.nodes.get_mut(&agent).expect("agent in tree").agent
}

/// A pending shell approval from `agent`, asked at `requested_at`.
pub(crate) fn shell_approval(
    agent: AgentId,
    command: &str,
    requested_at: Timestamp,
) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: agent,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "bash".to_owned(),
        description: format!("bash {{\"command\":{command:?}}}"),
        argument: Some(command.to_owned()),
        resources: Vec::new(),
        side_effect: SideEffectClass::Mutation,
        requested_at,
        status: ApprovalStatus::Pending,
    }
}

/// Push a committed cell at `at` onto `agent`'s transcript.
pub(crate) fn push_cell(app: &mut App, agent: AgentId, at: Timestamp, model: CellModel) {
    let transcript = app.transcripts.entry(agent).or_default();
    let id: CellId = transcript.next_cell_id();
    transcript.cells.push(Cell {
        id,
        revision: CellRevision::default(),
        at,
        model,
    });
}

/// The operator reads the card: it is drawn, then on screen for [`MIN_VISIBLE_MS`].
pub(crate) fn read_card(app: &mut App) {
    app.track_approvals();
    app.now += Duration::milliseconds(MIN_VISIBLE_MS);
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Draw into a `w`×`h` test terminal.
pub(crate) fn draw(
    w: u16,
    h: u16,
    paint: impl FnOnce(&mut ratatui::Frame),
) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("a test terminal");
    terminal.draw(paint).expect("draw");
    terminal
}

/// Screen rows with trailing blanks trimmed.
pub(crate) fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    screen_text(terminal)
        .lines()
        .map(|row| row.trim_end().to_owned())
        .collect()
}

/// The cell at (`x`, `y`) is painted exactly with `expected` over a blank cell.
#[track_caller]
pub(crate) fn assert_style(terminal: &Terminal<TestBackend>, x: u16, y: u16, expected: Style) {
    let actual = terminal.backend().buffer()[(x, y)].clone();
    let mut want = actual.clone();
    want.reset();
    want.set_symbol(actual.symbol());
    want.set_style(expected);
    assert_eq!(actual, want, "cell ({x}, {y}) {:?}", actual.symbol());
}

/// Column of the first `needle` on row `y`.
#[track_caller]
pub(crate) fn column_of(terminal: &Terminal<TestBackend>, y: u16, needle: &str) -> u16 {
    let row = &rows(terminal)[usize::from(y)];
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    u16::try_from(row[..byte].width()).expect("fits")
}
