//! `wait_for` on a sender that can never reply is refused at once; a wait on
//! a live agent, or on a message already delivered, still parks.

use super::*;

/// A root never scheduled, exactly as a pipeline creates its run node.
fn passive_node(runtime: &Runtime) -> Agent {
    let (node, task) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "pipeline:run".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("run node");
    assert!(task.is_none(), "a run node holds no task");
    assert_eq!(node.status.kind(), AgentStatusKind::Created);
    node
}

/// An idle, started agent: it can take turns and so send messages.
fn live_agent(runtime: &Runtime, parent: Option<AgentId>) -> Agent {
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "lead".into(),
                parent,
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("live agent");
    assert_ne!(agent.status.kind(), AgentStatusKind::Created);
    agent
}

/// Make the harness agent a stage agent under `parent`, holding an assigned
/// task, so a bare wait has nothing that could wake it.
fn adopt_stage_agent(harness: &mut ComposeHarness, parent: AgentId) {
    let (agent, task) = harness
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "stage".into(),
                parent: Some(parent),
                objective: Some("build it".into()),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("stage agent");
    let task = task.expect("stage task");
    assert_eq!(agent.parent, Some(parent));
    assert_eq!(agent.current_task, Some(task.id));
    harness.ctx.agent = agent;
}

fn refusal(harness: &ComposeHarness, condition: &WaitCondition) -> Option<ToolOutcome> {
    validate_wait(&harness.runtime, &harness.ctx, condition, false).expect("validate")
}

fn assert_refused_naming(outcome: Option<ToolOutcome>, sender: AgentId, why: &str) {
    let outcome = outcome.expect("the wait must be refused");
    assert!(outcome.is_error);
    for expected in [&sender.to_string(), why, "complete_task", "question tool"] {
        assert!(
            outcome.content.contains(expected),
            "missing {expected}: {}",
            outcome.content
        );
    }
}

#[tokio::test]
async fn message_from_a_passive_run_node_parent_is_refused() {
    let (mut harness, _dir) = ComposeHarness::new().await;
    let node = passive_node(&harness.runtime);
    adopt_stage_agent(&mut harness, node.id);

    let alone = WaitCondition::MessageFrom { agent: node.id };
    assert_refused_naming(refusal(&harness, &alone), node.id, "never started");
    let outcome = refusal(&harness, &alone).expect("refused");
    assert!(
        outcome.content.contains("your parent"),
        "{}",
        outcome.content
    );

    // Another wake source beside it does not make naming a mute sender right.
    let with_timer = WaitCondition::Any {
        of: vec![
            alone.clone(),
            WaitCondition::Deadline {
                at: now() + chrono::Duration::hours(1),
            },
        ],
    };
    assert_refused_naming(refusal(&harness, &with_timer), node.id, "never started");
}

#[tokio::test]
async fn message_from_a_finished_or_unknown_agent_is_refused() {
    let (harness, _dir) = ComposeHarness::new().await;
    let finished = live_agent(&harness.runtime, None);
    harness
        .runtime
        .cancel_agent(finished.id, None, Actor::user())
        .expect("cancel");
    let finished_wait = WaitCondition::MessageFrom { agent: finished.id };
    assert_refused_naming(
        refusal(&harness, &finished_wait),
        finished.id,
        "has finished",
    );

    let unknown = AgentId::new();
    let unknown_wait = WaitCondition::MessageFrom { agent: unknown };
    assert_refused_naming(
        refusal(&harness, &unknown_wait),
        unknown,
        "not a known agent",
    );
}

/// Every terminal status refuses, not only a cancellation.
#[tokio::test]
async fn message_from_a_completed_failed_or_imported_agent_is_refused() {
    let (harness, _dir) = ComposeHarness::new().await;
    for status in [
        AgentStatus::Completed { summary: None },
        AgentStatus::Failed {
            error: "broke".into(),
        },
        AgentStatus::Imported {
            was: AgentStatusKind::Completed,
        },
    ] {
        let mut sender = live_agent(&harness.runtime, None);
        let kind = status.kind();
        sender.status = status;
        harness
            .runtime
            .store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(sender.clone())))
            .expect("store the terminal sender");
        let wait = WaitCondition::MessageFrom { agent: sender.id };
        assert_refused_naming(
            refusal(&harness, &wait),
            sender.id,
            &format!("has finished (status {kind})"),
        );
    }
}

#[tokio::test]
async fn a_message_a_finished_sender_already_sent_still_wakes_the_wait() {
    let (harness, _dir) = ComposeHarness::new().await;
    let sender = live_agent(&harness.runtime, None);
    harness
        .runtime
        .send_message(
            harness.ctx.agent.id,
            vec![ContentBlock::text("done, see my result")],
            Actor::agent(sender.id),
        )
        .expect("send");
    harness
        .runtime
        .cancel_agent(sender.id, None, Actor::user())
        .expect("cancel");
    let wait = WaitCondition::MessageFrom { agent: sender.id };
    assert!(refusal(&harness, &wait).is_none(), "its message is waiting");
}

#[tokio::test]
async fn message_from_a_live_parent_still_waits() {
    let (mut harness, _dir) = ComposeHarness::new().await;
    let parent = live_agent(&harness.runtime, None);
    adopt_stage_agent(&mut harness, parent.id);
    let wait = WaitCondition::MessageFrom { agent: parent.id };
    assert!(
        refusal(&harness, &wait).is_none(),
        "a live parent can answer"
    );
}

#[tokio::test]
async fn a_bare_wait_under_a_run_node_names_the_parent_that_cannot_answer() {
    let (mut harness, _dir) = ComposeHarness::new().await;
    let node = passive_node(&harness.runtime);
    adopt_stage_agent(&mut harness, node.id);
    let outcome = refusal(&harness, &WaitCondition::Inbox).expect("bare wait refused");
    assert!(outcome.content.contains("Nothing can wake you"));
    assert!(
        outcome
            .content
            .contains(&format!("Your parent `{}` cannot answer", node.id)),
        "{}",
        outcome.content
    );

    // Under a live parent the same refusal says nothing about the parent.
    let (mut harness, _dir) = ComposeHarness::new().await;
    let parent = live_agent(&harness.runtime, None);
    adopt_stage_agent(&mut harness, parent.id);
    let outcome = refusal(&harness, &WaitCondition::Inbox).expect("bare wait refused");
    assert!(outcome.content.contains("Nothing can wake you"));
    assert!(
        !outcome.content.contains("Your parent"),
        "{}",
        outcome.content
    );
}
