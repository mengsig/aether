//! `create_schedule` with `spawn`, and `delegate`, through the tool path: the
//! schedule is admitted as the caller's own `delegate` call.

use super::delegation_policy::{adaptive_policy, delegate, lead_harness, run};
use super::*;
use aether_core::agent::{Agent, AgentConfig};
use aether_core::model::Effort;
use aether_core::schedule::{
    BusyPolicy, CreateScheduleSpec, MissedRunPolicy, Recurrence, Schedule, ScheduleTarget,
    SpawnSpec,
};
use std::num::NonZeroU64;

fn spawn_schedule(model: &str, effort: Effort) -> HarnessAction {
    let template = SpawnSpec {
        role: "poller".into(),
        name: None,
        objective: "Check the queue".into(),
        model: Some(ModelRef::parse(model).expect("provider/model")),
        fallbacks: Vec::new(),
        scope: None,
        config: Some(AgentConfig {
            effort: Some(effort),
            ..AgentConfig::default()
        }),
        approval: None,
    };
    HarnessAction::CreateSchedule {
        spec: CreateScheduleSpec {
            name: "Poll".into(),
            target: ScheduleTarget::Spawn(template),
            recurrence: Recurrence::Interval {
                seconds: NonZeroU64::new(3600).expect("non-zero"),
            },
            missed: MissedRunPolicy::Coalesce,
            busy: BusyPolicy::Skip,
            payload: serde_json::Value::Null,
            max_deliveries: None,
        },
    }
}

/// The harness's lead, acting as itself the way a real turn does.
async fn acting_lead() -> (ComposeHarness, tempfile::TempDir) {
    let (mut h, dir) = lead_harness(adaptive_policy()).await;
    h.ctx.base_actor = Actor::agent(h.ctx.agent.id);
    (h, dir)
}

fn rewrite_lead(h: &mut ComposeHarness, patch: impl FnOnce(&mut Agent)) {
    let mut lead = h
        .runtime
        .store
        .agent(h.ctx.agent.id)
        .expect("read")
        .expect("lead");
    patch(&mut lead);
    h.ctx.agent = lead.clone();
    h.runtime
        .store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(lead)))
        .expect("rewrite the lead");
}

fn children(h: &ComposeHarness) -> Vec<Agent> {
    h.runtime
        .store
        .agents(&aether_store::AgentFilter {
            parent: Some(h.ctx.agent.id),
            ..Default::default()
        })
        .expect("children")
}

