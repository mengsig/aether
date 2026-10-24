use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::budget::RunSummary;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, RunId};
use aether_core::model::{ContentBlock, MessageRole, ModelRef, Usage};
use aether_core::provider::ProviderHealth;
use aether_core::time::now;
use aether_protocol::{AgentTree, DaemonStatus, Reply, TreeNode};

use crate::app::{App, AppEvent, Then, TuiOptions};

fn busy_agent(name: &str, parent: Option<AgentId>) -> Agent {
    let at = now();
    let mut a = Agent {
        id: AgentId::new(),
        parent,
        role: Role::new("assistant"),
        name: name.to_owned(),
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
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: AgentStats {
            turns: 1,
            ..AgentStats::default()
        },
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    };
    a.stats.usage = Usage {
        input_tokens: 12_345,
        output_tokens: 678,
        cost_usd: 0.1234,
        ..Default::default()
    };
    a
}

/// A layout fixture with `agents` under one root and `messages` turns of chat.
/// Retains the full workload independently of the live ingestion window.
pub fn loaded_app(agents: usize, events: usize, messages: usize) -> App {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);

    let root = busy_agent("orchestrator", None);
    let root_id = root.id;
    let mut nodes = std::collections::BTreeMap::new();
    let mut children = Vec::new();
    for i in 0..agents.saturating_sub(1) {
        let child = busy_agent(&format!("worker {i}"), Some(root_id));
        children.push(child.id);
        nodes.insert(
            child.id,
            TreeNode {
                agent: child,
                children: vec![],
                task: None,
                waiting_on: vec![],
                lease: None,
                subtree_usage: Usage::default(),
                hidden_usage: None,
            },
        );
    }
    nodes.insert(
        root_id,
        TreeNode {
            agent: root,
            children: children.clone(),
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
            roots: vec![root_id],
            nodes,
        })),
    });
    app.reduce(AppEvent::Reply {
        then: super::backfill_then(&app, root_id),
        result: Ok(Reply::Events(vec![])),
    });

    // A real session has the daemon's own counts too: the status bar reads
    // them, and the frame clock keys the spinner off them.
    let running = agents.min(u32::MAX as usize) as u32;
    app.reduce(AppEvent::Reply {
        then: Then::Status,
        result: Ok(Reply::Status(DaemonStatus {
            problems: Vec::new(),
            instance_id: "test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            started_at: now(),
            last_seq: 0,
            agents_live: running,
            agents_running: running,
            agents_waiting: 0,
            agents_blocked: 0,
            agents_suspended: 0,
            agents_paused: 0,
            tasks_active: 0,
            pending_approvals: 0,
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
                running,
                suspended_agents: 0,
                limit: Some(8),
                backoff: None,
                quota: None,
                usage: None,
                recent_errors: Vec::new(),
                last_fallback: None,
            }],
            resource_leases: Vec::new(),
            runs: vec![RunSummary {
                run: RunId::new(),
                agents: running,
                agents_live: running,
                usage: Usage::default(),
                started_at: now(),
                rate_usd_per_hour: 1.75,
                forecast_usd_next_hour: 1.75,
            }],
            approval_default: Default::default(),
            network_default: Default::default(),
            max_concurrent_turns: None,
            max_concurrent_engine_turns: None,
            engine_turns_running: 0,
            run_budget_usd: None,
            run_hard_stop_usd: None,
            run_hard_stop_tokens: None,
            mcp: Vec::new(),
            low_disk: None,
            writes: None,
            bridge: None,
        })),
    });

    let mut seq = 0u64;
    let mut push = |app: &mut App, kind: EventKind| {
        seq += 1;
        let event = Event::from_draft(
            EventDraft::new(Actor::System, kind).agent(root_id),
            EventId::new(),
            seq,
            now(),
        );
        let ctx = crate::app::transcript::IngestCtx {
            tree: &app.tree,
            now: app.now,
        };
        let transcript = app.transcripts.entry(root_id).or_default();
        transcript.ingest(&event, &ctx);
        transcript.max_seq = seq;
        app.last_seq = Some(seq);
    };
    for i in 0..messages {
        let question = ContentBlock::text(format!("question {i} about the build"));
        for kind in super::operator_message(root_id, vec![question]) {
            push(&mut app, kind);
        }
        push(
            &mut app,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text(format!(
                    "Answer {i}.\n\nSome **markdown** with a list:\n\n- one\n- two\n\n```rust\nfn main() {{}}\n```"
                ))],
                invocation: None,
            },
        );
    }
    for _ in 0..events.saturating_sub(messages * 3) {
        push(&mut app, EventKind::AgentReady);
    }
    app
}

/// A root with a subtree `concurrency` cap, one running child, and one
/// running grandchild under that child - so a test can tell whether a cap
/// gauge counts the whole live subtree rather than
/// only direct children, where the grandchild would be invisible.
pub fn loaded_app_with_capped_subtree(cap: usize) -> App {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);

    let mut root = busy_agent("orchestrator", None);
    root.config.concurrency = Some(cap);
    let root_id = root.id;
    let child = busy_agent("worker", Some(root_id));
    let child_id = child.id;
    let grandchild = busy_agent("sub-worker", Some(child_id));
    let grandchild_id = grandchild.id;

    let mut nodes = std::collections::BTreeMap::new();
    nodes.insert(
        grandchild_id,
        TreeNode {
            agent: grandchild,
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    nodes.insert(
        child_id,
        TreeNode {
            agent: child,
            children: vec![grandchild_id],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    nodes.insert(
        root_id,
        TreeNode {
            agent: root,
            children: vec![child_id],
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
            roots: vec![root_id],
            nodes,
        })),
    });
    app.reduce(AppEvent::Reply {
        then: super::backfill_then(&app, root_id),
        result: Ok(Reply::Events(vec![])),
    });
    app
}

/// A root with a subtree `concurrency` cap, one *finished* child, and one
/// running grandchild underneath that finished child - so a test can tell
/// whether the inspector's count still finds the grandchild (the
/// scheduler's own cap enforcement keeps a finished intermediate in the
/// ancestor chain, so a live descendant beneath it still counts against the
/// cap, and the inspector must not stop walking there).
pub fn loaded_app_with_capped_subtree_under_finished_child(cap: usize) -> App {
    let mut app = App::new(TuiOptions::default());
    app.reduce(AppEvent::Connected);

    let mut root = busy_agent("orchestrator", None);
    root.config.concurrency = Some(cap);
    let root_id = root.id;
    let mut child = busy_agent("worker", Some(root_id));
    child.status = AgentStatus::Completed { summary: None };
    let child_id = child.id;
    let grandchild = busy_agent("sub-worker", Some(child_id));
    let grandchild_id = grandchild.id;

    let mut nodes = std::collections::BTreeMap::new();
    nodes.insert(
        grandchild_id,
        TreeNode {
            agent: grandchild,
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    nodes.insert(
        child_id,
        TreeNode {
            agent: child,
            children: vec![grandchild_id],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    nodes.insert(
        root_id,
        TreeNode {
            agent: root,
            children: vec![child_id],
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
            roots: vec![root_id],
            nodes,
        })),
    });
    app.reduce(AppEvent::Reply {
        then: super::backfill_then(&app, root_id),
        result: Ok(Reply::Events(vec![])),
    });
    app
}
