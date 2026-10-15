//! Headless ingestion: one test per §3.1 row family and per grouping rule.

use aether_core::agent::{AgentStatusKind, Role, Scope, WaitCondition};
use aether_core::approval::{ApprovalRule, RuleMatch, RuleScope};
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{
    ApprovalId, ApprovalRuleId, EventId, InboxItemId, InvocationId, MessageId, ScheduleId, TaskId,
    ToolCallId,
};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole, ModelErrorClass, ModelRef, Usage};
use aether_core::provider::QuotaWindow;
use aether_core::task::{Priority, TaskResult};
use aether_core::time::{Timestamp, now};
use aether_core::tool::{ApprovalDecision, SideEffectClass, ToolCall, ToolOutcome};
use aether_protocol::{AgentTree, Delta};
use chrono::Duration;
use serde_json::{Value, json};

use super::*;

/// Drives events through `ingest_event` the way the daemon orders them.
struct Feed {
    transcripts: Transcripts,
    tree: AgentTree,
    seq: u64,
    clock: Timestamp,
    alerts: Vec<String>,
    events: Vec<Event>,
}

impl Feed {
    fn new() -> Self {
        Self {
            transcripts: Transcripts::new(),
            tree: AgentTree::default(),
            seq: 0,
            clock: now(),
            alerts: Vec::new(),
            events: Vec::new(),
        }
    }

    fn wait(&mut self, secs: i64) {
        self.clock += Duration::seconds(secs);
    }

    fn send(&mut self, draft: EventDraft) {
        self.seq += 1;
        let event = Event::from_draft(draft, EventId::new(), self.seq, self.clock);
        self.feed(&event);
    }

    fn feed(&mut self, event: &Event) {
        self.events.push(event.clone());
        let ctx = IngestCtx {
            tree: &self.tree,
            now: self.clock,
        };
        if let Some(alert) = ingest_event(&mut self.transcripts, event, &ctx) {
            self.alerts.push(alert);
        }
    }

    fn on(&mut self, agent: AgentId, kind: EventKind) {
        self.send(EventDraft::new(Actor::System, kind).agent(agent));
    }

    /// `AgentCreated`, `TaskCreated`, `TaskAssigned`, as one commit lands them.
    fn spawn(&mut self, parent: Option<AgentId>, name: &str, role: &str) -> (AgentId, TaskId) {
        let id = AgentId::new();
        let task = TaskId::new();
        let actor = parent.map(Actor::agent).unwrap_or_else(Actor::user);
        let created = EventKind::AgentCreated {
            role: Role::new(role),
            name: name.into(),
            parent,
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            fallbacks: Vec::new(),
            scope: Scope::default(),
        };
        self.send(
            EventDraft::new(actor.clone(), created)
                .agent(id)
                .parent(parent),
        );
        let objective = EventKind::TaskCreated {
            objective: format!("{name}: do the thing\nwith detail"),
            parent_task: None,
            dependencies: Vec::new(),
            priority: Priority::Normal,
        };
        self.send(
            EventDraft::new(actor.clone(), objective)
                .agent(id)
                .task(task)
                .parent(parent),
        );
        self.send(
            EventDraft::new(actor, EventKind::TaskAssigned { owner: id })
                .agent(id)
                .task(task)
                .parent(parent),
        );
        (id, task)
    }

    fn tool(&mut self, agent: AgentId, name: &str, input: Value) -> ToolCallId {
        let call = ToolCall {
            id: ToolCallId::new(),
            provider_call_id: "toolu_1".into(),
            name: name.into(),
            input,
            agent_id: agent,
            task_id: None,
            invocation_id: None,
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: self.clock,
        };
        let id = call.id;
        self.on(
            agent,
            EventKind::ToolRequested {
                call,
                side_effect: SideEffectClass::ReadOnly,
            },
        );
        id
    }

    fn tool_ok(&mut self, agent: AgentId, call: ToolCallId, outcome: ToolOutcome) {
        self.on(agent, EventKind::ToolSucceeded { call, outcome });
    }

    fn operator_message(&mut self, agent: AgentId, text: &str) -> InboxItemId {
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: agent,
            seq: self.seq + 1,
            kind: InboxKind::Message {
                message_id: MessageId::new(),
                from: Actor::user(),
                content: vec![ContentBlock::Text { text: text.into() }],
            },
            delivered_at: self.clock,
        };
        let id = item.id;
        self.on(agent, EventKind::InboxDelivered { item });
        id
    }

    fn usage(&mut self, agent: AgentId, total_usd: f64, turn_usd: f64) {
        let usage = |usd| Usage {
            cost_usd: usd,
            ..Usage::default()
        };
        self.on(
            agent,
            EventKind::AgentUsageUpdated {
                total: usage(total_usd),
                turn_usage: usage(turn_usd),
            },
        );
    }

    fn t(&self, agent: AgentId) -> &AgentTranscript {
        &self.transcripts[&agent]
    }

    fn models(&self, agent: AgentId) -> Vec<&CellModel> {
        self.t(agent).all_cells().map(|c| &c.model).collect()
    }
}

fn reply(text: &str) -> EventKind {
    EventKind::AgentMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Text { text: text.into() }],
        invocation: None,
    }
}

fn turn_done(next: AgentStatusKind) -> EventKind {
    EventKind::AgentTurnCompleted {
        turn: 1,
        invocation: None,
        next,
    }
}

fn read(path: &str) -> Value {
    json!({ "path": path })
}

#[test]
fn operator_task_and_reply_become_user_and_assistant_cells() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    feed.on(root, reply("  Done.  "));
    let cells = feed.models(root);
    assert_eq!(
        cells,
        vec![
            &CellModel::User {
                from: UserFrom::Operator,
                text: "aether: do the thing\nwith detail".into()
            },
            &CellModel::Assistant {
                text: "Done.".into()
            },
        ]
    );
}

#[test]
fn reasoning_commits_before_the_reply_with_its_title_and_duration() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    let started = feed.clock;
    feed.transcripts.get_mut(&root).unwrap().stream(
        invocation,
        &Delta::Thinking {
            text: "**Planning the fix** first".into(),
        },
        started,
    );
    assert_eq!(
        feed.t(root).stream.reasoning_title.as_deref(),
        Some("Planning the fix")
    );
    feed.wait(3);
    feed.on(
        root,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "**Planning the fix** first".into(),
                    signature: None,
                    redacted: false,
                },
                ContentBlock::Text {
                    text: "Fixed.".into(),
                },
            ],
            invocation: Some(invocation),
        },
    );
    let t = feed.t(root);
    assert!(t.stream.is_empty(), "the committed reply clears the stream");
    let n = t.cells.len();
    assert_eq!(
        t.cells[n - 2].model,
        CellModel::Reasoning {
            title: Some("Planning the fix".into()),
            text: "**Planning the fix** first".into(),
            duration_ms: Some(3_000),
        }
    );
    assert_eq!(
        t.cells[n - 1].model,
        CellModel::Assistant {
            text: "Fixed.".into()
        }
    );
}

