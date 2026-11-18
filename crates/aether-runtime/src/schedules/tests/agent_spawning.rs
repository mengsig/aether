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

/// A balanced-tier lead, offline, unbudgeted, with `policy`.
fn creator(runtime: &Runtime, policy: Option<DelegationPolicy>) -> Agent {
    creator_under(runtime, policy, None, None)
}

fn creator_under(
    runtime: &Runtime,
    policy: Option<DelegationPolicy>,
    parent: Option<AgentId>,
    cost_budget: Option<f64>,
) -> Agent {
    let mut lead = runtime
        .create_agent(
            CreateAgentSpec {
                role: "lead".into(),
                model: Some(model(BALANCED)),
                parent,
                config: Some(AgentConfig {
                    effort: Some(Effort::Medium),
                    cost_budget,
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
    assert_eq!(config.cost_budget, Some(50.0), "the template's own budget");
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
    assert_eq!(run.config.cost_budget, Some(50.0));
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
    let budgeted = creator_under(&runtime, Some(fast_only()), None, Some(5.0));
    let error = refused(agent_template(FAST, Effort::Low), budgeted.id);
    assert!(error.contains("cost or token budget"), "{error}");
    let under_budget = creator_under(&runtime, Some(fast_only()), Some(budgeted.id), None);
    let error = refused(agent_template(FAST, Effort::Low), under_budget.id);
    assert!(
        error.contains(&budgeted.id.to_string()),
        "an ancestor's budget: {error}"
    );
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

#[tokio::test]
async fn a_link_inside_the_creators_workspace_cannot_reach_outside_it() {
    let (runtime, dir) = fixture().await;
    let (inside, outside) = (dir.path().join("inside"), dir.path().join("outside"));
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, inside.join("link")).unwrap();
    let mut lead = creator(&runtime, Some(fast_only()));
    lead.scope.workspace_root = Some(inside.clone());
    set_agent(&runtime, lead.clone());
    let template = SpawnSpec {
        scope: Some(aether_core::agent::Scope {
            workspace_root: Some(inside.join("link")),
            ..lead.scope.clone()
        }),
        ..agent_template(FAST, Effort::Low)
    };
    let error = runtime
        .create_schedule(agent_spec(template), Actor::agent(lead.id))
        .expect_err("the link resolves outside the lead's workspace")
        .to_string();
    assert!(error.contains("widens your `workspace_root`"), "{error}");
    assert!(runtime.store.schedules(None).unwrap().is_empty());
}

#[tokio::test]
async fn an_agents_spawn_schedule_fails_when_its_creator_finishes_mid_run_or_paused() {
    for paused in [false, true] {
        let (runtime, _dir, job, lead, first) = fired_once().await;
        let mut finished = runtime.store.agent(lead.id).unwrap().unwrap();
        finished.paused = paused;
        finished.status = AgentStatus::Completed { summary: None };
        set_agent(&runtime, finished);
        assert!(runtime.store.agent(first).unwrap().unwrap().is_live());
        tick(&runtime, next_due(&runtime, &job)).unwrap();
        let stored = runtime.store.schedule(job.id).unwrap().unwrap();
        let ScheduleStatus::Failed { error, .. } = &stored.status else {
            panic!("paused={paused}: not skipped: {:?}", stored.last);
        };
        assert!(error.contains("its creator"), "{error}");
    }
}

#[tokio::test]
async fn an_agents_spawn_reads_its_creator_under_the_commit_hold() {
    let (runtime, _dir, job, lead, first) = fired_once().await;
    end_run(&runtime, first);
    let mut job = runtime.store.schedule(job.id).unwrap().unwrap();
    let due = job.status.next_due().unwrap();
    // Narrowed after the scheduler read the job: the occurrence must see it.
    let mut narrowed = runtime.store.agent(lead.id).unwrap().unwrap();
    narrowed.config.delegation_policy = Some(DelegationPolicy {
        max_effort: Some(Effort::Low),
        workers: vec![WorkerChoice {
            model: Some(model(BALANCED)),
            tier: None,
            effort: None,
        }],
        ..DelegationPolicy::default()
    });
    set_agent(&runtime, narrowed);
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let refused = super::super::execution::prepare_occurrence(
        &runtime,
        &hold,
        &mut commit,
        &mut job,
        due,
        due,
        None,
    );
    let Err(error) = refused else {
        panic!("refused under the hold");
    };
    let error = error.to_string();
    assert!(error.contains("the model is outside"), "{error}");
    assert!(commit.mutations.is_empty());
}

#[tokio::test]
async fn an_agents_spawn_is_refused_while_its_creator_is_being_cancelled() {
    let (runtime, _dir, job, lead, first) = fired_once().await;
    end_run(&runtime, first);
    crate::cancellation::request(&runtime, lead.id, None, Actor::user()).unwrap();
    tick(&runtime, next_due(&runtime, &job)).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let ScheduleStatus::Failed { error, .. } = &stored.status else {
        panic!("no run under a pending cancellation: {:?}", stored.status);
    };
    assert!(error.contains("being cancelled"), "{error}");
    assert_eq!(spawned_agents(&runtime, lead.id).len(), 1);
}

/// An engine that cannot ask before acting, as legacy Codex exec.
struct Ungated;

#[async_trait::async_trait]
impl aether_engine::Engine for Ungated {
    fn kind(&self) -> aether_engine::EngineKind {
        aether_engine::EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "ungated/1".into()
    }

    fn gates_permissions(&self) -> bool {
        false
    }

    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<aether_engine::EngineRun, aether_engine::EngineError> {
        Err(aether_engine::EngineError::Unsupported("ungated".into()))
    }
}

#[tokio::test]
async fn a_template_whose_engine_would_drop_the_creators_gates_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let engines: HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>> =
        HashMap::from([(
            aether_engine::EngineKind::CLAUDE_CODE,
            Arc::new(Ungated) as Arc<dyn aether_engine::Engine>,
        )]);
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(model(BALANCED)),
            ..Default::default()
        },
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        Arc::new(Models::load(Some(dir.path().to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(vec![]))),
        engines,
    )
    .await
    .unwrap();
    runtime.shutdown.cancel();
    let mut lead = creator(&runtime, Some(fast_only()));
    lead.config.approval = aether_core::agent::ApprovalPolicy::default();
    set_agent(&runtime, lead.clone());
    assert!(!lead.config.approval.is_yolo(), "the lead is gated");
    let ungated = model(("claude-code", "claude-haiku-5-5"));
    assert!(!runtime.engine_can_gate(&ungated).unwrap());
    let template = SpawnSpec {
        model: Some(ungated),
        ..agent_template(FAST, Effort::Low)
    };
    let error = runtime
        .create_schedule(agent_spec(template), Actor::agent(lead.id))
        .expect_err("the run would be ungated")
        .to_string();
    assert!(error.contains("cannot ask before acting"), "{error}");
    assert!(runtime.store.schedules(None).unwrap().is_empty());
}