#[tokio::test]
async fn a_spawn_schedule_is_admitted_as_the_callers_delegate_call() {
    let (mut h, _dir) = acting_lead().await;
    let above = run(
        &mut h,
        spawn_schedule("anthropic/claude-fable-5-2", Effort::Low),
    )
    .await;
    assert!(above.is_error, "{}", above.content);
    assert!(
        above
            .content
            .starts_with("Cannot create schedule: delegation refused:")
            && above
                .content
                .contains("the model is outside your delegation policy"),
        "{}",
        above.content
    );

    let created = run(
        &mut h,
        spawn_schedule("anthropic/claude-sonnet-5-5", Effort::Medium),
    )
    .await;
    assert!(!created.is_error, "{}", created.content);
    let reply: serde_json::Value = serde_json::from_str(&created.content).expect("json");
    let job: Schedule = serde_json::from_value(reply["schedule"].clone()).expect("schedule");
    assert_eq!(job.owner, Actor::agent(h.ctx.agent.id));
    assert!(children(&h).is_empty(), "nothing runs before it is due");

    crate::schedules::tick(&h.runtime, job.status.next_due().expect("active")).expect("tick");
    let born = children(&h);
    assert_eq!(born.len(), 1);
    assert_eq!(born[0].run, h.ctx.agent.run);
    assert_eq!(
        born[0].model,
        ModelRef::new("anthropic", "claude-sonnet-5-5")
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_leaf_caller_gets_delegates_refusal_for_a_spawn_schedule() {
    let (mut h, _dir) = acting_lead().await;
    rewrite_lead(&mut h, |lead| lead.config.leaf = true);
    let refused = run(
        &mut h,
        spawn_schedule("anthropic/claude-sonnet-5-5", Effort::Medium),
    )
    .await;
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused
            .content
            .starts_with("Leaf agents cannot delegate children"),
        "{}",
        refused.content
    );
    assert!(
        h.runtime
            .store
            .schedules(None)
            .expect("schedules")
            .is_empty()
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_template_on_a_model_that_cannot_run_is_refused_as_delegate_refuses_it() {
    let (mut h, _dir) = acting_lead().await;
    let schedule = run(&mut h, spawn_schedule("nowhere/none", Effort::Medium)).await;
    let direct = run(
        &mut h,
        delegate("poller", "nowhere/none", Effort::Medium, None),
    )
    .await;
    assert!(schedule.is_error, "{}", schedule.content);
    assert!(
        schedule
            .content
            .starts_with("Cannot delegate to model `nowhere/none`"),
        "{}",
        schedule.content
    );
    assert_eq!(schedule.content, direct.content);
    assert!(
        h.runtime
            .store
            .schedules(None)
            .expect("schedules")
            .is_empty()
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_delegated_child_does_not_inherit_its_parents_compaction_threshold() {
    let (mut h, _dir) = acting_lead().await;
    rewrite_lead(&mut h, |lead| {
        lead.config.compaction_tokens = Some(150_000);
    });
    let delegated = run(
        &mut h,
        delegate(
            "worker",
            "anthropic/claude-sonnet-5-5",
            Effort::Medium,
            None,
        ),
    )
    .await;
    assert!(!delegated.is_error, "{}", delegated.content);
    let born = children(&h);
    assert_eq!(born.len(), 1);
    assert_eq!(
        born[0].config.compaction_tokens, None,
        "chosen for the parent's model and context"
    );
    assert_eq!(h.ctx.agent.config.compaction_tokens, Some(150_000));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_spawn_schedule_alone_does_not_let_its_creator_wait_bare() {
    let (mut h, _dir) = acting_lead().await;
    let mut task = h.ctx.task.clone().expect("the lead has a task");
    task.status = aether_core::task::TaskStatus::Active;
    h.runtime
        .store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertTask(task)))
        .expect("activate the lead's task");
    let nothing_wakes =
        |h: &ComposeHarness| super::waiting::bare_wait_has_no_wake_source(&h.runtime, &h.ctx.agent);

    let created = run(
        &mut h,
        spawn_schedule("anthropic/claude-sonnet-5-5", Effort::Medium),
    )
    .await;
    assert!(!created.is_error, "{}", created.content);
    assert!(
        nothing_wakes(&h).expect("read"),
        "a schedule that can end without a child must not park its creator"
    );

    let reply: serde_json::Value = serde_json::from_str(&created.content).expect("json");
    let job: Schedule = serde_json::from_value(reply["schedule"].clone()).expect("schedule");
    let due = job.status.next_due().expect("active");
    crate::schedules::tick(&h.runtime, due).expect("tick");
    assert_eq!(children(&h).len(), 1, "the occurrence started a child");
    assert!(
        !nothing_wakes(&h).expect("read"),
        "the child reports to its creator like any other"
    );
    h.runtime.shutdown().await;
}

async fn schedule_tool(
    h: &mut ComposeHarness,
    name: &str,
    input: serde_json::Value,
) -> ToolEffects {
    let ctx = Arc::new(tool_context(&h.runtime, &h.ctx));
    run_tool_calls(
        &h.runtime,
        &mut h.ctx,
        &ctx,
        InvocationId::new(),
        vec![("schedule".into(), name.into(), input)],
        &CancellationToken::new(),
    )
    .await
    .expect("run schedule tool")
}

async fn answer_schedule_tool(h: &mut ComposeHarness, effects: ToolEffects, tool: &str) {
    assert_eq!(effects.pending_approvals.len(), 1, "{tool}");
    let id = effects.pending_approvals[0];
    let approval = h.runtime.store.approval(id).unwrap().unwrap();
    assert_eq!(approval.tool, tool);
    assert_eq!(
        approval.side_effect,
        aether_core::tool::SideEffectClass::Mutation
    );
    h.runtime
        .resolve_approval(id, ApprovalDecision::Approved, Actor::user(), None, None)
        .expect("approve schedule tool");
    let ctx = Arc::new(tool_context(&h.runtime, &h.ctx));
    settle_resolved_approvals(&h.runtime, &mut h.ctx, &ctx, &CancellationToken::new())
        .await
        .expect("settle approval");
}

fn stored_schedules(h: &ComposeHarness) -> Vec<Schedule> {
    h.runtime.store.schedules(None).expect("schedules")
}

#[tokio::test]
async fn api_schedule_spawn_create_and_resume_gate_only_mutations() {
    use aether_core::schedule::ScheduleStatus;
    use serde_json::json;

    for gated in [true, false] {
        let (mut h, _dir) = acting_lead().await;
        if gated {
            rewrite_lead(&mut h, |lead| {
                lead.config.approval.require_for =
                    [aether_core::tool::SideEffectClass::Mutation].into();
            });
        }
        let input = json!({"name":"poll","payload":null,"every_seconds":3600,"spawn":{
            "role":"poller","objective":"Check the queue",
            "model":"anthropic/claude-sonnet-5-5","effort":"medium"
        }});
        let create = schedule_tool(&mut h, "create_schedule", input).await;
        if gated {
            assert!(stored_schedules(&h).is_empty());
            answer_schedule_tool(&mut h, create, "create_schedule").await;
        } else {
            assert!(create.pending_approvals.is_empty());
        }
        let jobs = stored_schedules(&h);
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.spawns_children_of(), Some(h.ctx.agent.id));
        assert!(matches!(job.status, ScheduleStatus::Active { .. }));
        let id = job.id;
        let pause =
            schedule_tool(&mut h, "update_schedule", json!({"id":id,"action":"pause"})).await;
        assert!(pause.pending_approvals.is_empty());
        assert!(matches!(
            stored_schedules(&h)[0].status,
            ScheduleStatus::Paused { .. }
        ));

        let resume = schedule_tool(
            &mut h,
            "update_schedule",
            json!({"id":id,"action":"resume"}),
        )
        .await;
        if gated {
            assert!(matches!(
                stored_schedules(&h)[0].status,
                ScheduleStatus::Paused { .. }
            ));
            answer_schedule_tool(&mut h, resume, "update_schedule").await;
        } else {
            assert!(resume.pending_approvals.is_empty());
        }
        assert!(matches!(
            stored_schedules(&h)[0].status,
            ScheduleStatus::Active { .. }
        ));
        assert!(children(&h).is_empty());
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn api_plain_schedule_create_and_resume_stay_read_only() {
    use aether_core::schedule::ScheduleStatus;
    use serde_json::json;

    let (mut h, _dir) = acting_lead().await;
    rewrite_lead(&mut h, |lead| {
        lead.config.approval.require_for = [aether_core::tool::SideEffectClass::Mutation].into();
    });
    let create = schedule_tool(
        &mut h,
        "create_schedule",
        json!({
            "name":"message","payload":"check in","every_seconds":3600
        }),
    )
    .await;
    assert!(create.pending_approvals.is_empty());
    let jobs = stored_schedules(&h);
    assert_eq!(jobs.len(), 1);
    let id = jobs[0].id;
    assert_eq!(jobs[0].target(), Some(h.ctx.agent.id));
    for (action, active) in [("pause", false), ("resume", true)] {
        let update =
            schedule_tool(&mut h, "update_schedule", json!({"id":id,"action":action})).await;
        assert!(update.pending_approvals.is_empty());
        assert_eq!(
            matches!(
                stored_schedules(&h)[0].status,
                ScheduleStatus::Active { .. }
            ),
            active
        );
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn spawn_schedule_admission_rechecks_the_current_policy() {
    use aether_core::schedule::{ScheduleAction, ScheduleStatus};
    use aether_core::tool::SideEffectClass::Mutation;

    let (mut h, _dir) = acting_lead().await;
    let HarnessAction::CreateSchedule { spec } =
        spawn_schedule("anthropic/claude-sonnet-5-5", Effort::Medium)
    else {
        panic!("expected a spawn schedule");
    };
    let by = h.ctx.base_actor.clone();
    let job = h.runtime.create_schedule(spec.clone(), by.clone()).unwrap();
    h.runtime
        .update_schedule(job.id, ScheduleAction::Pause, by.clone())
        .unwrap();
    rewrite_lead(&mut h, |lead| {
        lead.config.approval.require_for = [Mutation].into()
    });

    let create = h
        .runtime
        .create_schedule_from_tool(spec, by.clone(), false)
        .unwrap_err();
    let resume = h
        .runtime
        .update_schedule_from_tool(job.id, ScheduleAction::Resume, by, false)
        .unwrap_err();
    for refusal in [create, resume] {
        assert!(
            refusal.to_string().contains("requires mutation approval"),
            "{refusal}"
        );
    }
    let jobs = stored_schedules(&h);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job.id);
    assert!(matches!(jobs[0].status, ScheduleStatus::Paused { .. }));
    h.runtime.shutdown().await;
}