#[test]
fn consecutive_explore_calls_merge_with_names_deduped_until_another_cell() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let a = feed.tool(root, "read_file", read("src/app/a.rs"));
    feed.tool_ok(root, a, ToolOutcome::ok("…"));
    let b = feed.tool(root, "Read", json!({ "file_path": "src/app/b.rs" }));
    let again = feed.tool(root, "read_file", read("src/app/a.rs"));
    let grep = feed.tool(
        root,
        "grep",
        json!({ "pattern": "list_runs", "path": "src" }),
    );
    {
        let active = feed.t(root).active.as_ref().expect("an open group");
        let CellModel::Explored { items, active } = &active.model else {
            panic!("expected an explore group, got {:?}", active.model);
        };
        assert!(*active, "calls are still running");
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0].verb, ExploreVerb::Read);
        assert_eq!(items[0].targets, vec!["app/a.rs", "app/b.rs"]);
        assert_eq!(items[1].verb, ExploreVerb::Search);
    }
    for call in [b, again, grep] {
        feed.tool_ok(root, call, ToolOutcome::ok(""));
    }
    assert!(matches!(
        feed.t(root).active.as_ref().map(|c| &c.model),
        Some(CellModel::Explored { active: false, .. })
    ));

    feed.tool(root, "bash", json!({ "command": "cargo check" }));
    let cells = feed.models(root);
    assert!(matches!(cells[cells.len() - 2], CellModel::Explored { .. }));
    assert!(matches!(cells[cells.len() - 1], CellModel::Exec { .. }));
    assert!(feed.t(root).active.is_none(), "an exec closes the group");

    feed.tool(root, "glob", json!({ "pattern": "*.rs" }));
    let explored = feed
        .models(root)
        .iter()
        .filter(|m| matches!(m, CellModel::Explored { .. }))
        .count();
    assert_eq!(explored, 2, "a closed group is never reopened");
}

#[test]
fn exec_settles_with_exit_code_and_search_no_match_is_not_a_failure() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let call = feed.tool(root, "bash", json!({ "command": "cargo test" }));
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Exec {
            status: CallStatus::Running,
            ..
        })
    ));
    feed.tool_ok(
        root,
        call,
        ToolOutcome::error("test failed\n[exit code: 101]").with_duration(1200),
    );
    let Some(CellModel::Exec {
        status,
        output,
        exit,
        duration_ms,
        ..
    }) = feed.models(root).last().cloned()
    else {
        panic!("expected an exec cell");
    };
    assert_eq!(*status, CallStatus::Failed);
    assert_eq!(output, "test failed");
    assert_eq!(*exit, Some(101));
    assert_eq!(*duration_ms, Some(1200));

    let search = feed.tool(root, "bash", json!({ "command": "rg missing src" }));
    feed.tool_ok(root, search, ToolOutcome::error("[exit code: 1]"));
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Exec {
            status: CallStatus::Succeeded,
            exit: Some(1),
            is_search: true,
            ..
        })
    ));
}

#[test]
fn edits_and_mcp_calls_take_their_outcome() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let edit = feed.tool(
        root,
        "edit_file",
        json!({ "path": "src/lib.rs", "old_string": "a\nb\n", "new_string": "a\nc\n" }),
    );
    feed.tool_ok(
        root,
        edit,
        ToolOutcome::ok("Edited src/lib.rs (1 replacement)"),
    );
    let Some(CellModel::Edited { files, status }) = feed.models(root).last().cloned() else {
        panic!("expected an edited cell");
    };
    assert_eq!(*status, CallStatus::Succeeded);
    assert_eq!((files[0].added, files[0].removed), (1, 1));

    let call = feed.tool(root, "mcp__github__get_issue", json!({ "number": 7 }));
    feed.tool_ok(root, call, ToolOutcome::error("not found"));
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::Called {
            target: CallTarget::Mcp {
                server: "github".into(),
                tool: "get_issue".into()
            },
            args: "number: 7".into(),
            result: String::new(),
            status: CallStatus::Failed,
            error: Some("not found".into()),
            artifacts: Vec::new(),
        })
    );
}

#[test]
fn harness_calls_stay_hidden_unless_they_fail() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let before = feed.t(root).cells.len();
    let ok = feed.tool(root, "mcp__aether__inspect_state", json!({}));
    feed.tool_ok(root, ok, ToolOutcome::ok("state"));
    assert_eq!(feed.t(root).cells.len(), before);

    let bad = feed.tool(root, "send_message", json!({ "to": "x" }));
    feed.on(
        root,
        EventKind::ToolFailed {
            call: bad,
            error: "no such agent".into(),
            retryable: false,
            duration_ms: None,
        },
    );
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Called {
            target: CallTarget::Tool { name },
            status: CallStatus::Failed,
            ..
        }) if name == "send_message"
    ));
}

#[test]
fn spawn_wait_and_child_result_land_in_the_parent_transcript() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    feed.tool(
        root,
        "delegate",
        json!({ "role": "explorer", "name": "researcher", "objective": "look", "effort": "medium" }),
    );
    let (child, task) = feed.spawn(Some(root), "researcher", "explorer");
    let spawned = feed
        .models(root)
        .into_iter()
        .find(|m| matches!(m, CellModel::Spawned { .. }))
        .cloned()
        .expect("a spawned cell in the parent");
    assert_eq!(
        spawned,
        CellModel::Spawned {
            agent: child,
            name: "researcher".into(),
            role: "explorer".into(),
            model: "sonnet-5.5".into(),
            effort: Some(aether_core::model::Effort::Medium),
            objective: "researcher: do the thing\nwith detail".into(),
        }
    );
    assert!(
        !feed
            .models(child)
            .iter()
            .any(|m| matches!(m, CellModel::Spawned { .. })),
        "the child's own transcript starts with its task, not its spawn"
    );

    feed.on(
        root,
        EventKind::AgentWaiting {
            condition: WaitCondition::Task { id: task },
        },
    );
    assert!(matches!(
        feed.t(root).active.as_ref().map(|c| &c.model),
        Some(CellModel::Waiting { on, ended: None, .. }) if on == &vec![(child, "researcher".to_owned())]
    ));

    feed.on(child, EventKind::AgentStarted { turn: 1 });
    feed.usage(child, 0.21, 0.21);
    feed.wait(63);
    feed.send(
        EventDraft::new(
            Actor::agent(child),
            EventKind::TaskCompleted {
                result: TaskResult::summary("Found it."),
            },
        )
        .agent(child)
        .task(task)
        .parent(Some(root)),
    );
    let result = feed
        .t(root)
        .cells
        .iter()
        .find(|c| matches!(c.model, CellModel::ChildResult { .. }))
        .expect("child result in the parent")
        .model
        .clone();
    let CellModel::ChildResult {
        agent,
        name,
        outcome,
        elapsed_ms,
        cost_with_children,
        result,
        ..
    } = result
    else {
        unreachable!()
    };
    assert_eq!((agent, name.as_str()), (child, "researcher"));
    assert_eq!(outcome, ChildOutcome::Finished);
    assert_eq!(elapsed_ms, 63_000);
    assert_eq!(result, "Found it.");
    assert!((cost_with_children.unwrap().usd - 0.21).abs() < 1e-9);
    assert!(
        matches!(
            feed.t(root).active.as_ref().map(|c| &c.model),
            Some(CellModel::Waiting { .. })
        ),
        "the result lands while the wait stays live below it"
    );

    feed.on(
        root,
        EventKind::AgentResumed {
            reason: "task done".into(),
            via: None,
        },
    );
    assert!(feed.t(root).active.is_none());
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Waiting { ended: Some(_), .. })
    ));

    // A crash after completion still reads as a failure, in the same cell.
    feed.on(
        child,
        EventKind::AgentFailed {
            error: "engine exited\nstack".into(),
        },
    );
    let results: Vec<_> = feed
        .models(root)
        .into_iter()
        .filter_map(|m| match m {
            CellModel::ChildResult { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![ChildOutcome::Failed {
            error: "engine exited".into(),
            quota: None
        }]
    );
}

#[test]
fn grandchildren_roll_up_under_the_child_result() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let (child, task) = feed.spawn(Some(root), "implementer", "worker");
    let (grandchild, gtask) = feed.spawn(Some(child), "tester", "verifier");
    feed.usage(grandchild, 0.09, 0.09);
    feed.usage(child, 0.42, 0.42);
    let done = |_task: TaskId, summary: &str| EventKind::TaskCompleted {
        result: TaskResult::summary(summary),
    };
    feed.send(
        EventDraft::new(Actor::System, done(gtask, "ok"))
            .agent(grandchild)
            .task(gtask),
    );
    feed.send(
        EventDraft::new(Actor::System, done(task, "shipped"))
            .agent(child)
            .task(task),
    );
    let Some(CellModel::ChildResult {
        cost_with_children,
        descendants,
        ..
    }) = feed.models(root).last().cloned()
    else {
        panic!("expected the child result last");
    };
    assert!((cost_with_children.unwrap().usd - 0.51).abs() < 1e-9);
    assert_eq!(descendants.len(), 1);
    assert_eq!(descendants[0].name, "tester");
    assert!((descendants[0].cost.unwrap().usd - 0.09).abs() < 1e-9);
}

