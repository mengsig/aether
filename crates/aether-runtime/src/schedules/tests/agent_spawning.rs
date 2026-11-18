//! Spawn schedules an agent creates: bounded by its delegation policy and
//! scope, and subject to the overlap rule, limits and its own lifetime.

use super::spawning::{end_run, skip_reason, spawned};
use super::*;
use aether_core::agent::{AgentConfig, NetworkPolicy};
use aether_core::delegation::{DelegationPolicy, ModelTier, WorkerChoice};
use aether_core::model::Effort;

const FAST: (&str, &str) = ("anthropic", "claude-haiku-5-5");
const BALANCED: (&str, &str) = ("anthropic", "claude-sonnet-5-5");

fn model((provider, id): (&str, &str)) -> ModelRef {
    ModelRef::new(provider, id)
}

fn fast_only() -> DelegationPolicy {
    DelegationPolicy {
        workers: vec![WorkerChoice {
            model: None,
            tier: Some(ModelTier::Fast),
            effort: Some(Effort::Medium),
        }],
        ..DelegationPolicy::default()
    }
}

/// A balanced-tier lead, offline, with a $5 budget and `policy`.
fn creator(runtime: &Runtime, policy: Option<DelegationPolicy>) -> Agent {
    let mut lead = runtime
        .create_agent(
            CreateAgentSpec {
                role: "lead".into(),
                model: Some(model(BALANCED)),
                config: Some(AgentConfig {
                    cost_budget: Some(5.0),
                    delegation_policy: policy,
                    ..AgentConfig::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap()
        .0;
    lead.scope.network = NetworkPolicy::Denied;
    set_agent(runtime, lead.clone());
    lead
}

fn agent_template(chosen: (&str, &str), effort: Effort) -> SpawnSpec {
    SpawnSpec {
        role: "poller".into(),
        name: None,
        objective: "Check the queue".into(),
        model: Some(model(chosen)),
        fallbacks: Vec::new(),
        scope: None,
        config: Some(AgentConfig {
            effort: Some(effort),
            cost_budget: Some(50.0),
            max_turns: Some(3),
            ..AgentConfig::default()
        }),
        approval: None,
    }
}

fn agent_spec(template: SpawnSpec) -> CreateScheduleSpec {
    CreateScheduleSpec {
        target: ScheduleTarget::Spawn(template),
        max_deliveries: None,
        ..spec(AgentId::new())
    }
}

fn spawned_agents(runtime: &Runtime, lead: AgentId) -> Vec<Agent> {
    runtime
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()
        .into_iter()
        .filter(|agent| agent.id != lead)
        .collect()
}

fn set_agent(runtime: &Runtime, agent: Agent) {
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .unwrap();
}

#[tokio::test]
async fn an_agent_schedules_a_fast_tier_spawn_bounded_by_its_own_authority() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(fast_only()));
    let job = runtime
        .create_schedule(
            agent_spec(agent_template(FAST, Effort::Low)),
            Actor::agent(lead.id),
        )
        .unwrap();
    assert_eq!(job.owner, Actor::agent(lead.id));
    let ScheduleMode::Spawn { template, .. } = &job.mode else {
        panic!("a spawn schedule");
    };
    assert_eq!(
        template.scope.as_ref(),
        Some(&lead.scope),
        "the lead's scope"
    );
    assert_eq!(template.approval.as_ref(), Some(&lead.config.approval));
    let config = template.config.as_ref().unwrap();
    assert_eq!(config.cost_budget, Some(5.0), "held to the lead's budget");
    assert_eq!(config.delegation_policy, None);

    tick(&runtime, job.status.next_due().unwrap()).unwrap();
    let runs = spawned_agents(&runtime, lead.id);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!((run.parent, run.status.clone()), (None, AgentStatus::Ready));
    assert_ne!(run.run, lead.run, "each occurrence is its own root run");
    assert_eq!(run.model, model(FAST));
    assert_eq!(run.config.effort, Some(Effort::Low));
    assert_eq!(run.config.max_turns, Some(3));
    assert_eq!(run.config.cost_budget, Some(5.0));
    assert_eq!(run.scope.network, NetworkPolicy::Denied);
}

#[tokio::test]
async fn a_template_above_the_agents_policy_or_scope_is_refused() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(fast_only()));
    let unbounded = creator(&runtime, None);
    let exempting = creator(
        &runtime,
        Some(DelegationPolicy {
            exempt_roles: vec!["poller".into()],
            exempt_models: vec![model(("anthropic", "claude-fable-5-2"))],
            ..fast_only()
        }),
    );
    let refused = |template: SpawnSpec, by: AgentId| {
        runtime
            .create_schedule(agent_spec(template), Actor::agent(by))
            .expect_err("refused")
            .to_string()
    };
    let cases: [(SpawnSpec, AgentId, &str); 9] = [
        (
            agent_template(BALANCED, Effort::Low),
            lead.id,
            "the model is outside",
        ),
        (
            agent_template(FAST, Effort::High),
            lead.id,
            "effort high is above",
        ),
        (
            agent_template(("anthropic", "claude-fable-5-2"), Effort::Low),
            unbounded.id,
            "above your own balanced tier",
        ),
        (
            agent_template(("anthropic", "claude-fable-5-2"), Effort::Low),
            exempting.id,
            "the model is outside",
        ),
        (
            SpawnSpec {
                model: None,
                ..agent_template(FAST, Effort::Low)
            },
            lead.id,
            "must name its model",
        ),
        (
            SpawnSpec {
                config: None,
                ..agent_template(FAST, Effort::Low)
            },
            lead.id,
            "must set an effort",
        ),
        (
            SpawnSpec {
                fallbacks: vec![model(BALANCED)],
                ..agent_template(FAST, Effort::Low)
            },
            lead.id,
            "fallback",
        ),
        (
            SpawnSpec {
                scope: Some(aether_core::agent::Scope {
                    network: NetworkPolicy::Allowed,
                    ..lead.scope.clone()
                }),
                ..agent_template(FAST, Effort::Low)
            },
            lead.id,
            "widens your `network`",
        ),
        (
            {
                let mut template = agent_template(FAST, Effort::Low);
                template.config.as_mut().unwrap().delegation_policy =
                    Some(DelegationPolicy::default());
                template
            },
            lead.id,
            "delegation policy",
        ),
    ];
    for (template, by, expected) in cases {
        let error = refused(template, by);
        assert!(error.contains(expected), "{expected}: {error}");
    }
    let mut leaf = creator(&runtime, None);
    leaf.config.leaf = true;
    set_agent(&runtime, leaf.clone());
    let error = refused(agent_template(FAST, Effort::Low), leaf.id);
    assert!(error.contains("leaf"), "{error}");
    assert!(runtime.store.schedules(None).unwrap().is_empty());
}

