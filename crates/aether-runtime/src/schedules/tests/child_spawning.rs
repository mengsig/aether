//! Spawn schedules an agent creates. Each occurrence is its creator's
//! `delegate` call, so each test holds one to something `delegate` is held to.

use super::spawning::{end_run, skip_reason, spawned};
use super::*;
use crate::schedules::execution;
use aether_core::agent::{AgentConfig, HostContext, NetworkPolicy, Scope};
use aether_core::delegation::{DelegationPolicy, ModelTier, WorkerChoice};
use aether_core::model::Effort;

pub(super) const FAST: (&str, &str) = ("anthropic", "claude-haiku-5-5");
pub(super) const BALANCED: (&str, &str) = ("anthropic", "claude-sonnet-5-5");
pub(super) const FRONTIER: (&str, &str) = ("anthropic", "claude-fable-5-2");

pub(super) fn model((provider, id): (&str, &str)) -> ModelRef {
    ModelRef::new(provider, id)
}

pub(super) fn fast_only() -> DelegationPolicy {
    DelegationPolicy {
        workers: vec![WorkerChoice {
            model: None,
            tier: Some(ModelTier::Fast),
            effort: Some(Effort::Medium),
        }],
        ..DelegationPolicy::default()
    }
}

/// Any worker the tier ceiling allows, `limit` of them unfinished at once.
pub(super) fn at_most(limit: u32) -> DelegationPolicy {
    DelegationPolicy {
        max_parallel: Some(limit),
        ..DelegationPolicy::default()
    }
}