#[test]
fn turn_separator_sums_the_subtree_spend_and_counts_agents() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    feed.usage(root, 1.00, 1.00);
    let (child, _) = feed.spawn(Some(root), "researcher", "explorer");
    feed.usage(child, 0.25, 0.25);
    feed.usage(root, 1.50, 0.50);
    feed.wait(374);
    feed.on(root, reply("All done."));
    feed.on(
        root,
        EventKind::AgentWaiting {
            condition: WaitCondition::Inbox,
        },
    );
    feed.on(root, turn_done(AgentStatusKind::Waiting));
    let Some(CellModel::Separator {
        elapsed_ms,
        cost,
        agents,
        ..
    }) = feed.models(root).last().cloned()
    else {
        panic!("expected a separator, got {:?}", feed.models(root));
    };
    assert_eq!(*elapsed_ms, 374_000);
    assert!((cost.unwrap().usd - 1.75).abs() < 1e-9, "{cost:?}");
    assert_eq!(*agents, 2);
    assert!(
        feed.t(root).active.is_none(),
        "an inbox wait is idling, not a waiting cell"
    );

    // A mid-turn wait on a child is not the end of the turn.
    feed.on(root, EventKind::AgentStarted { turn: 2 });
    feed.on(root, turn_done(AgentStatusKind::Running));
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Separator { .. })
    ));
    let separators = feed
        .models(root)
        .iter()
        .filter(|m| matches!(m, CellModel::Separator { .. }))
        .count();
    assert_eq!(separators, 1);
}

#[test]
fn steering_preview_retires_into_a_user_cell_on_consume() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.transcripts
        .get_mut(&root)
        .unwrap()
        .steering
        .push(Steer {
            text: "use the other API".into(),
            sent_at: feed.clock,
        });
    let item = feed.operator_message(root, "use the other API");
    assert_eq!(feed.t(root).steering.len(), 1, "delivered is not consumed");
    let before = feed.t(root).cells.len();
    feed.on(
        root,
        EventKind::InboxConsumed {
            up_to: 1,
            items: vec![item],
        },
    );
    let t = feed.t(root);
    assert!(t.steering.is_empty());
    assert_eq!(t.cells.len(), before + 1);
    assert_eq!(
        t.cells.last().unwrap().model,
        CellModel::User {
            from: UserFrom::Operator,
            text: "use the other API".into()
        }
    );
}

#[test]
fn an_operator_reply_dims_the_open_question() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(
        root,
        EventKind::QuestionAsked {
            question: "Which branch?".into(),
        },
    );
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Question {
            state: crate::app::transcript::QuestionState::Pending,
            ..
        })
    ));
    let revision = feed.t(root).cells.last().unwrap().revision;
    let item = feed.operator_message(root, "main");
    feed.on(
        root,
        EventKind::InboxConsumed {
            up_to: 1,
            items: vec![item],
        },
    );
    let question = feed
        .t(root)
        .cells
        .iter()
        .find(|c| matches!(c.model, CellModel::Question { .. }))
        .unwrap();
    assert!(matches!(
        question.model,
        CellModel::Question {
            state: crate::app::transcript::QuestionState::Answered,
            ..
        }
    ));
    assert_ne!(
        question.revision, revision,
        "a changed cell invalidates its layout"
    );
}

#[test]
fn backoff_is_replaced_in_place_by_its_recovery() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    let backoff = |until| EventKind::ProviderBackoff {
        provider: "anthropic".into(),
        reason: "overloaded".into(),
        until,
    };
    let until = feed.clock + Duration::seconds(8);
    feed.send(EventDraft::new(Actor::Scheduler, backoff(until)));
    let until = feed.clock + Duration::seconds(16);
    feed.send(EventDraft::new(Actor::Scheduler, backoff(until)));
    let notices = |feed: &Feed| -> Vec<CellModel> {
        feed.models(root)
            .into_iter()
            .filter(|m| matches!(m, CellModel::Notice { .. }))
            .cloned()
            .collect()
    };
    assert_eq!(
        notices(&feed),
        vec![CellModel::Notice {
            kind: NoticeKind::Backoff {
                incident: "anthropic".into()
            },
            text: NoticeText::joined(&["Anthropic busy", "retrying in 16s"], NoticeSeparator::Dot),
        }],
        "one cell per incident"
    );
    feed.send(EventDraft::new(
        Actor::Scheduler,
        EventKind::ProviderRecovered {
            provider: "anthropic".into(),
        },
    ));
    assert_eq!(
        notices(&feed),
        vec![CellModel::Notice {
            kind: NoticeKind::Recovered {
                incident: "anthropic".into()
            },
            text: "Anthropic recovered".into(),
        }]
    );
}

#[test]
fn quota_failures_raise_an_alert_and_mark_the_child_result() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let (child, task) = feed.spawn(Some(root), "tester", "verifier");
    feed.on(
        child,
        EventKind::InvocationFailed {
            invocation: InvocationId::new(),
            snapshot: aether_core::id::SnapshotId::new(),
            error: "weekly limit reached".into(),
            class: Some(ModelErrorClass::UsageLimit),
            retryable: false,
            cancelled: false,
            partial: Vec::new(),
        },
    );
    assert_eq!(feed.alerts, vec!["anthropic usage limit reached"]);
    assert!(matches!(
        feed.models(child).last(),
        Some(CellModel::Error { .. })
    ));
    feed.send(
        EventDraft::new(
            Actor::System,
            EventKind::TaskFailed {
                error: "weekly limit reached".into(),
                structured: None,
            },
        )
        .agent(child)
        .task(task),
    );
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::ChildResult {
            outcome: ChildOutcome::Failed { quota: Some(_), .. },
            ..
        })
    ));

    feed.send(EventDraft::new(
        Actor::Scheduler,
        EventKind::ProviderQuota {
            provider: "codex".into(),
            window: QuotaWindow {
                used_percent: 100,
                window_minutes: Some(10_080),
            },
            usage: None,
        },
    ));
    assert_eq!(feed.alerts.last().unwrap(), "codex 7d limit reached");
}

