//! An agent's spawn schedule against a creator that moves: whatever was
//! decided before the hold is decided again under it, as `delegate` decides.

use super::child_spawning::{
    BALANCED, FAST, FRONTIER, at_most, change, child_spec, child_template, children, creator,
    creator_with, ended_with, fast_only, fire, model, refused, schedule,
};
use super::spawning::{skip_reason, spawned};
use super::*;
use crate::schedules::execution::{self, Fired, StagedSpawn};
use aether_core::agent::{AgentConfig, ApprovalPolicy, NetworkPolicy, Scope};
use aether_core::delegation::{DelegationPolicy, WorkerChoice};
use aether_core::model::Effort;
use aether_core::tool::SideEffectClass;
use aether_engine::{EngineError, EngineKind};

const OTHER_FRONTIER: (&str, &str) = ("anthropic", "claude-fable-5-1");
const ENGINE_FAST: (&str, &str) = ("claude-code", "haiku");
const KIND: EngineKind = EngineKind::CLAUDE_CODE;

/// An engine that cannot run a child without network access.
struct NeedsNetwork;

#[async_trait::async_trait]
impl aether_engine::Engine for NeedsNetwork {
    fn kind(&self) -> EngineKind {
        KIND
    }

    fn adapter(&self) -> String {
        "needs-network".into()
    }

    fn validate_network_policy(
        &self,
        network_access: bool,
    ) -> std::result::Result<(), EngineError> {
        if network_access {
            return Ok(());
        }
        Err(EngineError::Unsupported("it cannot run offline".into()))
    }

    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<aether_engine::EngineRun, EngineError> {
        Err(EngineError::Unsupported("a test engine".into()))
    }
}

fn stage(runtime: &Runtime, job: &Schedule) -> StagedSpawn {
    execution::stage_spawn(runtime, job, job.status.next_due().expect("active"))
}

/// Decide `job`'s due occurrence under the hold from what was staged before
/// it, and return what it would write.
fn decide(
    runtime: &Runtime,
    job: &mut Schedule,
    staged: StagedSpawn,
) -> (crate::Result<Option<Fired>>, Commit) {
    let due = job.status.next_due().expect("active");
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let fired =
        execution::prepare_occurrence(runtime, &hold, &mut commit, job, due, due, Some(staged));
    (fired, commit)
}

/// The refusal creation gets under its hold for a template checked before it.
fn held_creation_refusal(
    runtime: &Runtime,
    lead: &Agent,
    template: SpawnSpec,
    between: impl FnOnce(),
) -> String {
    let by = Actor::agent(lead.id);
    let (template, child) = runtime
        .checked_template(template, &by, BusyPolicy::Skip)
        .expect("admitted before the hold");
    between();
    let before = runtime.store.last_seq().unwrap();
    let hold = runtime.hold();
    let error = runtime
        .initial_mode(&hold, ScheduleTarget::Spawn(template), child, &by)
        .expect_err("refused under the hold")
        .to_string();
    drop(hold);
    assert_eq!(runtime.store.last_seq().unwrap(), before);
    error
}

fn fail(runtime: &Runtime, id: AgentId) {
    change(runtime, id, |agent| {
        agent.status = AgentStatus::Failed {
            error: "provider outage".into(),
        };
    });
}

#[tokio::test]
async fn creation_is_refused_when_its_creators_tier_changed_before_the_hold() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let template = child_template(BALANCED, Effort::Low);
    let error = held_creation_refusal(&runtime, &lead, template.clone(), || {
        change(&runtime, lead.id, |lead| lead.model = model(FAST));
    });
    assert!(
        error.contains("changed while this schedule was being created"),
        "{error}"
    );
    let error = refused(&runtime, &lead, template);
    assert!(error.contains("above your own fast tier"), "{error}");
}