/// An agent-owned schedule, fired once; returns it with the run it started.
async fn fired_once() -> (Arc<Runtime>, tempfile::TempDir, Schedule, Agent, AgentId) {
    let (runtime, dir) = fixture().await;
    let lead = creator(&runtime, Some(fast_only()));
    let job = runtime
        .create_schedule(
            agent_spec(agent_template(FAST, Effort::Low)),
            Actor::agent(lead.id),
        )
        .unwrap();
    tick(&runtime, job.status.next_due().unwrap()).unwrap();
    let first = spawned(&runtime.store.schedule(job.id).unwrap().unwrap()).unwrap();
    (runtime, dir, job, lead, first)
}

fn next_due(runtime: &Runtime, job: &Schedule) -> Timestamp {
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    stored.status.next_due().expect("still active")
}

#[tokio::test]
async fn an_agents_spawn_skips_while_its_previous_run_or_its_creator_is_held() {
    let (runtime, _dir, job, lead, first) = fired_once().await;
    tick(&runtime, next_due(&runtime, &job)).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let reason = skip_reason(&stored).unwrap();
    assert!(reason.contains("is still live"), "{reason}");
    assert_eq!(spawned_agents(&runtime, lead.id).len(), 1);

    end_run(&runtime, first);
    let mut paused = runtime.store.agent(lead.id).unwrap().unwrap();
    paused.paused = true;
    set_agent(&runtime, paused);
    tick(&runtime, next_due(&runtime, &job)).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert_eq!(
        skip_reason(&stored).as_deref(),
        Some(format!("its creator {} is paused", lead.id).as_str())
    );
    assert_eq!(
        (stored.deliveries, spawned_agents(&runtime, lead.id).len()),
        (1, 1)
    );
}

#[tokio::test]
async fn an_agents_spawn_schedule_fails_once_its_creator_finishes_or_narrows() {
    let (runtime, _dir, job, lead, first) = fired_once().await;
    end_run(&runtime, first);
    let mut narrowed = runtime.store.agent(lead.id).unwrap().unwrap();
    narrowed.config.delegation_policy = Some(DelegationPolicy {
        workers: vec![WorkerChoice {
            model: None,
            tier: Some(ModelTier::Balanced),
            effort: None,
        }],
        ..DelegationPolicy::default()
    });
    set_agent(&runtime, narrowed);
    tick(&runtime, next_due(&runtime, &job)).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let ScheduleStatus::Failed { error, .. } = &stored.status else {
        panic!("a narrowed policy fails the job: {:?}", stored.status);
    };
    assert!(error.contains("the model is outside"), "{error}");
    assert_eq!(spawned_agents(&runtime, lead.id).len(), 1);

    end_run(&runtime, lead.id);
    runtime
        .update_schedule(job.id, ScheduleAction::Resume, Actor::agent(lead.id))
        .unwrap();
    tick(&runtime, next_due(&runtime, &job)).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let ScheduleStatus::Failed { error, .. } = &stored.status else {
        panic!("a finished creator fails the job: {:?}", stored.status);
    };
    assert!(error.contains("its creator"), "{error}");
    assert_eq!(spawned_agents(&runtime, lead.id).len(), 1);
}

#[tokio::test]
async fn an_agent_pauses_and_cancels_its_spawn_schedule_and_the_limit_holds() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(fast_only()));
    let other = creator(&runtime, Some(fast_only()));
    let job = runtime
        .create_schedule(
            CreateScheduleSpec {
                max_deliveries: NonZeroU64::new(1),
                ..agent_spec(agent_template(FAST, Effort::Low))
            },
            Actor::agent(lead.id),
        )
        .unwrap();
    let stranger = runtime.update_schedule(job.id, ScheduleAction::Pause, Actor::agent(other.id));
    assert!(stranger.unwrap_err().to_string().contains("only the owner"));
    let paused = runtime
        .update_schedule(job.id, ScheduleAction::Pause, Actor::agent(lead.id))
        .unwrap();
    assert!(matches!(paused.status, ScheduleStatus::Paused { .. }));
    let resumed = runtime
        .update_schedule(job.id, ScheduleAction::Resume, Actor::agent(lead.id))
        .unwrap();
    tick(&runtime, resumed.status.next_due().unwrap()).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert!(matches!(stored.status, ScheduleStatus::Exhausted { .. }));
    assert_eq!(stored.deliveries, 1);
    let cancelled = runtime
        .update_schedule(job.id, ScheduleAction::Cancel, Actor::agent(lead.id))
        .unwrap();
    assert!(matches!(cancelled.status, ScheduleStatus::Cancelled { .. }));
}