#[test]
fn a_retryable_failure_stays_quiet_and_a_cancelled_one_too() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let failed = |retryable, cancelled| EventKind::InvocationFailed {
        invocation: InvocationId::new(),
        snapshot: aether_core::id::SnapshotId::new(),
        error: "overloaded".into(),
        class: None,
        retryable,
        cancelled,
        partial: Vec::new(),
    };
    let before = feed.t(root).cells.len();
    feed.on(root, failed(true, false));
    feed.on(root, failed(false, true));
    assert_eq!(feed.t(root).cells.len(), before);
    feed.on(root, failed(false, false));
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Error { hint: Some(h), .. }) if h.as_str() == "/wake to retry"
    ));
    assert!(feed.alerts.is_empty());
}

#[test]
fn interrupt_keeps_what_streamed_and_names_the_agent() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    feed.transcripts.get_mut(&root).unwrap().stream(
        InvocationId::new(),
        &Delta::Text {
            text: "Half a th".into(),
        },
        feed.clock,
    );
    feed.on(root, EventKind::AgentInterrupted { turn: 1 });
    feed.on(
        root,
        EventKind::AgentWaiting {
            condition: WaitCondition::Inbox,
        },
    );
    feed.on(root, turn_done(AgentStatusKind::Waiting));
    let cells = feed.models(root);
    let n = cells.len();
    assert_eq!(
        cells[n - 2],
        &CellModel::Assistant {
            text: "Half a th".into()
        }
    );
    assert_eq!(
        cells[n - 1],
        &CellModel::Notice {
            kind: NoticeKind::Interrupted,
            text: NoticeText::joined(
                &["Interrupted", "tell aether what to do differently"],
                NoticeSeparator::Dot
            )
        },
        "no separator after an interrupt"
    );
}

#[test]
fn messages_sent_to_several_agents_share_one_cell() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let (a, _) = feed.spawn(Some(root), "tester", "verifier");
    let (b, _) = feed.spawn(Some(root), "docs", "writer");
    for to in [a, b] {
        feed.send(
            EventDraft::new(
                Actor::agent(root),
                EventKind::MessageSent {
                    message: MessageId::new(),
                    from: Actor::agent(root),
                    to,
                    content: vec![ContentBlock::Text {
                        text: "stop and report\nnow".into(),
                    }],
                },
            )
            .agent(to),
        );
    }
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::Sent {
            to: "tester, docs".into(),
            text: "stop and report\nnow".into()
        })
    );
}

#[test]
fn operator_approvals_are_recorded_and_rules_widen_their_scope() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let approval = ApprovalId::new();
    feed.on(
        root,
        EventKind::ApprovalRequested {
            approval,
            call: ToolCallId::new(),
            tool: "bash".into(),
            description: "Run a command".into(),
            argument: Some("git push".into()),
            side_effect: SideEffectClass::External,
        },
    );
    feed.send(
        EventDraft::new(
            Actor::user(),
            EventKind::ApprovalResolved {
                approval,
                decision: ApprovalDecision::Approved,
                by: Actor::user(),
                note: None,
            },
        )
        .agent(root),
    );
    feed.send(
        EventDraft::new(
            Actor::user(),
            EventKind::ApprovalRuleAdded {
                rule: ApprovalRule {
                    id: ApprovalRuleId::new(),
                    decision: ApprovalDecision::Approved,
                    matcher: RuleMatch::tool("bash"),
                    scope: RuleScope::Agent { id: root },
                    created_at: feed.clock,
                    created_by: Actor::user(),
                    note: None,
                    revoked: None,
                },
            },
        )
        .agent(root),
    );
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::ApprovalRecord {
            approval,
            approved: true,
            what: "git push".into(),
            scope: ApprovalScope::Agent {
                name: "aether".into()
            },
        })
    );
}

/// The operator approves `git push` for `agent`; the record it leaves.
fn approve_git_push(feed: &mut Feed, agent: AgentId) -> ApprovalId {
    let approval = ApprovalId::new();
    feed.on(
        agent,
        EventKind::ApprovalRequested {
            approval,
            call: ToolCallId::new(),
            tool: "bash".into(),
            description: "Run a command".into(),
            argument: Some("git push".into()),
            side_effect: SideEffectClass::External,
        },
    );
    feed.send(
        EventDraft::new(
            Actor::user(),
            EventKind::ApprovalResolved {
                approval,
                decision: ApprovalDecision::Approved,
                by: Actor::user(),
                note: None,
            },
        )
        .agent(agent),
    );
    approval
}

/// A remembered rule as the daemon writes it: no agent on the event.
fn rule_added(feed: &mut Feed, scope: RuleScope) {
    feed.send(EventDraft::new(
        Actor::user(),
        EventKind::ApprovalRuleAdded {
            rule: ApprovalRule {
                id: ApprovalRuleId::new(),
                decision: ApprovalDecision::Approved,
                matcher: RuleMatch::tool("bash"),
                scope,
                created_at: feed.clock,
                created_by: Actor::user(),
                note: None,
                revoked: None,
            },
        },
    ));
}

fn record_scope(feed: &Feed, agent: AgentId) -> Option<ApprovalScope> {
    feed.models(agent).iter().rev().find_map(|m| match m {
        CellModel::ApprovalRecord { scope, .. } => Some(scope.clone()),
        _ => None,
    })
}

#[test]
fn a_rule_from_the_same_answer_labels_the_record_though_it_names_no_agent() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let (worker, _) = feed.spawn(Some(root), "worker", "coder");

    approve_git_push(&mut feed, worker);
    rule_added(&mut feed, RuleScope::Subtree { root: worker });
    assert_eq!(
        record_scope(&feed, worker),
        Some(ApprovalScope::Subtree {
            root: "worker".into()
        })
    );

    approve_git_push(&mut feed, worker);
    rule_added(&mut feed, RuleScope::Everywhere);
    assert_eq!(record_scope(&feed, worker), Some(ApprovalScope::Everywhere));
}

#[test]
fn a_rule_added_on_its_own_leaves_the_last_record_alone() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    approve_git_push(&mut feed, root);
    feed.on(root, reply("pushed"));
    rule_added(&mut feed, RuleScope::Everywhere);
    assert_eq!(record_scope(&feed, root), Some(ApprovalScope::Once));
}

/// A web fetch held for approval, answered `decision` by `by`.
fn gated_fetch(by: Actor, decision: ApprovalDecision) -> (Feed, AgentId, ToolCallId) {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let url = "https://example.com/changelog";
    let call = feed.tool(root, "web_fetch", json!({ "url": url }));
    let approval = ApprovalId::new();
    feed.on(
        root,
        EventKind::ApprovalRequested {
            approval,
            call,
            tool: "web_fetch".into(),
            description: "Fetch a page".into(),
            argument: Some(url.into()),
            side_effect: SideEffectClass::External,
        },
    );
    let status = |feed: &Feed| match feed.models(root).last() {
        Some(CellModel::Called { status, .. }) => Some(*status),
        _ => None,
    };
    assert_eq!(status(&feed), Some(CallStatus::Awaiting));
    feed.send(
        EventDraft::new(
            by.clone(),
            EventKind::ApprovalResolved {
                approval,
                decision,
                by,
                note: None,
            },
        )
        .agent(root),
    );
    (feed, root, call)
}

fn shapes(feed: &Feed, agent: AgentId) -> Vec<&'static str> {
    feed.models(agent)
        .iter()
        .filter_map(|m| match m {
            CellModel::ApprovalRecord { approved: true, .. } => Some("approved"),
            CellModel::ApprovalRecord {
                approved: false, ..
            } => Some("denied"),
            CellModel::Called { .. } => Some("call"),
            _ => None,
        })
        .collect()
}

