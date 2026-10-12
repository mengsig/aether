use aether_core::agent::{Agent, AgentStatus, BlockReason, ExhaustionClass};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ApprovalId, TaskId, ToolCallId};
use aether_core::model::ContentBlock;
use aether_core::model::ModelRef;
use aether_core::time::now;
use aether_core::tool::{ApprovalRequest, ApprovalStatus, SideEffectClass};
use aether_protocol::remote::{Attention, AttentionKind};
use aether_protocol::{Command, Reply};
use aether_runtime::CreateAgentSpec;
use aether_store::{Commit, Mutation};
use chrono::Duration;

use super::*;
use crate::connection::host_services::dispatch;
use crate::connection::host_services::test_support::daemon;

fn create(daemon: &Daemon, name: &str) -> Agent {
    let spec = CreateAgentSpec {
        role: "worker".into(),
        name: Some(name.into()),
        ..Default::default()
    };
    daemon
        .runtime
        .create_agent(spec, Actor::user())
        .expect("agent")
        .0
}

fn amend(daemon: &Daemon, id: AgentId, change: impl FnOnce(&mut Agent)) {
    let mut agent = daemon.runtime.store().agent(id).unwrap().unwrap();
    change(&mut agent);
    daemon
        .runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .expect("amend");
}

fn ask(daemon: &Daemon, agent: AgentId, question: &str) -> u64 {
    daemon
        .runtime
        .apply(
            Commit::new().event(
                EventDraft::new(
                    Actor::agent(agent),
                    EventKind::QuestionAsked {
                        question: question.into(),
                    },
                )
                .agent(agent),
            ),
        )
        .expect("ask")[0]
        .seq
}

fn approval(agent: AgentId, tool: &str, at: aether_core::time::Timestamp) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: agent,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: tool.into(),
        description: format!("run {tool}"),
        argument: Some("rm -rf build".into()),
        resources: Vec::new(),
        side_effect: SideEffectClass::Mutation,
        requested_at: at,
        status: ApprovalStatus::Pending,
    }
}

fn put_approval(daemon: &Daemon, request: ApprovalRequest) {
    daemon
        .runtime
        .apply(Commit::new().mutate(Mutation::PutApproval(request)))
        .expect("approval");
}

async fn attention(daemon: &Daemon) -> Attention {
    match dispatch(daemon, Command::GetAttention)
        .await
        .expect("attention")
    {
        Reply::Attention(attention) => attention,
        other => panic!("wrong reply: {other:?}"),
    }
}

