//! Fixtures for driving the reducer from tests, in the crate so integration
//! tests and benchmarks build the same app the unit tests do.

use aether_client::SubscriptionFrame;
use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::budget::RunSummary;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{
    AgentId, ApprovalId, ArtifactId, EventId, InboxItemId, InvocationId, MessageId, RunId,
    ToolCallId,
};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole, ModelRef, Usage};
use aether_core::provider::ProviderHealth;
use aether_core::time::now;
use aether_core::tool::SideEffectClass;
use aether_protocol::{AgentTree, DaemonStatus, Delta, Reply, TreeNode};

use crate::app::{App, AppEvent, Then, TuiOptions};

mod fleet;
pub use fleet::{
    loaded_app, loaded_app_with_capped_subtree, loaded_app_with_capped_subtree_under_finished_child,
};

/// Draw one frame at `w`x`h` and hand back what is on the screen, so a test
/// can assert about what a person would read.
pub fn render_to(app: &App, w: u16, h: u16) -> String {
    render_with(app, &mut crate::ui::transcript::ViewCache::default(), w, h)
}

/// [`render_to`] with the caller's caches, so scroll state carries across frames.
pub fn render_with(
    app: &App,
    cache: &mut crate::ui::transcript::ViewCache,
    w: u16,
    h: u16,
) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("a test terminal");
    terminal
        .draw(|frame| crate::ui::render(frame, app, cache))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Routes a reply to `agent`'s history page asked for on the current connection.
pub fn backfill_then(app: &App, agent: AgentId) -> Then {
    Then::Backfill {
        agent,
        connection: app.connection,
    }
}

/// Routes a reply to the latest read of `id`'s bytes.
pub fn artifact_bytes_then(app: &App, id: ArtifactId) -> Then {
    Then::ArtifactBytes {
        id,
        read: app.artifact_reads,
    }
}

/// Feed `app` a daemon status report with a chosen suspended-agent count and
/// everything else quiet, for the fleet-line and status-bar tests.
pub fn apply_status_with_suspended(app: &mut App, agents_suspended: u32) {
    reply_status(app, Then::Status, agents_suspended);
}

/// Answer the periodic status poll, as a healthy daemon does.
pub fn answer_status_poll(app: &mut App) {
    reply_status(app, Then::StatusPoll, 0);
}

fn reply_status(app: &mut App, then: Then, agents_suspended: u32) {
    app.reduce(AppEvent::Reply {
        then,
        result: Ok(Reply::Status(DaemonStatus {
            instance_id: "test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            started_at: now(),
            last_seq: 0,
            agents_live: agents_suspended,
            agents_running: 0,
            agents_waiting: 0,
            agents_blocked: agents_suspended,
            agents_suspended,
            agents_paused: 0,
            tasks_active: 0,
            pending_approvals: 0,
            default_model: None,
            usage_total: Usage::default(),
            yolo: false,
            engines: Vec::new(),
            providers: Vec::new(),
            resource_leases: Vec::new(),
            runs: Vec::new(),
            approval_default: Default::default(),
            network_default: Default::default(),
            max_concurrent_turns: None,
            max_concurrent_engine_turns: None,
            engine_turns_running: 0,
            run_budget_usd: None,
            mcp: Vec::new(),
            low_disk: None,
            bridge: None,
        })),
    });
}

/// One connected agent whose transcript alternates person and agent over
/// `messages`, in order.
pub fn app_with_transcript(messages: &[&str]) -> App {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);

    let at = now();
    let agent = Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("assistant"),
        name: "assistant".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-opus-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Running,
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
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    };
    let id = agent.id;
    let mut nodes = std::collections::BTreeMap::new();
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
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(AgentTree {
            roots: vec![id],
            nodes,
        })),
    });
    app.reduce(AppEvent::Reply {
        then: backfill_then(&app, id),
        result: Ok(Reply::Events(vec![])),
    });

    for (i, text) in messages.iter().enumerate() {
        let role = if i % 2 == 0 {
            MessageRole::User
        } else {
            MessageRole::Assistant
        };
        push_message(&mut app, id, role, text);
    }
    app
}

/// One more message from the person, streamed to the selected agent's
/// transcript as the daemon would send it.
#[allow(clippy::expect_used)]
pub fn push_user_message(app: &mut App, text: &str) {
    let agent = app.selected.expect("an agent is selected");
    push_message(app, agent, MessageRole::User, text);
}

pub fn push_message(app: &mut App, agent: AgentId, role: MessageRole, text: &str) {
    // An operator message becomes a cell when the agent consumes it from its inbox.
    let kinds = match role {
        MessageRole::User => operator_message(agent, vec![ContentBlock::text(text)]),
        _ => vec![EventKind::AgentMessage {
            role,
            content: vec![ContentBlock::text(text)],
            invocation: None,
        }],
    };
    push_events(app, agent, kinds);
}

/// A message from the person with any content, such as stored images.
#[allow(clippy::expect_used)]
pub fn push_user_content(app: &mut App, content: Vec<ContentBlock>) {
    let agent = app.selected.expect("an agent is selected");
    push_events(app, agent, operator_message(agent, content));
}

