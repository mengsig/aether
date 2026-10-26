use super::*;
use aether_core::agent::{AgentConfig, AgentStats, AgentStatus, Scope};
use aether_core::event::{Actor, EventDraft};
use aether_core::id::{EventId, InvocationId, RunId, ToolCallId};
use aether_core::model::Usage;
use aether_core::provider::QuotaWindow;
use aether_core::tool::{ApprovalDecision, SideEffectClass, ToolCall, ToolOutcome};
use aether_protocol::TreeNode;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::transcript::{Cell, CellModel};

fn agent(id: AgentId, name: &str) -> Agent {
    let at = now();
    Agent {
        id,
        parent: None,
        role: Role::new("assistant"),
        name: name.into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
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
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

fn tree(agent: Agent) -> AgentTree {
    let mut nodes = std::collections::BTreeMap::new();
    let id = agent.id;
    nodes.insert(
        id,
        TreeNode {
            agent,
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    AgentTree {
        roots: vec![id],
        nodes,
    }
}

fn event(seq: u64, agent: AgentId, kind: EventKind) -> Event {
    Event::from_draft(
        EventDraft::new(Actor::System, kind).agent(agent),
        EventId::new(),
        seq,
        now(),
    )
}

/// The `Then` a root start's reply carries: its create request's, or that of
/// the status request it waits on first.
fn start_then(effects: &[Effect]) -> Then {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::CreateAgent(_),
                then,
            }
            | Effect::Request {
                command: Command::Status,
                then: then @ Then::RootDefaults { .. },
            } => Some(then.clone()),
            _ => None,
        })
        .expect("a root start")
}

pub(super) fn key(code: KeyCode) -> AppEvent {
    AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn app_with_status(options: TuiOptions) -> App {
    let mut app = App::new(options);
    app.size = (120, 40);
    app.status = Some(crate::ui::tests_support::quiet_status());
    app
}

fn app_with_agent() -> (App, AgentId) {
    let mut app = app_with_status(TuiOptions::default());
    let id = AgentId::new();
    let effects = app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    assert!(matches!(effects[0], Effect::Subscribe { after: None }));
    app.status = Some(crate::ui::tests_support::quiet_status());
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(agent(id, "root")))),
    });
    // Selecting the root requests its history.
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::GetEvents { .. },
            ..
        }
    )));
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, id),
        result: Ok(Reply::Events(vec![])),
    });
    assert_eq!(app.selected, Some(id));
    (app, id)
}

fn revoked_rule(id: aether_core::id::ApprovalRuleId) -> ApprovalRule {
    let mut r = rule("bash");
    r.id = id;
    r.revoked = Some(aether_core::approval::Revocation {
        at: aether_core::time::now(),
        by: aether_core::event::Actor::user(),
        reason: None,
    });
    r
}

fn base64_of(text: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(text)
}

pub(super) fn artifact(name: &str) -> Artifact {
    Artifact {
        id: aether_core::id::ArtifactId::new(),
        kind: aether_core::artifact::ArtifactKind::CommandOutput,
        name: name.into(),
        path: None,
        summary: String::new(),
        size_bytes: 12,
        content_type: None,
        digest: None,
        created_by: aether_core::event::Actor::System,
        agent_id: None,
        task_id: None,
        created_at: aether_core::time::now(),
        evicted_at: None,
    }
}

fn composer_fixture(pending: bool) -> App {
    let (mut app, id) = app_with_agent();
    if pending {
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
            7,
            id,
            EventKind::ApprovalRequested {
                approval: ApprovalId::new(),
                call: ToolCallId::new(),
                tool: "bash".into(),
                description: "cargo test".into(),
                argument: Some("cargo test".into()),
                side_effect: SideEffectClass::Mutation,
            },
        )))));
    }
    app
}

/// The asker records a question at seq 9, before any answer below.
fn ask_a_question(app: &mut App, id: AgentId) {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        9,
        id,
        EventKind::QuestionAsked {
            question: "which colour?".into(),
        },
    )))));
    assert!(!question_answered(app));
}

/// The viewed agent's cells, committed then live, in display order.
fn cells(app: &App) -> Vec<Cell> {
    let id = app.selected.expect("an agent is viewed");
    app.transcripts
        .get(&id)
        .map(|t| t.all_cells().cloned().collect())
        .unwrap_or_default()
}

fn models(app: &App) -> Vec<CellModel> {
    cells(app).into_iter().map(|c| c.model).collect()
}

/// The viewed transcript's (single) question; panics if it went missing,
/// which is its own bug.
fn the_question(app: &App) -> Cell {
    cells(app)
        .into_iter()
        .find(|c| matches!(c.model, CellModel::Question { .. }))
        .expect("the question is still in the transcript")
}