#[tokio::test]
async fn a_quiet_daemon_has_nothing_waiting_and_names_its_position() {
    let (daemon, _config) = daemon().await;
    create(&daemon, "idle");

    let found = attention(&daemon).await;

    assert!(found.approvals.is_empty() && found.questions.is_empty() && found.agents.is_empty());
    assert_eq!(found.last_seq, daemon.runtime.store().last_seq().unwrap());
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_pending_approval_says_what_is_being_approved() {
    let (daemon, _config) = daemon().await;
    let agent = create(&daemon, "builder");
    let request = approval(agent.id, "shell", now());
    put_approval(&daemon, request.clone());
    let resolved = ApprovalRequest {
        status: ApprovalStatus::Resolved {
            decision: aether_core::tool::ApprovalDecision::Denied,
            by: Actor::user(),
            at: now(),
            note: None,
        },
        ..approval(agent.id, "done", now())
    };
    put_approval(&daemon, resolved);

    let found = attention(&daemon).await;

    assert_eq!(found.approvals, std::slice::from_ref(&request));
    assert_eq!(found.approvals[0].argument.as_deref(), Some("rm -rf build"));
    assert_eq!(found.last_seq, daemon.runtime.store().last_seq().unwrap());
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_question_is_pending_until_the_operator_answers_or_the_agent_asks_again() {
    let (daemon, _config) = daemon().await;
    let agent = create(&daemon, "asker");
    let first = ask(&daemon, agent.id, "Which branch?");

    let open = attention(&daemon).await;
    assert_eq!(open.questions.len(), 1);
    let question = &open.questions[0];
    assert_eq!(
        (
            question.agent,
            question.question_seq,
            question.question.as_str()
        ),
        (agent.id, first, "Which branch?")
    );
    assert_eq!(question.agent_name.as_deref(), Some("asker"));
    assert_eq!(
        open.last_seq, first,
        "the read is at the question's position"
    );

    let second = ask(&daemon, agent.id, "Or this one?");
    let superseded = attention(&daemon).await;
    assert_eq!(superseded.questions.len(), 1);
    assert_eq!(superseded.questions[0].question_seq, second);

    daemon
        .runtime
        .reply_to_question(
            agent.id,
            second,
            vec![ContentBlock::text("main")],
            Actor::user(),
            None,
        )
        .expect("reply");
    let answered = attention(&daemon).await;
    assert!(answered.questions.is_empty());
    assert!(answered.last_seq > second, "the reply is in the position");
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_colleagues_note_does_not_answer_a_question_and_questions_come_oldest_first() {
    let (daemon, _config) = daemon().await;
    let (a, b) = (create(&daemon, "a"), create(&daemon, "b"));
    let first = ask(&daemon, b.id, "b asks first");
    let second = ask(&daemon, a.id, "a asks second");
    daemon
        .runtime
        .send_message(a.id, vec![ContentBlock::text("fyi")], Actor::agent(b.id))
        .expect("note");

    let found = attention(&daemon).await;

    let order: Vec<u64> = found.questions.iter().map(|q| q.question_seq).collect();
    assert_eq!(order, [first, second]);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_long_question_is_cut_and_a_finished_agents_question_is_not_listed() {
    let (daemon, _config) = daemon().await;
    let asker = create(&daemon, "asker");
    let gone = create(&daemon, "gone");
    ask(&daemon, asker.id, &"é".repeat(MAX_TEXT_CHARS + 50));
    ask(&daemon, gone.id, "unreachable");
    amend(&daemon, gone.id, |a| {
        a.status = AgentStatus::Cancelled { reason: None }
    });

    let found = attention(&daemon).await;

    assert_eq!(found.questions.len(), 1);
    let text = &found.questions[0].question;
    assert_eq!(text.chars().count(), MAX_TEXT_CHARS + 1);
    assert!(text.ends_with('…'));
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_failed_agent_waits_for_attention_until_it_is_restarted_or_archived() {
    let (daemon, _config) = daemon().await;
    let failed = create(&daemon, "broken");
    let restarted = create(&daemon, "retried");
    let archived = create(&daemon, "old");
    let completed = create(&daemon, "fine");
    let error = "tool crashed\n\u{1b}[31mstack trace".to_owned();
    amend(&daemon, failed.id, |a| {
        a.status = AgentStatus::Failed { error }
    });
    amend(&daemon, restarted.id, |a| {
        a.status = AgentStatus::Failed { error: "x".into() };
        a.restarted_as = Some(AgentId::new());
    });
    amend(&daemon, archived.id, |a| {
        a.status = AgentStatus::Failed { error: "y".into() };
        a.archived = true;
    });
    amend(&daemon, completed.id, |a| {
        a.status = AgentStatus::Completed { summary: None }
    });

    let found = attention(&daemon).await;

    assert_eq!(found.agents.len(), 1);
    let item = &found.agents[0];
    assert_eq!(
        (item.agent, item.kind),
        (failed.id, AttentionKind::AgentFailed)
    );
    assert_eq!(item.reason, "tool crashed");
    assert_eq!(item.agent_name.as_deref(), Some("broken"));
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn only_a_block_the_operator_can_clear_needs_attention() {
    let (daemon, _config) = daemon().await;
    let inspect = create(&daemon, "inspect");
    let waiting = create(&daemon, "waiting");
    let spent = create(&daemon, "spent");
    amend(&daemon, inspect.id, |a| {
        a.status = AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: "deploy started without an outcome".into(),
            },
        }
    });
    amend(&daemon, waiting.id, |a| {
        a.status = AgentStatus::Blocked {
            reason: BlockReason::Dependency { on: TaskId::new() },
        }
    });
    amend(&daemon, spent.id, |a| {
        a.status = AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
                class: ExhaustionClass::QuotaExhausted,
                since: now(),
                reported_reset: None,
                next_probe: now() + Duration::hours(1),
                probes: 0,
                detail: "usage limit".into(),
            },
        }
    });

    let found = attention(&daemon).await;

    let mut ids: Vec<AgentId> = found.agents.iter().map(|a| a.agent).collect();
    ids.sort();
    let mut expected = [inspect.id, spent.id];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(
        found
            .agents
            .iter()
            .all(|a| a.kind == AttentionKind::AgentBlocked)
    );
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn every_list_is_bounded_and_keeps_what_the_operator_needs_first() {
    let (daemon, _config) = daemon().await;
    let agent = create(&daemon, "busy");
    let base = now();
    let extra = 5;
    for n in 0..MAX_ITEMS + extra {
        put_approval(
            &daemon,
            approval(agent.id, "shell", base + Duration::seconds(n as i64)),
        );
    }
    let agents: Vec<Agent> = (0..MAX_ITEMS + extra)
        .map(|n| create(&daemon, &format!("failing-{n}")))
        .collect();
    for (n, failing) in agents.iter().enumerate() {
        amend(&daemon, failing.id, |a| {
            a.status = AgentStatus::Failed {
                error: "boom".into(),
            };
            a.status_since = Some(base + Duration::seconds(n as i64));
        });
    }
    for failing in &agents[..MAX_ITEMS + extra] {
        ask(&daemon, failing.id, "q");
    }

    let found = attention(&daemon).await;

    assert_eq!(found.approvals.len(), MAX_ITEMS);
    assert_eq!(found.agents.len(), MAX_ITEMS);
    assert_eq!(
        found.agents[0].agent, agents[extra].id,
        "the newest failures are kept"
    );
    assert!(found.agents.windows(2).all(|w| w[0].since <= w[1].since));
    assert!(found.questions.is_empty(), "failed agents are not asked of");
    daemon.runtime.shutdown().await;
}

#[test]
fn text_helpers_cut_on_characters_and_flatten_to_one_line() {
    assert_eq!(truncated("abc", 3), "abc");
    assert_eq!(truncated("añb", 2), "añ…");
    assert_eq!(one_line("\n  first \u{7}line\nsecond"), "first line");
    assert_eq!(one_line(""), "");
    assert_eq!(
        one_line(&"x".repeat(1000)).chars().count(),
        MAX_REASON_CHARS + 1
    );
}