#[tokio::test]
async fn budget_spent_after_preparation_is_refused_under_the_hold() {
    let (runtime, _dir) = fixture_with_run_budget(Some(1.0)).await;
    let lead = creator(&runtime, None);
    let mut job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let staged = stage(&runtime, &job);
    let spend = || {
        change(&runtime, lead.id, |lead| lead.stats.usage.cost_usd = 2.0);
    };
    let error = held_creation_refusal(&runtime, &lead, child_template(FAST, Effort::Low), spend);
    assert!(error.contains("`[scheduler] run_budget_usd`"), "{error}");

    let (fired, commit) = decide(&runtime, &mut job, staged);
    let error = fired.expect_err("refused under the hold").to_string();
    assert!(error.contains("`[scheduler] run_budget_usd`"), "{error}");
    assert!(commit.mutations.is_empty() && children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_missed_occurrence_does_not_skip_past_an_ended_creator() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut spec = child_spec(child_template(FAST, Effort::Low));
    spec.missed = MissedRunPolicy::Skip;
    let job = runtime
        .create_schedule(spec, Actor::agent(lead.id))
        .unwrap();
    let late = job.status.next_due().unwrap() + chrono::Duration::minutes(10);

    tick(&runtime, late).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let reason = skip_reason(&stored).expect("a live creator's missed run is skipped");
    assert!(reason.contains("missed occurrences skipped"), "{reason}");
    assert!(matches!(stored.status, ScheduleStatus::Active { .. }));

    fail(&runtime, lead.id);
    tick(&runtime, late + chrono::Duration::minutes(10)).unwrap();
    let error = ended_with(&runtime.store.schedule(job.id).unwrap().unwrap());
    assert!(error.contains("ends with the agent"), "{error}");
}

#[tokio::test]
async fn resuming_an_active_schedule_is_refused_once_its_creator_ended() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let resume = || runtime.update_schedule(job.id, ScheduleAction::Resume, Actor::user());
    assert_eq!(resume().expect("idempotent while it lives"), job);

    fail(&runtime, lead.id);
    let error = resume().expect_err("its creator ended").to_string();
    assert!(error.contains("ends with the agent"), "{error}");
}

#[tokio::test]
async fn a_refusal_staged_against_a_row_that_has_moved_is_decided_again() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let mut job = schedule(&runtime, &lead, child_template(BALANCED, Effort::Low));
    change(&runtime, lead.id, |lead| {
        lead.config.delegation_policy = Some(fast_only());
    });
    let staged = stage(&runtime, &job);
    // The row moves on, as a creator that failed and was woken would have.
    change(&runtime, lead.id, |lead| {
        lead.updated_at += chrono::Duration::seconds(1);
    });

    let (fired, commit) = decide(&runtime, &mut job, staged);
    assert!(fired.unwrap().is_none(), "staged again on the next pass");
    assert!(commit.mutations.is_empty() && commit.events.is_empty());
    let error = ended_with(&fire(&runtime, job.id));
    assert!(
        error.contains("the model is outside your delegation policy"),
        "{error}"
    );
}

#[tokio::test]
async fn a_cancellation_in_progress_ends_the_schedule_ahead_of_any_wait() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    change(&runtime, lead.id, |lead| lead.paused = true);
    crate::cancellation::request(&runtime, lead.id, None, Actor::user()).unwrap();
    assert!(runtime.store.agent(lead.id).unwrap().unwrap().is_live());

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("subtree is being cancelled"), "{error}");
}