#[test]
fn an_approval_reads_before_the_call_it_let_through() {
    let (mut feed, root, call) = gated_fetch(Actor::user(), ApprovalDecision::Approved);
    assert_eq!(shapes(&feed, root), ["approved", "call"]);
    let Some(CellModel::Called { status, .. }) = feed.models(root).last().copied() else {
        panic!("the call cell last: {:?}", feed.models(root));
    };
    assert_eq!(*status, CallStatus::Running);
    feed.tool_ok(root, call, ToolOutcome::ok("changelog"));
    assert_eq!(shapes(&feed, root), ["approved", "call"]);
}

#[test]
fn a_denied_call_leaves_only_its_record() {
    let (mut feed, root, call) = gated_fetch(Actor::user(), ApprovalDecision::Denied);
    feed.on(
        root,
        EventKind::ToolDenied {
            call,
            reason: "denied by approver".into(),
        },
    );
    assert_eq!(shapes(&feed, root), ["denied"]);
}

#[test]
fn a_rule_lets_a_call_through_without_a_record() {
    let (feed, root, _) = gated_fetch(Actor::System, ApprovalDecision::Approved);
    assert_eq!(shapes(&feed, root), ["call"]);
    let Some(CellModel::Called { status, .. }) = feed.models(root).last().copied() else {
        panic!("the call cell: {:?}", feed.models(root));
    };
    assert_eq!(*status, CallStatus::Running);
}

#[test]
fn replayed_events_are_ingested_once_and_archive_drops_the_transcript() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let event = Event::from_draft(
        EventDraft::new(Actor::System, reply("once")).agent(root),
        EventId::new(),
        feed.seq + 1,
        feed.clock,
    );
    feed.feed(&event);
    feed.feed(&event);
    let replies = feed
        .models(root)
        .iter()
        .filter(|m| matches!(m, CellModel::Assistant { .. }))
        .count();
    assert_eq!(replies, 1);

    feed.seq += 1;
    feed.on(root, EventKind::AgentArchived { archived: true });
    assert!(!feed.transcripts.contains_key(&root));
}

#[test]
fn compaction_reports_the_tokens_its_record_carries() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let completed = |before_tokens, after_tokens| EventKind::CompactionCompleted {
        checkpoint: aether_core::id::CheckpointId::new(),
        version: 1,
        from_seq: 0,
        to_seq: 9,
        before_tokens,
        after_tokens,
    };
    // An earlier turn's usage is no measure of the fold.
    feed.on(
        root,
        EventKind::InvocationCompleted {
            invocation: InvocationId::new(),
            snapshot: aether_core::id::SnapshotId::new(),
            model: ModelRef::new("mock", "mock-1"),
            usage: Usage {
                input_tokens: 971,
                ..Usage::default()
            },
            stop_reason: None,
            duration_ms: 1,
        },
    );
    feed.on(
        root,
        EventKind::CompactionRequested {
            trigger: aether_core::event::CompactionTrigger::LongWait,
        },
    );
    assert!(matches!(
        feed.models(root).last(),
        Some(CellModel::Compaction(CompactionState::Active))
    ));
    feed.on(root, completed(Some(182_000), Some(31_000)));
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::Compaction(CompactionState::Done {
            before: Some(182_000),
            after: Some(31_000)
        }))
    );
    // An older log's record has no counts, and none are guessed.
    feed.on(root, completed(None, None));
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::Compaction(CompactionState::Done {
            before: None,
            after: None
        }))
    );
}

#[test]
fn triggers_and_schedule_deliveries_show_where_they_landed() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let trigger = |feed: &mut Feed, source: &str, payload: Value| {
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: root,
            seq: feed.seq + 1,
            kind: InboxKind::ExternalTrigger {
                source: source.into(),
                payload,
                operator: false,
            },
            delivered_at: feed.clock,
        };
        feed.send(
            EventDraft::new(Actor::Scheduler, EventKind::InboxDelivered { item }).agent(root),
        );
        feed.models(root).last().cloned().cloned()
    };
    assert_eq!(
        trigger(&mut feed, "ci", json!({"build": "green"})),
        Some(CellModel::Notice {
            kind: NoticeKind::Trigger { schedule: None },
            text: NoticeText::joined(
                &["Trigger from ci", r#"{"build":"green"}"#],
                NoticeSeparator::Dot
            )
        })
    );
    assert_eq!(
        trigger(&mut feed, "schedule:sch_1", json!(" tick hello\n")),
        Some(CellModel::Notice {
            kind: NoticeKind::Trigger { schedule: None },
            text: NoticeText::joined(
                &["Trigger from schedule:sch_1", "tick hello"],
                NoticeSeparator::Dot
            )
        })
    );
    assert_eq!(
        trigger(&mut feed, "ci", Value::Null),
        Some(CellModel::Notice {
            kind: NoticeKind::Trigger { schedule: None },
            text: "Trigger from ci".into()
        })
    );
}

#[test]
fn a_schedule_delivery_takes_its_schedules_name_safely_or_keeps_the_id() {
    let id = ScheduleId::new();
    let source = format!("schedule:{id}");
    assert_eq!(schedule_of(&source), Some(id));
    assert_eq!(schedule_of("schedule:sch_1"), None);
    assert_eq!(schedule_of("ci"), None);

    let cell = CellModel::Notice {
        kind: NoticeKind::Trigger { schedule: Some(id) },
        text: NoticeText::joined(
            &[&format!("Trigger from {source}"), "hello tick"],
            NoticeSeparator::Dot,
        ),
    };
    let named = |name: &str, detail| {
        cell.with_schedule_name(|_| Some(name.to_owned()), detail)
            .map(|cell| match cell {
                CellModel::Notice { text, .. } => text.ascii("-").into_owned(),
                other => panic!("{other:?}"),
            })
    };
    assert_eq!(
        named("bf4-tick", false).as_deref(),
        Some("Trigger from schedule bf4-tick - hello tick")
    );
    assert_eq!(
        named("bf4-tick", true),
        Some(format!(
            "Trigger from schedule bf4-tick ({id}) - hello tick"
        ))
    );
    let hostile = named("\u{1b}[31mbf4\n\u{202e}tick", false).unwrap();
    assert!(hostile.starts_with("Trigger from schedule "));
    assert!(!hostile.chars().any(|c| c.is_control() || c == '\u{202e}'));
    assert!(named(&"x".repeat(10_000), false).unwrap().len() < 400);
    assert_eq!(named(" \n", false), None);
    assert_eq!(cell.with_schedule_name(|_| None, false), None);
}

#[test]
fn trigger_summaries_show_one_line_and_keep_json_in_details() {
    use crate::ui::cells::ChatCell;

    let theme = crate::ui::theme::tokens::Theme::resolve(Default::default());
    for (summary, expected) in [
        ("Build passed", "Trigger from ci · Build passed"),
        (" Build\n\tpassed\u{7} ", "Trigger from ci · Build passed"),
        ("\u{1b}[31m\u{7}", "Trigger from ci"),
    ] {
        let payload = json!({"summary": summary, "build_id": 42});
        let mut feed = Feed::new();
        let (root, _) = feed.spawn(None, "aether", "assistant");
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: root,
            seq: feed.seq + 1,
            kind: InboxKind::ExternalTrigger {
                source: "ci".into(),
                payload: payload.clone(),
                operator: false,
            },
            delivered_at: feed.clock,
        };
        feed.on(root, EventKind::InboxDelivered { item });
        let cell = feed.models(root).last().cloned().unwrap();
        let CellModel::Notice { text, .. } = cell else {
            panic!("trigger notice");
        };
        assert_eq!(text.as_str(), expected);
        assert_eq!(text.detail_payload(), Some(payload.to_string().as_str()));
        assert_eq!(text.ascii("-"), expected.replace('·', "-"));
        let display = cell.display_lines(160, &theme);
        assert_eq!(display.len(), 1);
        assert!(display[0].to_string().contains(expected));
        assert!(!display[0].to_string().contains("build_id"));
        let compact = crate::ui::cells::compact::line(cell, 160, &theme).unwrap();
        assert!(compact.to_string().contains(expected));
        assert!(!compact.to_string().contains("build_id"));
        let details = cell.detail_lines(160, &theme);
        assert!(
            details
                .iter()
                .any(|line| line.to_string().contains(&payload.to_string()))
        );

        // Naming a schedule must preserve its payload details.
        let id = ScheduleId::new();
        let scheduled = CellModel::Notice {
            kind: NoticeKind::Trigger { schedule: Some(id) },
            text: text.clone(),
        };
        let named = scheduled
            .with_schedule_name(|_| Some("builds".into()), true)
            .unwrap();
        assert!(
            named
                .detail_lines(160, &theme)
                .iter()
                .any(|line| { line.to_string().contains(&payload.to_string()) })
        );
    }
}