/// An unstarted root on `chosen`, with `config` and network access.
pub(super) fn creator_with(runtime: &Runtime, chosen: (&str, &str), config: AgentConfig) -> Agent {
    let lead = runtime
        .create_agent(
            CreateAgentSpec {
                role: "lead".into(),
                model: Some(model(chosen)),
                scope: Some(Scope {
                    network: NetworkPolicy::Allowed,
                    ..Scope::default()
                }),
                config: Some(AgentConfig {
                    effort: Some(Effort::Medium),
                    ..config
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap()
        .0;
    assert_eq!(lead.parent, None);
    lead
}

/// A balanced-tier lead with `policy`.
pub(super) fn creator(runtime: &Runtime, policy: Option<DelegationPolicy>) -> Agent {
    let config = AgentConfig {
        delegation_policy: policy,
        ..AgentConfig::default()
    };
    creator_with(runtime, BALANCED, config)
}

/// Rewrite an agent's row, as an operator's update or its own progress would.
pub(super) fn change(runtime: &Runtime, id: AgentId, patch: impl FnOnce(&mut Agent)) -> Agent {
    let mut agent = runtime.store.agent(id).unwrap().unwrap();
    patch(&mut agent);
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    agent
}

pub(super) fn child_template(chosen: (&str, &str), effort: Effort) -> SpawnSpec {
    SpawnSpec {
        role: "poller".into(),
        name: None,
        objective: "Check the queue".into(),
        model: Some(model(chosen)),
        fallbacks: Vec::new(),
        scope: None,
        config: Some(AgentConfig {
            effort: Some(effort),
            ..AgentConfig::default()
        }),
        approval: None,
    }
}

pub(super) fn child_spec(template: SpawnSpec) -> CreateScheduleSpec {
    CreateScheduleSpec {
        target: ScheduleTarget::Spawn(template),
        max_deliveries: None,
        ..spec(AgentId::new())
    }
}

pub(super) fn schedule(runtime: &Runtime, creator: &Agent, template: SpawnSpec) -> Schedule {
    let job = runtime
        .create_schedule(child_spec(template), Actor::agent(creator.id))
        .unwrap();
    assert_eq!(job.spawns_children_of(), Some(creator.id));
    job
}

pub(super) fn refused(runtime: &Runtime, creator: &Agent, template: SpawnSpec) -> String {
    let before = runtime.store.last_seq().unwrap();
    let error = runtime
        .create_schedule(child_spec(template), Actor::agent(creator.id))
        .expect_err("refused")
        .to_string();
    assert_eq!(
        runtime.store.last_seq().unwrap(),
        before,
        "a refusal writes nothing"
    );
    error
}

pub(super) fn children(runtime: &Runtime, creator: AgentId) -> Vec<Agent> {
    runtime
        .store
        .agents(&aether_store::AgentFilter {
            parent: Some(creator),
            ..Default::default()
        })
        .unwrap()
}

/// Fire the schedule's next occurrence and return it as that left it.
pub(super) fn fire(runtime: &Runtime, id: aether_core::id::ScheduleId) -> Schedule {
    let due = runtime
        .store
        .schedule(id)
        .unwrap()
        .unwrap()
        .status
        .next_due();
    tick(runtime, due.expect("an active schedule")).unwrap();
    runtime.store.schedule(id).unwrap().unwrap()
}

/// The error a schedule ended with, and the same error in its history.
pub(super) fn ended_with(job: &Schedule) -> String {
    let ScheduleStatus::Failed { error, .. } = &job.status else {
        panic!("expected a failed schedule, got {:?}", job.status);
    };
    let last = job
        .last
        .as_ref()
        .expect("the ending occurrence is recorded");
    assert_eq!(
        last.outcome,
        ScheduleOutcome::Failed {
            error: error.clone()
        }
    );
    error.clone()
}

#[tokio::test]
async fn an_agents_fast_spawn_runs_as_its_child_in_its_run() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(fast_only()));
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Medium));
    assert_eq!(job.owner, Actor::agent(lead.id));
    assert!(
        children(&runtime, lead.id).is_empty(),
        "the creation check is a dry run"
    );

    let due = job.status.next_due().unwrap();
    tick(&runtime, due).unwrap();
    // The same due time again, as after a restart that replays the pass.
    tick(&runtime, due).unwrap();
    let born = children(&runtime, lead.id);
    assert_eq!(born.len(), 1, "one child per due time");
    let child = &born[0];
    assert_eq!(child.parent, Some(lead.id));
    assert_eq!(child.run, lead.run, "a child joins its creator's run");
    assert_eq!(child.model, model(FAST));
    assert_eq!(child.config.effort, Some(Effort::Medium));
    assert_eq!(child.config.delegation_policy, None);
    assert_eq!(child.status, AgentStatus::Ready);
    let task = runtime
        .store
        .task(child.current_task.unwrap())
        .unwrap()
        .unwrap();
    assert!(task.objective.starts_with("Check the queue"));
    assert!(
        task.objective
            .contains(&format!("`Read docs` ({})", job.id))
    );

    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert_eq!(stored.deliveries, 1);
    assert_eq!(spawned(&stored), Some(child.id));
    assert_eq!(
        stored.last.as_ref().map(|last| &last.outcome),
        Some(&ScheduleOutcome::Spawned {
            agent: child.id,
            task: task.id,
            run: lead.run,
        })
    );
}

#[tokio::test]
async fn a_template_delegate_would_refuse_is_refused_when_scheduled() {
    let (runtime, _dir) = fixture().await;
    let bounded = creator(&runtime, Some(fast_only()));
    let above_policy = refused(&runtime, &bounded, child_template(BALANCED, Effort::Low));
    assert!(
        above_policy.contains("the model is outside your delegation policy"),
        "{above_policy}"
    );
    assert!(
        above_policy.contains("any fast-tier model at effort medium or lower"),
        "{above_policy}"
    );
    let too_hard = refused(&runtime, &bounded, child_template(FAST, Effort::High));
    assert!(too_hard.contains("effort high is above"), "{too_hard}");

    let fast_lead = creator_with(&runtime, FAST, AgentConfig::default());
    let above_tier = refused(&runtime, &fast_lead, child_template(BALANCED, Effort::Low));
    assert!(
        above_tier.contains("above your own fast tier"),
        "{above_tier}"
    );

    let frontier = creator_with(&runtime, FRONTIER, AgentConfig::default());
    let clone = refused(&runtime, &frontier, child_template(FRONTIER, Effort::Low));
    assert!(
        clone.contains("is your own frontier-tier model")
            && clone.contains("cannot carry the `reason`"),
        "{clone}"
    );
    assert!(runtime.store.schedules(None).unwrap().is_empty());
}