#[tokio::test]
async fn an_engine_that_cannot_run_under_the_childs_network_refuses_it_then_or_later() {
    let (runtime, _dir) = fixture().await;
    runtime.engines.mark_usable(KIND, Arc::new(NeedsNetwork));
    let lead = creator(&runtime, None);
    let template = child_template(ENGINE_FAST, Effort::Low);
    let job = schedule(&runtime, &lead, template.clone());
    change(&runtime, lead.id, |lead| {
        lead.scope.network = NetworkPolicy::Denied;
    });

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("it cannot run offline"), "{error}");
    let error = refused(&runtime, &lead, template);
    assert!(error.contains("it cannot run offline"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn an_occurrence_waits_while_its_engine_is_unavailable() {
    let (runtime, _dir) = fixture().await;
    runtime.engines.mark_usable(KIND, Arc::new(NeedsNetwork));
    let lead = creator(&runtime, None);
    let template = child_template(ENGINE_FAST, Effort::Low);
    let job = schedule(&runtime, &lead, template.clone());
    runtime
        .engines
        .mark_unavailable(KIND, "claude --version exited 1");

    let stored = fire(&runtime, job.id);
    let reason = skip_reason(&stored).expect("waits for the engine");
    assert!(reason.contains("its engine is not available"), "{reason}");
    assert!(matches!(stored.status, ScheduleStatus::Active { .. }));
    assert_eq!(stored.deliveries, 0);
    let error = refused(&runtime, &lead, template);
    assert!(error.contains("its engine is not available"), "{error}");

    runtime.engines.mark_usable(KIND, Arc::new(NeedsNetwork));
    assert!(spawned(&fire(&runtime, job.id)).is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn a_template_path_that_resolves_outside_its_creators_roots_is_refused() {
    let (runtime, dir) = fixture().await;
    let workspace = dir.path().join("workspace");
    let (inside, outside) = (workspace.join("sub"), dir.path().join("outside"));
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let lead = creator(&runtime, None);
    let lead = change(&runtime, lead.id, |lead| {
        lead.scope.workspace_root = Some(workspace.clone());
    });
    let narrowed = |scope: Scope| SpawnSpec {
        scope: Some(scope),
        ..child_template(FAST, Effort::Low)
    };
    let rooted = narrowed(Scope {
        workspace_root: Some(inside.clone()),
        ..lead.scope.clone()
    });
    let allowed = narrowed(Scope {
        allowed_paths: vec![inside.clone()],
        ..lead.scope.clone()
    });
    let job = schedule(&runtime, &lead, rooted.clone());
    schedule(&runtime, &lead, allowed.clone());
    // The same lexical path, now the way out of the creator's workspace.
    std::fs::remove_dir(&inside).unwrap();
    std::os::unix::fs::symlink(&outside, &inside).unwrap();

    let widens = |field: &str| format!("`{field}` widens it");
    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains(&widens("workspace_root")), "{error}");
    let error = refused(&runtime, &lead, rooted);
    assert!(error.contains(&widens("workspace_root")), "{error}");
    let error = refused(&runtime, &lead, allowed);
    assert!(error.contains(&widens("allowed_paths")), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_slot_taken_after_staging_makes_the_held_decision_wait() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, Some(at_most(1)));
    let mut job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let staged = stage(&runtime, &job);
    // Staged with the slot free; another child has it by the hold.
    agent(&runtime, Some(lead.id));

    let (fired, commit) = decide(&runtime, &mut job, staged);
    let (outcome, _) = fired.unwrap().expect("a wait is recorded");
    assert!(
        matches!(&outcome, ScheduleOutcome::Skipped { reason }
            if reason.contains("at most 1 unfinished children at once")),
        "{outcome:?}"
    );
    assert_eq!(job.deliveries, 0, "a wait consumes no delivery");
    assert!(commit.mutations.is_empty());
    assert_eq!(children(&runtime, lead.id).len(), 1);
}

#[tokio::test]
async fn a_restart_of_the_previous_child_still_holds_the_next_occurrence() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let first = spawned(&fire(&runtime, job.id)).expect("a child");
    let successor = agent(&runtime, Some(lead.id));
    change(&runtime, first, |first| {
        first.status = AgentStatus::Failed {
            error: "provider outage".into(),
        };
        first.restarted_as = Some(successor.id);
    });

    let stored = fire(&runtime, job.id);
    let reason = skip_reason(&stored).expect("the restart is the previous child");
    assert!(reason.contains("is still live"), "{reason}");
    assert_eq!(stored.deliveries, 1);

    fail(&runtime, successor.id);
    let second = spawned(&fire(&runtime, job.id)).expect("the next child");
    assert_ne!(second, first);
}

#[tokio::test]
async fn an_effort_cap_tightened_after_creation_ends_the_schedule() {
    let (runtime, _dir) = fixture().await;
    let lead = creator(&runtime, None);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::High));
    change(&runtime, lead.id, |lead| {
        lead.config.delegation_policy = Some(DelegationPolicy {
            max_effort: Some(Effort::Low),
            ..DelegationPolicy::default()
        });
    });

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("delegation refused"), "{error}");
    assert!(error.contains("effort"), "{error}");
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn an_exempt_role_fires_with_the_slot_full_as_delegate_would_admit_it() {
    let (runtime, _dir) = fixture().await;
    let policy = DelegationPolicy {
        exempt_roles: vec!["poller".into()],
        ..at_most(1)
    };
    let lead = creator(&runtime, Some(policy));
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    agent(&runtime, Some(lead.id));

    let child = spawned(&fire(&runtime, job.id)).expect("exempt from max_parallel");
    assert_eq!(children(&runtime, lead.id).len(), 2);
    assert_eq!(
        runtime.store.agent(child).unwrap().unwrap().role.as_str(),
        "poller"
    );
}