#[test]
fn unusable_trigger_summaries_keep_the_object_preview() {
    for payload in [
        json!({"summary": ""}),
        json!({"summary": " \n\t "}),
        json!({"summary": null}),
        json!({"summary": 42}),
        json!({"nested": {"summary": "Build passed"}}),
    ] {
        let mut feed = Feed::new();
        let (root, _) = feed.spawn(None, "aether", "assistant");
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: root,
            seq: feed.seq + 1,
            kind: InboxKind::ExternalTrigger {
                source: "ci".into(),
                payload: payload.clone(),
                operator: false,
            },
            delivered_at: feed.clock,
        };
        feed.on(root, EventKind::InboxDelivered { item });
        assert_eq!(
            feed.models(root).last().cloned(),
            Some(&CellModel::Notice {
                kind: NoticeKind::Trigger { schedule: None },
                text: NoticeText::joined(
                    &["Trigger from ci", &payload.to_string()],
                    NoticeSeparator::Dot,
                ),
            })
        );
    }
}

#[test]
fn trigger_previews_are_safe_bounded_and_identical_on_replay() {
    let hostile = format!("\u{1b}[32mgreen\u{1b}[0m\u{7} {}", "é".repeat(10_000));
    for payload in [
        json!(hostile),
        json!({"summary": format!("green\n\t\u{202e}{hostile}"), "build_id": 42}),
    ] {
        let root = AgentId::new();
        let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: root,
            seq: 1,
            kind: InboxKind::ExternalTrigger {
                source: "ci\u{1b}[31m\u{0}\u{202e}".into(),
                payload,
                operator: false,
            },
            delivered_at: at,
        };
        let delivered = Event::from_draft(
            EventDraft::new(
                Actor::Scheduler,
                EventKind::InboxDelivered { item: item.clone() },
            )
            .agent(root),
            EventId::new(),
            1,
            at,
        );
        let consumed = Event::from_draft(
            EventDraft::new(
                Actor::Scheduler,
                EventKind::InboxConsumed {
                    items: vec![item.id],
                    up_to: 1,
                },
            )
            .agent(root),
            EventId::new(),
            2,
            at,
        );
        let tree = AgentTree::default();
        let ctx = IngestCtx {
            tree: &tree,
            now: at,
        };
        let mut live = Transcripts::new();
        let mut replay = Transcripts::new();
        for event in [&delivered, &delivered, &consumed] {
            ingest_event(&mut live, event, &ctx);
            ingest_replay_event(&mut replay, event, &ctx);
        }
        assert_eq!(live[&root].cells.len(), 1);
        assert_eq!(live[&root].cells, replay[&root].cells);
        let CellModel::Notice {
            kind: NoticeKind::Trigger { .. },
            text,
        } = &live[&root].cells[0].model
        else {
            panic!("trigger notice");
        };
        assert!(text.starts_with("Trigger from ci · green"));
        assert!(text.len() <= 4_500, "preview is {} bytes", text.len());
        assert!(text.ends_with("[truncated]"));
        assert!(!text.chars().any(|c| c.is_control() || c == '\u{202e}'));
        if text.detail_payload().is_some() {
            assert!(text.starts_with("Trigger from ci · green green é"));
        }
    }
}

#[test]
fn trigger_sources_and_payloads_are_bounded_even_in_details() {
    use crate::ui::cells::ChatCell;

    for payload in [
        json!("é".repeat(10_000)),
        json!({"text": "é".repeat(10_000)}),
        json!({"summary": "é".repeat(10_000), "text": "é".repeat(10_000)}),
    ] {
        let mut feed = Feed::new();
        let (root, _) = feed.spawn(None, "aether", "assistant");
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: root,
            seq: feed.seq + 1,
            kind: InboxKind::ExternalTrigger {
                source: "schedule:".repeat(1_000),
                payload,
                operator: false,
            },
            delivered_at: feed.clock,
        };
        feed.on(root, EventKind::InboxDelivered { item });
        let cell = feed.t(root).cells.last().unwrap().model.clone();
        let CellModel::Notice { text, .. } = &cell else {
            panic!("trigger");
        };
        assert!(text.len() <= 4_500, "preview is {} bytes", text.len());
        assert!(text.ends_with("[truncated]"));
        for width in [59, 60, 99, 100, 160] {
            let theme = crate::ui::theme::tokens::Theme::resolve(Default::default());
            assert_eq!(cell.display_lines(width, &theme).len(), 1);
            let max_rows = if text.detail_payload().is_some() {
                240
            } else {
                120
            };
            assert!(cell.detail_lines(width, &theme).len() < max_rows);
        }
    }
}

#[test]
fn model_change_is_noticed_only_when_something_changed() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let before = feed.t(root).cells.len();
    let update = |model: &str| EventKind::AgentConfigUpdated {
        name: None,
        model: Some(ModelRef::new("anthropic", model)),
        config: aether_core::agent::AgentConfig::default(),
        scope: None,
    };
    feed.on(root, update("claude-sonnet-5-5"));
    assert_eq!(feed.t(root).cells.len(), before);
    feed.on(root, update("claude-opus-5-5"));
    assert_eq!(
        feed.models(root).last().cloned(),
        Some(&CellModel::Notice {
            kind: NoticeKind::ModelChange,
            text: NoticeText::joined(
                &["Model", "opus-5.5 from the next turn"],
                NoticeSeparator::Arrow
            )
        })
    );
}