#[tokio::test]
async fn a_template_takes_only_what_delegate_takes() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    type Patch = fn(&mut SpawnSpec);
    let patches: [(Patch, &str); 6] = [
        (|t| t.model = None, "names its model and effort"),
        (
            |t| t.config.as_mut().unwrap().effort = None,
            "names its model and effort",
        ),
        (
            |t| t.config.as_mut().unwrap().cost_budget = Some(50.0),
            "takes only what `delegate` takes",
        ),
        (
            |t| t.config.as_mut().unwrap().delegation_policy = Some(DelegationPolicy::default()),
            "takes only what `delegate` takes",
        ),
        (
            |t| t.fallbacks = vec![model(FAST)],
            "takes only what `delegate` takes",
        ),
        (
            |t| t.approval = Some(Default::default()),
            "takes only what `delegate` takes",
        ),
    ];
    for (patch, expected) in patches {
        let mut template = child_template(FAST, Effort::Low);
        patch(&mut template);
        let error = refused(&runtime, &lead, template);
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let mut queued = child_spec(child_template(FAST, Effort::Low));
    queued.busy = BusyPolicy::QueueOne;
    let error = runtime
        .create_schedule(queued, Actor::agent(lead.id))
        .unwrap_err();
    assert!(error.to_string().contains("queue_one"), "{error}");
}

#[tokio::test]
async fn a_policy_tightened_after_creation_ends_the_schedule_with_delegates_refusal() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(BALANCED, Effort::Low));
    change(&runtime, lead.id, |lead| {
        lead.config.delegation_policy = Some(fast_only());
    });

    let error = ended_with(&fire(&runtime, job.id));
    assert!(
        error.contains("the model is outside your delegation policy"),
        "{error}"
    );
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn the_tier_ceiling_is_the_creators_tier_when_the_occurrence_fires() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(BALANCED, Effort::Low));
    change(&runtime, lead.id, |lead| lead.model = model(FAST));

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("above your own fast tier"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn two_schedules_under_max_parallel_one_never_run_two_children_at_once() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(at_most(1)));
    let first = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let second = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let live = |runtime: &Runtime| {
        children(runtime, lead.id)
            .into_iter()
            .filter(Agent::is_live)
            .collect::<Vec<_>>()
    };
    let mut at = first
        .status
        .next_due()
        .max(second.status.next_due())
        .unwrap();
    let mut started = std::collections::BTreeSet::new();
    for round in 0..6 {
        tick(&runtime, at).unwrap();
        let running = live(&runtime);
        assert_eq!(running.len(), 1, "round {round}: {running:?}");
        started.insert(running[0].id);
        let jobs = [first.id, second.id].map(|id| runtime.store.schedule(id).unwrap().unwrap());
        assert!(
            jobs.iter()
                .all(|job| matches!(job.status, ScheduleStatus::Active { .. })),
            "waiting for a slot ends neither schedule: {jobs:?}"
        );
        let waiting: Vec<String> = jobs.iter().filter_map(skip_reason).collect();
        assert!(
            waiting
                .iter()
                .any(|why| why.contains("at most 1 unfinished children at once")),
            "round {round}: {waiting:?}"
        );
        // Every other round the slot frees, and exactly one schedule takes it.
        if round % 2 == 1 {
            end_run(&runtime, running[0].id);
        }
        at = first.recurrence.next_after(at).unwrap();
    }
    assert_eq!(started.len(), 3, "a freed slot was taken each time");
    assert_eq!(children(&runtime, lead.id).len(), 3);
}

#[tokio::test]
async fn a_full_max_parallel_refuses_a_new_schedule_as_it_refuses_delegate() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(at_most(1)));
    agent(&runtime, Some(lead.id));
    let error = refused(&runtime, &lead, child_template(FAST, Effort::Low));
    assert!(
        error.contains("at most 1 unfinished children at once"),
        "{error}"
    );
}