#[tokio::test]
async fn a_policy_and_a_template_may_name_one_model_by_different_names() {
    let (runtime, _dir) = fixture().await;
    let naming = |name: &str| DelegationPolicy {
        workers: vec![WorkerChoice {
            model: Some(ModelRef::new("anthropic", name)),
            tier: None,
            effort: None,
        }],
        ..DelegationPolicy::default()
    };
    for (policy, template) in [
        ("sonnet", "claude-sonnet-5-5"),
        ("claude-sonnet-5-5", "sonnet"),
    ] {
        let lead = creator(&runtime, Some(naming(policy)));
        let template = child_template(("anthropic", template), Effort::Low);
        let job = schedule(&runtime, &lead, template);
        let child = spawned(&fire(&runtime, job.id)).expect("the policy names this model");
        let child = runtime.store.agent(child).unwrap().unwrap();
        assert_eq!(runtime.models.canonical(&child.model), model(BALANCED));
    }
}

#[tokio::test]
async fn a_creator_that_moves_onto_the_templates_frontier_model_gets_no_child() {
    let (runtime, _dir) = fixture().await;
    let lead = creator_with(&runtime, OTHER_FRONTIER, AgentConfig::default());
    let job = schedule(&runtime, &lead, child_template(FRONTIER, Effort::Low));
    change(&runtime, lead.id, |lead| lead.model = model(FRONTIER));

    let error = ended_with(&fire(&runtime, job.id));
    assert!(error.contains("is your own frontier-tier model"), "{error}");
    assert!(
        error.contains("a spawn schedule cannot carry the `reason`"),
        "{error}"
    );
    assert!(children(&runtime, lead.id).is_empty());
}

#[tokio::test]
async fn a_scheduled_child_inherits_its_creators_approval_policy_and_token_budget() {
    let (runtime, _dir) = fixture().await;
    let approval = ApprovalPolicy {
        require_for: [SideEffectClass::External].into(),
    };
    let config = AgentConfig {
        token_budget: Some(1_000_000),
        ..AgentConfig::default()
    };
    let lead = creator_with(&runtime, BALANCED, config);
    let job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    // Tightened after creation: the child gets what its creator has by then.
    change(&runtime, lead.id, |lead| {
        lead.config.approval = approval.clone();
    });

    let child = spawned(&fire(&runtime, job.id)).expect("a child");
    let child = runtime.store.agent(child).unwrap().unwrap();
    assert_eq!(child.config.approval, approval);
    assert_eq!(child.config.token_budget, Some(1_000_000));
}

#[tokio::test]
async fn a_scheduled_child_waits_out_maintenance_of_its_workspace() {
    let (runtime, dir) = fixture().await;
    let workspace = dir.path().join("managed/collected");
    std::fs::create_dir_all(&workspace).unwrap();
    let lead = creator(&runtime, None);
    change(&runtime, lead.id, |lead| {
        lead.scope.workspace_root = Some(workspace.clone());
    });
    let mut job = schedule(&runtime, &lead, child_template(FAST, Effort::Low));
    let staged = stage(&runtime, &job);
    *runtime.workspace_gc_active.lock() = Some(workspace);

    let (fired, commit) = decide(&runtime, &mut job, staged);
    let (outcome, _) = fired.unwrap().expect("a wait is recorded");
    assert!(
        matches!(&outcome, ScheduleOutcome::Skipped { reason }
            if reason.contains("workspace maintenance in progress")),
        "{outcome:?}"
    );
    assert!(commit.mutations.is_empty() && children(&runtime, lead.id).is_empty());
}