fn question_answered(app: &App) -> bool {
    match the_question(app).model {
        CellModel::Question { state, .. } => {
            state == crate::app::transcript::QuestionState::Answered
        }
        other => unreachable!("{other:?}"),
    }
}

fn ctrl_key(code: KeyCode) -> AppEvent {
    AppEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

fn shift_key(code: KeyCode) -> AppEvent {
    AppEvent::Key(KeyEvent::new(code, KeyModifiers::SHIFT))
}

mod operator_commands;

mod host_commands;

mod preferences;

mod archiving;

mod models;

mod memory;

mod composer;

#[cfg(unix)]
mod clipboard_paste;
mod images;

mod streaming;

mod streaming_failure;

mod backfill;

mod parent_history;

mod tool_results;

mod approvals;

mod artifact_lifecycle;

mod artifact_requests;

mod usage;

mod schedules;

mod connection;

mod questions;

mod steering;

mod agent_actions;

mod accounting;
mod state_regressions;

mod workspaces;

pub(super) fn memory_record(forgotten: bool) -> aether_core::memory::MemoryRecord {
    let mut record = aether_core::memory::MemoryRecord::from_candidate(
        aether_core::memory::MemoryCandidate {
            scope: aether_core::memory::MemoryScope::Global,
            kind: aether_core::memory::MemoryKind::Fact,
            text: "Deploys go out on Tuesday afternoons".into(),
            structured: None,
            importance: 0.5,
            confidence: 0.5,
            tags: vec![],
            provenance: Default::default(),
            supersedes: None,
        },
        now(),
    );
    record.forgotten_at = forgotten.then(now);
    record
}

pub(super) fn rule(tool: &str) -> ApprovalRule {
    ApprovalRule {
        id: aether_core::id::ApprovalRuleId::new(),
        decision: ApprovalDecision::Approved,
        matcher: aether_core::approval::RuleMatch::tool(tool),
        scope: aether_core::approval::RuleScope::Everywhere,
        created_at: aether_core::time::now(),
        created_by: aether_core::event::Actor::user(),
        note: None,
        revoked: None,
    }
}

#[test]
fn daemon_error_hints_preserve_the_selected_socket() {
    let socket = std::path::PathBuf::from("/private/operator's $socket");
    let prefix = "aether --socket '/private/operator'\"'\"'s $socket'";
    for ascii in [false, true] {
        for (error, action) in [
            (ClientError::Incompatible("version mismatch".into()), "stop"),
            (
                ClientError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
                "daemon",
            ),
            (
                ClientError::TimedOut {
                    command: "hello".into(),
                    timeout: std::time::Duration::from_secs(1),
                },
                "stop",
            ),
            (
                ClientError::TimedOut {
                    command: "status".into(),
                    timeout: std::time::Duration::from_secs(1),
                },
                "stop",
            ),
        ] {
            let timeout_command = match &error {
                ClientError::TimedOut { command, .. } => Some(command.clone()),
                _ => None,
            };
            let mut app = App::new(TuiOptions {
                socket_path: Some(socket.clone()),
                explicit_socket: true,
                ascii,
                ..Default::default()
            });
            app.reduce(AppEvent::Reply {
                then: Then::Ignore,
                result: Err(error),
            });
            let message = &app.flash.as_ref().expect("error flash").text;
            if timeout_command.as_deref() == Some("hello") {
                assert!(
                    message.contains("overloaded or from an older build"),
                    "{message}"
                );
            } else if timeout_command.as_deref() == Some("status") {
                assert!(message.contains("connected but slow"), "{message}");
                assert!(message.contains("try again"), "{message}");
            }
            assert!(
                message.contains(&format!("`{prefix} {action}`")),
                "{message}"
            );
            assert!(
                !message.contains("`aether stop") && !message.contains("`aether daemon"),
                "{message}"
            );
        }
    }
}

#[test]
fn unprintable_socket_error_hints_do_not_offer_a_default_daemon_command() {
    use std::os::unix::ffi::OsStrExt;
    for socket in [
        std::path::PathBuf::from("/private/socket\ncontrol"),
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/private/socket-\xff")),
    ] {
        let mut app = App::new(TuiOptions {
            socket_path: Some(socket),
            explicit_socket: true,
            ..Default::default()
        });
        app.reduce(AppEvent::Reply {
            then: Then::Ignore,
            result: Err(ClientError::Incompatible("version mismatch".into())),
        });
        let message = &app.flash.as_ref().expect("error flash").text;
        assert!(message.contains("`# socket path"), "{message}");
        assert!(!message.contains("`aether "), "{message}");
        assert!(!message.contains(['\n', '\u{1b}']), "{message}");
    }
}