#[tokio::test]
async fn a_schedule_ends_with_its_creator_and_cannot_be_resumed() {
    for cancelled in [false, true] {
        let (runtime, _dir) = fixture().await;
        let lead = creator(&runtime, None);
        let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
        let state = if cancelled {
            runtime.cancel_agent(lead.id, None, Actor::user()).unwrap();
            "cancelled"
        } else {
            end_run(&runtime, lead.id);
            "completed"
        };

        let error = ended_with(&fire(&runtime, job.id));
        let expected = format!(
            "its creator {} is {state}: an agent's spawn schedule ends with the agent",
            lead.id
        );
        assert_eq!(error, expected);
        assert!(children(&runtime, lead.id).is_empty());
        let resumed = runtime
            .update_schedule(job.id, ScheduleAction::Resume, Actor::user())
            .unwrap_err();
        assert!(resumed.to_string().contains(&expected), "{resumed}");
        let stored = runtime.store.schedule(job.id).unwrap().unwrap();
        assert!(matches!(stored.status, ScheduleStatus::Failed { .. }));
    }
}

#[tokio::test]
async fn a_creator_being_cancelled_gets_no_schedule_and_no_child() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    // Requested but not yet carried out: the creator is still live.
    crate::cancellation::request(&runtime, lead.id, None, Actor::user()).unwrap();
    assert!(runtime.store.agent(lead.id).unwrap().unwrap().is_live());

    let error = refused(&runtime, &lead, child_template(FAST, Effort::Low));
    assert!(error.contains("subtree is being cancelled"), "{error}");
    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("subtree is being cancelled"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_leaf_creator_cannot_schedule_children_then_or_later() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    change(&runtime, lead.id, |lead| lead.config.leaf = true);

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("Leaf agents cannot delegate"), "{error}");
    let error = refused(&runtime, &lead, child_template(FAST, Effort::Low));
    assert!(error.contains("Leaf agents cannot delegate"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_scheduled_child_starts_in_its_creators_scope_as_it_is_when_it_fires() {
    let (runtime, dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let narrowed = dir.path().join("narrowed");
    std::fs::create_dir_all(&narrowed).unwrap();
    let lead = change(&runtime, lead.id, |lead| {
        lead.scope.network = NetworkPolicy::Denied;
        lead.scope.workspace_root = Some(narrowed.clone());
    });

    let stored = fire(&runtime, job.id);
    let child = runtime
        .store
        .agent(spawned(&stored).expect("a child"))
        .unwrap()
        .unwrap();
    assert_eq!(child.scope.network, NetworkPolicy::Denied);
    assert_eq!(child.scope.workspace_root, lead.scope.workspace_root);
}

#[tokio::test]
async fn a_template_scope_wider_than_its_creators_is_refused_then_or_later() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut template = child_template(FAST, Effort::Low);
    template.scope = Some(lead.scope.clone());
    let job = schedule(&runtime, &lead, template.clone());
    change(&runtime, lead.id, |lead| {
        lead.scope.network = NetworkPolicy::Denied;
    });

    let widens = "child scope must narrow the parent's scope; `network` widens it";
    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains(widens), "{error}");
    let error = refused(&runtime, &lead, template);
    assert!(error.contains(widens), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn the_run_budget_refuses_a_schedule_and_its_children_as_it_refuses_delegate() {
    let (runtime, _dir) = fixture_with_run_budget(Some(1.0)).await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    change(&runtime, lead.id, |lead| lead.stats.usage.cost_usd = 2.0);

    let error = refused(&runtime, &lead, child_template(FAST, Effort::Low));
    assert!(error.contains("`[scheduler] run_budget_usd`"), "{error}");
    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("`[scheduler] run_budget_usd`"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_scheduled_child_inherits_its_creators_limits_and_host_context() {
    let (runtime, _dir) = fixture().await;
    let deadline = now() + chrono::Duration::hours(1);
    let config = AgentConfig {
        cost_budget: Some(5.0),
        deadline: Some(deadline),
        host_context: HostContext::Lean,
        compaction_tokens: Some(150_000),
        ..AgentConfig::default()
    };
    let lead = creator_with(&runtime, BALANCED, config);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));

    let stored = fire(&runtime, job.id);
    let child = runtime
        .store
        .agent(spawned(&stored).expect("a child"))
        .unwrap()
        .unwrap();
    assert_eq!(child.config.cost_budget, Some(5.0));
    assert_eq!(child.config.deadline, Some(deadline));
    assert_eq!(
        child.config.host_context,
        HostContext::Lean,
        "a lean creator's scheduled child stays lean"
    );
    assert_eq!(
        child.config.compaction_tokens, None,
        "chosen for the creator's model, not the child's"
    );
}

#[tokio::test]
async fn a_full_creators_template_can_ask_for_a_lean_child() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut template = child_template(FAST, Effort::Low);
    template.config.as_mut().unwrap().host_context = HostContext::Lean;
    let lean = schedule(&runtime, &lead, template);
    let inherited = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let host_context = |job: &Schedule| {
        let child = spawned(&fire(&runtime, job.id)).expect("a child");
        let child = runtime.store.agent(child).unwrap().unwrap();
        child.config.host_context
    };
    assert_eq!(host_context(&lean), HostContext::Lean);
    assert_eq!(host_context(&inherited), HostContext::Full);
}

#[tokio::test]
async fn a_scheduled_childs_reach_is_a_childs_reach() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let stranger = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let child = spawned(&fire(&runtime, job.id)).expect("a child");
    let child = runtime.store.agent(child).unwrap().unwrap();

    let reach = |to| crate::peers::message_reach_refusal(&runtime, &child, to, false).unwrap();
    assert_eq!(reach(lead.id), None);
    let refusal = reach(stranger.id).expect("another run's root is out of reach");
    assert!(refusal.contains("outside your reach"), "{refusal}");
}

#[tokio::test]
async fn a_spawn_skips_while_its_previous_child_or_anything_beneath_it_is_live() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let previous = spawned(&fire(&runtime, job.id)).expect("a child");

    let stored = fire(&runtime, job.id);
    let expected = format!("previous child {previous} is still live: {previous} is ready");
    assert_eq!(skip_reason(&stored).as_deref(), Some(expected.as_str()));
    assert_eq!(stored.deliveries, 1, "a skipped occurrence is not counted");

    let grandchild = agent(&runtime, Some(previous));
    end_run(&runtime, previous);
    let stored = fire(&runtime, job.id);
    let expected = format!(
        "previous child {previous} is still live: {} is created",
        grandchild.id
    );
    assert_eq!(skip_reason(&stored).as_deref(), Some(expected.as_str()));
    assert_eq!(children(&runtime, lead.id).len(), 1);

    end_run(&runtime, grandchild.id);
    let stored = fire(&runtime, job.id);
    assert_ne!(spawned(&stored), Some(previous));
    assert_eq!(children(&runtime, lead.id).len(), 2);
    assert_eq!(stored.deliveries, 2);
}