#[test]
fn same_named_children_are_numbered_and_finish_under_that_name() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    feed.spawn(Some(root), "worker", "worker");
    let (second, task) = feed.spawn(Some(root), "worker", "worker");
    // A late child's transcript starts after its parent's spawn cell.
    feed.transcripts.remove(&second);
    feed.send(
        EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
            .agent(second)
            .parent(Some(root)),
    );
    feed.on(
        root,
        EventKind::AgentWaiting {
            condition: WaitCondition::MessageFrom { agent: second },
        },
    );
    let waiting = feed.models(root).into_iter().find_map(|m| match m {
        CellModel::Waiting { on, .. } => on.first().map(|(_, name)| name.clone()),
        _ => None,
    });
    assert_eq!(waiting.as_deref(), Some("worker 2"));
    feed.send(
        EventDraft::new(
            Actor::System,
            EventKind::TaskCompleted {
                result: TaskResult::summary("listed"),
            },
        )
        .agent(second)
        .task(task),
    );
    let names: Vec<&str> = feed
        .models(root)
        .into_iter()
        .filter_map(|m| match m {
            CellModel::Spawned { name, .. } | CellModel::ChildResult { name, .. } => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(names, ["worker", "worker 2", "worker 2"]);
}

#[test]
fn an_approval_described_as_json_is_recorded_by_its_tool() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let approval = ApprovalId::new();
    feed.on(
        root,
        EventKind::ApprovalRequested {
            approval,
            call: ToolCallId::new(),
            tool: "linear.create_issue".into(),
            description: "{\"title\":\"flaky wal test\",\"team\":4}".into(),
            argument: None,
            side_effect: SideEffectClass::External,
        },
    );
    feed.send(
        EventDraft::new(
            Actor::user(),
            EventKind::ApprovalResolved {
                approval,
                decision: ApprovalDecision::Approved,
                by: Actor::user(),
                note: None,
            },
        )
        .agent(root),
    );
    let what = feed.models(root).iter().find_map(|m| match m {
        CellModel::ApprovalRecord { what, .. } => Some(what.clone()),
        _ => None,
    });
    assert_eq!(what.as_deref(), Some("linear.create_issue"));
}

#[test]
fn remembered_approval_records_keep_run_and_everywhere_scope() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let run = aether_core::id::RunId::new();
    approve_git_push(&mut feed, root);
    rule_added(&mut feed, RuleScope::Run { id: run });
    assert_eq!(
        record_scope(&feed, root),
        Some(ApprovalScope::Run { id: run })
    );
    let record = feed
        .t(root)
        .cells
        .iter()
        .find(|c| matches!(c.model, CellModel::ApprovalRecord { .. }))
        .unwrap();
    let output = crate::plain::describe(record).join("\n");
    assert!(output.contains(&format!("saved for run {run}")), "{output}");
    assert!(!output.contains("this session"));
    approve_git_push(&mut feed, root);
    rule_added(&mut feed, RuleScope::Everywhere);
    assert_eq!(record_scope(&feed, root), Some(ApprovalScope::Everywhere));
    let output = crate::plain::describe(feed.t(root).cells.last().unwrap()).join("\n");
    assert!(output.contains("always, everywhere"), "{output}");
}

#[test]
fn pending_delivery_promotes_one_stable_cell_without_changing_standard_timing() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "coordinator", "supervisor");
    let sender = AgentId::new();
    let item = InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        delivered_at: feed.clock,
        seq: 1,
        kind: InboxKind::Message {
            from: Actor::agent(sender),
            content: vec![ContentBlock::text("first line\nfull message")],
            message_id: MessageId::new(),
        },
    };
    feed.on(agent, EventKind::InboxDelivered { item: item.clone() });
    let t = &feed.transcripts[&agent];
    let pending = t.pending_messages.values().next().unwrap().id;
    assert!(!t.presentation_cells(false).any(|c| c.id == pending));
    assert!(t.presentation_cells(true).any(|c| c.id == pending));
    feed.on(
        agent,
        EventKind::InboxConsumed {
            items: vec![item.id],
            up_to: 1,
        },
    );
    let t = &feed.transcripts[&agent];
    assert!(t.pending_messages.is_empty());
    assert_eq!(t.cells.iter().filter(|c| c.id == pending).count(), 1);
    assert!(
        matches!(&t.cell(pending).unwrap().model, CellModel::User { text, .. } if text == "first line\nfull message")
    );
    feed.on(agent, EventKind::InboxDelivered { item });
    assert!(feed.transcripts[&agent].pending_messages.is_empty());
}

#[test]
fn own_completion_deduplicates_task_and_agent_outcomes_and_failure_wins() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "coordinator", "supervisor");
    let call = feed.tool(agent, "read_file", json!({"path": "notes.txt"}));
    let active = feed.transcripts[&agent].active.as_ref().unwrap().id;
    feed.on(
        agent,
        EventKind::TaskCompleted {
            result: TaskResult::summary("Complete\nfull result"),
        },
    );
    assert_eq!(feed.transcripts[&agent].active.as_ref().unwrap().id, active);
    feed.tool_ok(agent, call, ToolOutcome::ok("notes"));
    feed.on(
        agent,
        EventKind::AgentCompleted {
            summary: Some("Complete".into()),
        },
    );
    let t = &feed.transcripts[&agent];
    assert_eq!(t.named_only.len(), 1);
    let id = *t.named_only.iter().next().unwrap();
    assert!(!t.presentation_cells(false).any(|c| c.id == id));
    feed.on(
        agent,
        EventKind::AgentFailed {
            error: "late failure".into(),
        },
    );
    assert!(
        matches!(&feed.transcripts[&agent].cell(id).unwrap().model, CellModel::ChildResult { outcome: ChildOutcome::Failed { error, .. }, result, .. } if error == "late failure" && result == "Complete\nfull result")
    );
}

#[test]
fn outgoing_messages_compare_full_bodies_instead_of_first_lines() {
    let mut feed = Feed::new();
    let (sender, _) = feed.spawn(None, "sender", "supervisor");
    let to = AgentId::new();
    for text in ["same first\nbody one", "same first\nbody two"] {
        feed.on(
            to,
            EventKind::MessageSent {
                message: MessageId::new(),
                from: Actor::agent(sender),
                to,
                content: vec![ContentBlock::text(text)],
            },
        );
    }
    let sent: Vec<_> = feed.transcripts[&sender]
        .cells
        .iter()
        .filter_map(|c| match &c.model {
            CellModel::Sent { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, ["same first\nbody one", "same first\nbody two"]);
}

#[test]
fn grouped_tool_failures_keep_full_named_mode_notices_and_stock_cells() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "coordinator", "supervisor");
    let call = feed.tool(agent, "read_file", json!({"path": "missing.txt"}));
    let before = feed.models(agent).len();
    feed.on(
        agent,
        EventKind::ToolFailed {
            call,
            error: "first line\nfull failure detail".into(),
            retryable: false,
            duration_ms: None,
        },
    );
    let transcript = &feed.transcripts[&agent];
    assert_eq!(transcript.presentation_cells(false).count(), before);
    assert!(transcript.presentation_cells(true).any(|cell| matches!(&cell.model, CellModel::Called { status: CallStatus::Failed, error: Some(error), .. } if error.contains("full failure detail"))));
}

#[test]
fn bounded_trimming_preserves_group_call_and_artifact_indexes() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "window", "assistant");
    let mut original = Vec::new();
    for n in 0..3_000 {
        let call = feed.tool(agent, "shell", json!({"command": format!("echo {n}")}));
        original.push(feed.events.last().unwrap().clone());
        let mut outcome = ToolOutcome::ok("done");
        outcome.artifacts.push(ArtifactId::new());
        feed.tool_ok(agent, call, outcome);
        original.push(feed.events.last().unwrap().clone());
    }
    let current = feed.t(agent);
    assert!(current.history.len() <= HISTORY_WINDOW);
    assert!(current.cells.len() < 3_000);
    current.assert_window_indexes();
    let mut restored = Transcripts::new();
    let ctx = IngestCtx {
        tree: &feed.tree,
        now: feed.clock,
    };
    for event in original.iter().take(2_500) {
        ingest_replay_event(&mut restored, event, &ctx);
    }
    restored[&agent].assert_window_indexes();
    assert_eq!(restored[&agent].cells.len(), 1_250);
    let call = feed.tool(agent, "read_file", json!({"path":"one.rs"}));
    feed.tool_ok(agent, call, ToolOutcome::ok("one"));
    let call = feed.tool(agent, "read_file", json!({"path":"two.rs"}));
    feed.tool_ok(agent, call, ToolOutcome::ok("two"));
    feed.t(agent).assert_window_indexes();
    assert!(matches!(&feed.t(agent).active.as_ref().unwrap().model,
        CellModel::Explored { items, .. } if !items.is_empty()));
}