fn push_events(app: &mut App, agent: AgentId, kinds: Vec<EventKind>) {
    for kind in kinds {
        let seq = app.transcripts.get(&agent).map_or(0, |t| t.max_seq) + 1;
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(
            Event::from_draft(
                EventDraft::new(Actor::System, kind).agent(agent),
                EventId::new(),
                seq,
                now(),
            ),
        ))));
    }
}

/// The daemon's two events for a message the person sent: delivered, then consumed.
fn operator_message(agent: AgentId, content: Vec<ContentBlock>) -> Vec<EventKind> {
    let item = InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        seq: 0,
        kind: InboxKind::Message {
            message_id: MessageId::new(),
            from: Actor::user(),
            content,
        },
        delivered_at: now(),
    };
    let items = vec![item.id];
    vec![
        EventKind::InboxDelivered { item },
        EventKind::InboxConsumed { up_to: 0, items },
    ]
}

/// One agent mid-turn with a question, a reply, a tool call, a pending
/// approval and text still streaming: a screen with something on it, which
/// is the only kind worth asserting about.
pub fn populated_app() -> App {
    let mut app = App::new(TuiOptions::default());
    app.size = (120, 40);
    let id = AgentId::new();
    let at = now();
    let mut agent = Agent {
        id,
        parent: None,
        role: Role::new("assistant"),
        name: "assistant".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-opus-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Running,
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
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    };
    agent.stats.usage = Usage {
        input_tokens: 12_345,
        output_tokens: 678,
        cost_usd: 0.1234,
        ..Default::default()
    };
    let mut nodes = std::collections::BTreeMap::new();
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
    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(AgentTree {
            roots: vec![id],
            nodes,
        })),
    });
    app.reduce(AppEvent::Reply {
        then: backfill_then(&app, id),
        result: Ok(Reply::Events(vec![])),
    });
    // Fleet counts, provider load and run spend: the status bar segments only
    // exist when the daemon has reported, and the guards paint this screen.
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(DaemonStatus {
            instance_id: "test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            started_at: at,
            last_seq: 3,
            agents_live: 3,
            agents_running: 1,
            agents_waiting: 1,
            agents_blocked: 1,
            agents_suspended: 0,
            agents_paused: 1,
            tasks_active: 2,
            pending_approvals: 1,
            default_model: Some(ModelRef::new("anthropic", "claude-opus-5")),
            usage_total: Usage {
                input_tokens: 12_345,
                output_tokens: 678,
                cost_usd: 0.1234,
                ..Default::default()
            },
            yolo: false,
            engines: Vec::new(),
            providers: vec![ProviderHealth {
                provider: "anthropic".into(),
                running: 3,
                suspended_agents: 0,
                limit: Some(4),
                backoff: None,
                quota: None,
                usage: None,
                recent_errors: Vec::new(),
                last_fallback: None,
            }],
            resource_leases: Vec::new(),
            runs: vec![RunSummary {
                run: RunId::new(),
                agents: 3,
                agents_live: 3,
                usage: Usage::default(),
                started_at: at,
                rate_usd_per_hour: 2.5,
                forecast_usd_next_hour: 2.5,
            }],
            approval_default: Default::default(),
            network_default: Default::default(),
            max_concurrent_turns: None,
            max_concurrent_engine_turns: None,
            engine_turns_running: 0,
            run_budget_usd: None,
            mcp: Vec::new(),
            low_disk: None,
            bridge: None,
        })),
    });
    let ev = |seq, kind| {
        Event::from_draft(
            EventDraft::new(Actor::System, kind).agent(id),
            EventId::new(),
            seq,
            now(),
        )
    };
    for kind in operator_message(
        id,
        vec![ContentBlock::text("please list the files in this repo")],
    ) {
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(ev(
            1, kind,
        )))));
    }
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(ev(
        2,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::text("Listing now."),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ],
            invocation: None,
        },
    )))));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(ev(
        3,
        EventKind::ApprovalRequested {
            approval: ApprovalId::new(),
            call: ToolCallId::new(),
            tool: "bash".into(),
            description: "run ls".into(),
            argument: Some("ls".into()),
            side_effect: SideEffectClass::Mutation,
        },
    )))));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation: InvocationId::new(),
        delta: Delta::TurnStarted { turn: 2 },
    }));
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent: id,
        invocation: InvocationId::new(),
        delta: Delta::Text {
            text: "streaming".into(),
        },
    }));
    app
}

#[allow(clippy::expect_used)]
pub fn schedule(target: AgentId) -> aether_core::schedule::Schedule {
    use aether_core::schedule::{
        BusyPolicy, MissedRunPolicy, Recurrence, Schedule, ScheduleMode, ScheduleStatus,
    };
    let at = now();
    Schedule {
        id: aether_core::id::ScheduleId::new(),
        name: "Read docs".into(),
        owner: Actor::user(),
        mode: ScheduleMode::Deliver {
            target,
            run: RunId::new(),
            last_inbox_seq: Some(1),
        },
        recurrence: Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(60).expect("positive"),
        },
        missed: MissedRunPolicy::Coalesce,
        busy: BusyPolicy::Skip,
        payload: serde_json::json!("Read the documentation"),
        status: ScheduleStatus::Active { next_due: at },
        max_deliveries: std::num::NonZeroU64::new(3),
        deliveries: 1,
        last: None,
        created_at: at,
        updated_at: at,
    }
}