#[tokio::test]
async fn max_deliveries_bounds_an_agents_spawn_schedule() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let spec = CreateScheduleSpec {
        max_deliveries: NonZeroU64::new(1),
        ..child_spec(child_template(FAST, Effort::Low))
    };
    let job = runtime
        .create_schedule(spec, Actor::agent(lead.id))
        .unwrap();
    let first = job.status.next_due().unwrap();
    tick(&runtime, first).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert!(matches!(stored.status, ScheduleStatus::Exhausted { .. }));
    assert_eq!(stored.deliveries, 1);

    end_run(&runtime, spawned(&stored).unwrap());
    tick(&runtime, job.recurrence.next_after(first).unwrap()).unwrap();
    assert_eq!(children(&runtime, lead.id).len(), 1);
}

#[tokio::test]
async fn the_owning_agent_pauses_resumes_and_cancels_its_spawn_schedule() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let other = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let due = job.status.next_due().unwrap();
    let act = |action, by: &Agent| runtime.update_schedule(job.id, action, Actor::agent(by.id));

    let error = act(ScheduleAction::Pause, &other).unwrap_err();
    assert!(error.to_string().contains("only the owner"), "{error}");
    act(ScheduleAction::Pause, &lead).unwrap();
    tick(&runtime, due).unwrap();
    assert!(children(&runtime, lead.id).is_empty(), "paused");

    act(ScheduleAction::Resume, &lead).unwrap();
    let resumed = runtime.store.schedule(job.id).unwrap().unwrap();
    tick(&runtime, resumed.status.next_due().unwrap()).unwrap();
    assert_eq!(children(&runtime, lead.id).len(), 1);

    let cancelled = act(ScheduleAction::Cancel, &lead).unwrap();
    assert!(matches!(cancelled.status, ScheduleStatus::Cancelled { .. }));
    end_run(&runtime, children(&runtime, lead.id)[0].id);
    tick(&runtime, now() + chrono::Duration::days(1)).unwrap();
    assert_eq!(children(&runtime, lead.id).len(), 1);
}