#[test]
fn local_error_notices_are_bounded_without_persisted_events() {
    let mut transcript = AgentTranscript::default();
    for n in 0..100 {
        transcript.client_error(now(), format!("error {n}"));
    }
    assert_eq!(transcript.cells.len(), 64);
    assert_eq!(transcript.history.len(), 0);
    transcript.assert_window_indexes();
    assert!(matches!(&transcript.cells.last().unwrap().model,
        CellModel::Notice { text, .. } if text.to_string().contains("error 99")));
}

#[test]
fn window_eviction_keeps_the_open_turns_elapsed_time_and_accumulated_spend() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "window", "assistant");
    feed.on(root, EventKind::AgentStarted { turn: 1 });
    feed.usage(root, 1.0, 1.0);
    for _ in 0..HISTORY_WINDOW {
        feed.on(root, reply("long running turn"));
    }
    feed.usage(root, 1.5, 0.5);
    feed.wait(42);
    feed.on(root, turn_done(AgentStatusKind::Completed));
    let Some(CellModel::Separator {
        elapsed_ms,
        cost,
        agents,
        ..
    }) = feed.models(root).last().copied()
    else {
        panic!("the window must retain the open turn's accounting");
    };
    assert_eq!(*elapsed_ms, 42_000);
    assert_eq!(cost.unwrap().usd, 1.5);
    assert_eq!(*agents, 1);
    feed.t(root).assert_window_indexes();
}

#[test]
fn window_eviction_preserves_pending_questions_until_answered() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "window", "assistant");
    feed.on(
        agent,
        EventKind::QuestionAsked {
            question: "Which branch?".into(),
        },
    );
    let question = feed.t(agent).cells.last().unwrap().id;
    for _ in 0..HISTORY_WINDOW + 1 {
        feed.on(agent, reply("activity"));
    }
    assert!(matches!(
        feed.t(agent).cell(question).unwrap().model,
        CellModel::Question {
            state: QuestionState::Pending,
            ..
        }
    ));
    let item = feed.operator_message(agent, "main");
    feed.on(
        agent,
        EventKind::InboxConsumed {
            up_to: feed.seq,
            items: vec![item],
        },
    );
    assert!(matches!(
        feed.t(agent).cell(question).unwrap().model,
        CellModel::Question {
            state: QuestionState::Answered,
            ..
        }
    ));
    feed.t(agent).assert_window_indexes();
}

#[test]
fn window_eviction_preserves_open_calls_until_settled() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "window", "assistant");
    let call = feed.tool(agent, "shell", json!({"command": "long-running"}));
    let cell = feed.t(agent).all_cells().last().unwrap().id;
    for _ in 0..HISTORY_WINDOW + 1 {
        feed.on(agent, reply("activity"));
    }
    assert!(matches!(
        feed.t(agent).cell(cell).unwrap().model,
        CellModel::Exec {
            status: CallStatus::Running,
            ..
        }
    ));
    let artifact = ArtifactId::new();
    let mut outcome = ToolOutcome::ok("done");
    outcome.artifacts.push(artifact);
    feed.tool_ok(agent, call, outcome);
    assert!(matches!(&feed.t(agent).cell(cell).unwrap().model,
        CellModel::Exec { status: CallStatus::Succeeded, artifacts, .. }
        if artifacts[0].id == artifact));
    for _ in 0..HISTORY_WINDOW + 1 {
        feed.on(agent, reply("later activity"));
    }
    assert!(feed.t(agent).cell(cell).is_none());
    feed.t(agent).assert_window_indexes();
}

#[test]
fn window_eviction_preserves_unconsumed_inbox_items() {
    let mut feed = Feed::new();
    let (agent, _) = feed.spawn(None, "window", "assistant");
    let item = feed.operator_message(agent, "pending instruction");
    for _ in 0..HISTORY_WINDOW + 1 {
        feed.on(agent, reply("activity"));
    }
    feed.on(
        agent,
        EventKind::InboxConsumed {
            up_to: feed.seq,
            items: vec![item],
        },
    );
    assert!(feed.models(agent).iter().any(|model| matches!(model,
        CellModel::User { text, .. } if text == "pending instruction")));
    feed.t(agent).assert_window_indexes();
}

#[test]
fn window_eviction_preserves_an_open_wait() {
    let mut feed = Feed::new();
    let (parent, _) = feed.spawn(None, "parent", "assistant");
    let (child, task) = feed.spawn(Some(parent), "child", "assistant");
    feed.on(
        parent,
        EventKind::AgentWaiting {
            condition: WaitCondition::Task { id: task },
        },
    );
    let cell = feed.t(parent).active.as_ref().unwrap().id;
    for _ in 0..HISTORY_WINDOW + 1 {
        feed.send(
            EventDraft::new(Actor::System, reply("child activity"))
                .agent(child)
                .parent(Some(parent)),
        );
    }
    assert!(matches!(
        feed.t(parent).cell(cell).unwrap().model,
        CellModel::Waiting { ended: None, .. }
    ));
    feed.on(parent, EventKind::AgentStarted { turn: 2 });
    assert!(matches!(
        feed.t(parent).cell(cell).unwrap().model,
        CellModel::Waiting { ended: Some(_), .. }
    ));
    feed.t(parent).assert_window_indexes();
}

#[test]
fn terminal_agents_release_orphaned_calls_and_artifact_records() {
    use aether_core::artifact::{Artifact, ArtifactKind};

    for terminal in [
        EventKind::AgentCompleted { summary: None },
        EventKind::AgentFailed {
            error: "failed".into(),
        },
        EventKind::AgentCancelled { reason: None },
    ] {
        let mut feed = Feed::new();
        let (agent, _) = feed.spawn(None, "window", "assistant");
        feed.tool(agent, "shell", json!({"command": "first orphan"}));
        feed.tool(agent, "shell", json!({"command": "second orphan"}));
        let calls: Vec<_> = feed
            .t(agent)
            .all_cells()
            .filter_map(|cell| matches!(cell.model, CellModel::Exec { .. }).then_some(cell.id))
            .collect();
        assert_eq!(calls.len(), 2);
        for _ in 0..HISTORY_WINDOW + 1 {
            feed.on(
                agent,
                EventKind::ArtifactCreated {
                    artifact: Artifact {
                        id: ArtifactId::new(),
                        kind: ArtifactKind::CommandOutput,
                        name: "unlinked output".into(),
                        path: None,
                        summary: String::new(),
                        size_bytes: 1,
                        content_type: None,
                        digest: None,
                        created_by: Actor::agent(agent),
                        agent_id: Some(agent),
                        task_id: None,
                        created_at: feed.clock,
                        evicted_at: None,
                    },
                },
            );
        }
        feed.on(agent, terminal);
        feed.t(agent).assert_calls_closed();
        for id in &calls {
            assert!(matches!(
                feed.t(agent).cell(*id).unwrap().model,
                CellModel::Exec {
                    status: CallStatus::Cancelled,
                    ..
                }
            ));
        }
        for _ in 0..HISTORY_WINDOW + 1 {
            feed.on(agent, reply("later activity"));
        }
        for id in calls {
            assert!(feed.t(agent).cell(id).is_none());
        }
        feed.t(agent).assert_window_indexes();
    }
}

#[path = "cancel_tests.rs"]
mod cancellation;