#[tokio::test]
async fn a_paused_creator_skips_occurrences_without_ending_the_schedule() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    change(&runtime, lead.id, |lead| lead.paused = true);

    let stored = fire(&runtime, job.id);
    assert_eq!(skip_reason(&stored).as_deref(), Some("creator is paused"));
    assert!(matches!(stored.status, ScheduleStatus::Active { .. }));
    assert!(children(&runtime, lead.id).is_empty());

    change(&runtime, lead.id, |lead| lead.paused = false);
    assert!(spawned(&fire(&runtime, job.id)).is_some());
}

#[tokio::test]
async fn a_child_staged_against_a_creator_that_has_since_changed_is_not_started() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut job = schedule(&runtime, &lead, child_template(BALANCED, Effort::Low));
    let due = job.status.next_due().unwrap();
    let staged = execution::stage_spawn(&runtime, &job, due);
    // Admitted while the creator had no policy; it has one by the hold.
    change(&runtime, lead.id, |lead| {
        lead.config.delegation_policy = Some(fast_only());
    });

    let hold = runtime.hold();
    let mut commit = Commit::new();
    let fired = execution::prepare_occurrence(
        &runtime,
        &hold,
        &mut commit,
        &mut job,
        due,
        due,
        Some(staged),
    )
    .unwrap();
    drop(hold);
    assert!(fired.is_none(), "decided again on the next pass");
    assert!(commit.mutations.is_empty() && commit.events.is_empty());

    let error = ended_with(&fire(&runtime, job.id));
    assert!(
        error.contains("the model is outside your delegation policy"),
        "{error}"
    );
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn an_ended_creator_is_judged_under_the_hold_not_when_staged() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let due = job.status.next_due().unwrap();
    change(&runtime, lead.id, |lead| {
        lead.status = AgentStatus::Failed {
            error: "provider outage".into(),
        };
    });
    let staged = execution::stage_spawn(&runtime, &job, due);
    // Woken before the scheduler takes the hold: the schedule must survive.
    change(&runtime, lead.id, |lead| lead.status = AgentStatus::Ready);

    let hold = runtime.hold();
    let mut commit = Commit::new();
    let fired = execution::prepare_occurrence(
        &runtime,
        &hold,
        &mut commit,
        &mut job,
        due,
        due,
        Some(staged),
    )
    .unwrap();
    drop(hold);
    assert!(fired.is_none(), "staged again on the next pass");
    assert!(commit.mutations.is_empty() && commit.events.is_empty());
    assert!(spawned(&fire(&runtime, job.id)).is_some());
}

#[tokio::test]
async fn staging_leaves_a_full_max_parallel_to_the_wait_and_the_hold() {
    use crate::delegation_admission::{ChildReason, ChildRequest, Slots, policy_refusal};
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(at_most(1)));
    agent(&runtime, Some(lead.id));
    let request = ChildRequest {
        role: "poller".into(),
        name: None,
        objective: "Check the queue".into(),
        instructions: None,
        model: model(FAST),
        fallbacks: Vec::new(),
        effort: Effort::Low,
        scope: None,
        leaf: false,
        host_context: None,
        reason: ChildReason::Unsupported,
    };
    let verdict = |slots| policy_refusal(&runtime, &lead, &request, slots).unwrap();
    let counted = verdict(Slots::Counted).expect("`delegate` is refused at once");
    assert!(
        counted.contains("at most 1 unfinished children at once"),
        "{counted}"
    );
    assert_eq!(
        verdict(Slots::LeftToCaller),
        None,
        "a moving count must not end a schedule that waits for a slot"
    );
}
