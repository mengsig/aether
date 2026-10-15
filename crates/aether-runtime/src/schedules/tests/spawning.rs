use super::*;

fn roots(runtime: &Runtime) -> Vec<Agent> {
    runtime
        .store
        .agents(&aether_store::AgentFilter::default())
        .unwrap()
}

fn spawned(job: &Schedule) -> Option<AgentId> {
    match &job.mode {
        ScheduleMode::Spawn { last_spawned, .. } => *last_spawned,
        ScheduleMode::Deliver { .. } => None,
    }
}

fn end_run(runtime: &Runtime, id: AgentId) {
    let mut root = runtime.store.agent(id).unwrap().unwrap();
    root.status = AgentStatus::Completed { summary: None };
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(root)))
        .unwrap();
}

#[tokio::test]
async fn spawn_occurrence_starts_one_budgeted_root_run_with_the_frozen_model() {
    let (runtime, _dir) = fixture().await;
    let job = runtime
        .create_schedule(spawn_spec(MissedRunPolicy::Coalesce), Actor::user())
        .unwrap();
    let ScheduleMode::Spawn { template, .. } = &job.mode else {
        panic!("spawn schedule");
    };
    assert_eq!(
        template.model,
        Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
        "the default model is frozen into the template at creation"
    );
    assert!(job.target().is_none() && roots(&runtime).is_empty());

    let due = job.status.next_due().unwrap();
    tick(&runtime, due).unwrap();
    // The same due time again, as after a restart that replays the pass.
    tick(&runtime, due).unwrap();
    let agents = roots(&runtime);
    assert_eq!(agents.len(), 1, "one run per due time");
    let root = &agents[0];
    assert_eq!(root.parent, None);
    assert_eq!(root.status, AgentStatus::Ready);
    assert_eq!(root.model, ModelRef::new("anthropic", "claude-sonnet-5-5"));
    assert_eq!(root.config.token_budget, Some(5_000));
    assert_eq!(root.config.max_turns, Some(4));
    let task = runtime
        .store
        .task(root.current_task.unwrap())
        .unwrap()
        .unwrap();
    assert!(task.objective.starts_with("Summarise overnight failures"));
    let line = task.objective.lines().last().unwrap();
    assert!(
        line.contains(&format!("`Read docs` ({})", job.id)),
        "{line}"
    );
    assert!(
        line.contains(r#"Payload: {"task":"Read the documentation"}"#),
        "{line}"
    );

    let history = runtime.store.schedule_history(job.id, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].outcome,
        ScheduleOutcome::Spawned {
            agent: root.id,
            task: task.id,
            run: root.run,
        }
    );
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert_eq!(stored.deliveries, 1);
    assert_eq!(spawned(&stored), Some(root.id));
    assert_eq!(stored.last.as_ref(), Some(&history[0]));
}

#[tokio::test]
async fn spawn_skips_while_its_previous_run_is_live_and_says_so() {
    let (runtime, _dir) = fixture().await;
    let job = runtime
        .create_schedule(spawn_spec(MissedRunPolicy::Coalesce), Actor::user())
        .unwrap();
    let first = job.status.next_due().unwrap();
    tick(&runtime, first).unwrap();
    let previous = spawned(&runtime.store.schedule(job.id).unwrap().unwrap()).unwrap();

    let second = job.recurrence.next_after(first).unwrap();
    tick(&runtime, second).unwrap();
    assert_eq!(roots(&runtime).len(), 1);
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    let run = runtime.store.agent(previous).unwrap().unwrap().run;
    let expected = format!("previous run {run} is still live: {previous} is ready");
    assert_eq!(skip_reason(&stored).as_deref(), Some(expected.as_str()));
    assert_eq!(stored.deliveries, 1);

    end_run(&runtime, previous);
    tick(&runtime, job.recurrence.next_after(second).unwrap()).unwrap();
    let stored = runtime.store.schedule(job.id).unwrap().unwrap();
    assert_eq!(roots(&runtime).len(), 2);
    assert_ne!(spawned(&stored), Some(previous));
    assert_eq!(stored.deliveries, 2);
}

fn skip_reason(job: &Schedule) -> Option<String> {
    match &job.last.as_ref()?.outcome {
        ScheduleOutcome::Skipped { reason } => Some(reason.clone()),
        _ => None,
    }
}

/// Fires the first occurrence, runs `between` on its root, fires the next and
/// returns the schedule as that second occurrence left it.
async fn second_occurrence_after(between: impl FnOnce(&Runtime, AgentId)) -> Schedule {
    let (runtime, _dir) = fixture().await;
    let job = runtime
        .create_schedule(spawn_spec(MissedRunPolicy::Coalesce), Actor::user())
        .unwrap();
    let first = job.status.next_due().unwrap();
    tick(&runtime, first).unwrap();
    let root = spawned(&runtime.store.schedule(job.id).unwrap().unwrap()).unwrap();
    between(&runtime, root);
    let before = roots(&runtime).len();
    tick(&runtime, job.recurrence.next_after(first).unwrap()).unwrap();
    assert_eq!(roots(&runtime).len(), before, "no second run started");
    runtime.store.schedule(job.id).unwrap().unwrap()
}

#[tokio::test]
async fn spawn_skips_while_a_restarted_root_of_the_previous_run_is_live() {
    let mut successor = None;
    let stored = second_occurrence_after(|runtime, root| {
        let mut failed = runtime.store.agent(root).unwrap().unwrap();
        failed.status = AgentStatus::Failed {
            error: "hard stop".into(),
        };
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(failed)))
            .unwrap();
        successor = Some(runtime.restart_agent(root, Actor::user()).unwrap().id);
    })
    .await;
    let reason = skip_reason(&stored).unwrap();
    assert!(
        reason.contains(&format!("{} is ", successor.unwrap())),
        "{reason}"
    );
    assert_eq!(stored.deliveries, 1);
}

#[tokio::test]
async fn spawn_skips_while_a_finished_root_still_has_live_children() {
    let mut child = None;
    let stored = second_occurrence_after(|runtime, root| {
        child = Some(agent(runtime, Some(root)).id);
        end_run(runtime, root);
    })
    .await;
    let reason = skip_reason(&stored).unwrap();
    assert!(
        reason.contains(&format!("{} is ", child.unwrap())),
        "{reason}"
    );
    assert_eq!(stored.deliveries, 1);
}

#[tokio::test]
async fn spawn_skip_names_a_paused_previous_run_as_paused() {
    let stored = second_occurrence_after(|runtime, root| {
        let mut held = runtime.store.agent(root).unwrap().unwrap();
        held.paused = true;
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(held)))
            .unwrap();
    })
    .await;
    let reason = skip_reason(&stored).unwrap();
    assert!(reason.ends_with(" is paused"), "{reason}");
}

#[tokio::test]
async fn spawn_missed_runs_follow_the_policy_and_the_limit_exhausts() {
    let (runtime, _dir) = fixture().await;
    let skip = runtime
        .create_schedule(spawn_spec(MissedRunPolicy::Skip), Actor::user())
        .unwrap();
    let mut coalesce = spawn_spec(MissedRunPolicy::Coalesce);
    coalesce.max_deliveries = NonZeroU64::new(1);
    let coalesce = runtime.create_schedule(coalesce, Actor::user()).unwrap();
    // Ten windows late: skip spawns nothing, coalesce spawns exactly one.
    let late = coalesce.status.next_due().unwrap() + chrono::TimeDelta::minutes(10);
    tick(&runtime, late).unwrap();
    let skipped = runtime.store.schedule_history(skip.id, 10).unwrap();
    assert!(
        matches!(&skipped[0].outcome, ScheduleOutcome::Skipped { reason } if reason.contains("policy"))
    );
    assert_eq!(roots(&runtime).len(), 1);
    let limited = runtime.store.schedule(coalesce.id).unwrap().unwrap();
    assert!(matches!(limited.status, ScheduleStatus::Exhausted { .. }));
    assert_eq!(limited.deliveries, 1);
}

#[tokio::test]
async fn spawn_templates_are_operator_only_and_checked_at_creation() {
    let (runtime, _dir) = fixture().await;
    let owner = agent(&runtime, None);
    let refused = |spec: CreateScheduleSpec, by: Actor| {
        runtime
            .create_schedule(spec, by)
            .expect_err("refused")
            .to_string()
    };
    assert!(
        refused(
            spawn_spec(MissedRunPolicy::Coalesce),
            Actor::agent(owner.id)
        )
        .contains("operator")
    );
    let mut queued = spawn_spec(MissedRunPolicy::Coalesce);
    queued.busy = BusyPolicy::QueueOne;
    assert!(refused(queued, Actor::user()).contains("queue_one"));
    for (patch, expected) in [
        (
            (|t: &mut SpawnSpec| t.config.as_mut().unwrap().long_lived = true)
                as fn(&mut SpawnSpec),
            "long-lived",
        ),
        (
            |t: &mut SpawnSpec| t.config.as_mut().unwrap().deadline = Some(now()),
            "deadline",
        ),
        (|t: &mut SpawnSpec| t.objective = "  ".into(), "objective"),
        (
            |t: &mut SpawnSpec| t.model = Some(ModelRef::new("nowhere", "none")),
            "nowhere",
        ),
        (
            |t: &mut SpawnSpec| t.config.as_mut().unwrap().token_budget = Some(0),
            "budget",
        ),
    ] {
        let mut template = template();
        patch(&mut template);
        let spec = CreateScheduleSpec {
            target: ScheduleTarget::Spawn(template),
            ..spawn_spec(MissedRunPolicy::Coalesce)
        };
        let error = refused(spec, Actor::user());
        assert!(error.contains(expected), "{expected}: {error}");
    }
    assert!(runtime.store.schedules(None).unwrap().is_empty());
    assert_eq!(roots(&runtime).len(), 1, "the dry run writes nothing");
}

#[tokio::test]
async fn a_retried_spawn_schedule_is_created_once() {
    let (runtime, _dir) = fixture().await;
    let key = Submission::new("spawn-1", &serde_json::json!({"cmd": "create_schedule"})).unwrap();
    let first = runtime
        .create_schedule_keyed(
            spawn_spec(MissedRunPolicy::Coalesce),
            Actor::user(),
            Some(key.clone()),
        )
        .unwrap();
    let again = runtime
        .create_schedule_keyed(
            spawn_spec(MissedRunPolicy::Coalesce),
            Actor::user(),
            Some(key),
        )
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(runtime.store.schedules(None).unwrap().len(), 1);
}

#[tokio::test]
async fn failed_spawn_commit_leaves_no_run_behind() {
    let (runtime, _dir) = fixture().await;
    let job = runtime
        .create_schedule(spawn_spec(MissedRunPolicy::Coalesce), Actor::user())
        .unwrap();
    let due = job.status.next_due().unwrap();
    runtime
        .apply(
            Commit::new().mutate(Mutation::RecordScheduleOccurrence(ScheduleOccurrence {
                schedule: job.id,
                due,
                recorded_at: due,
                outcome: ScheduleOutcome::Skipped {
                    reason: "injected conflict".into(),
                },
            })),
        )
        .unwrap();
    let before = runtime.store.last_seq().unwrap();
    assert!(matches!(tick(&runtime, due), Err(RuntimeError::Store(_))));
    assert!(
        roots(&runtime).is_empty(),
        "the run commits with its occurrence or not at all"
    );
    assert!(
        runtime
            .store
            .tasks(&aether_store::TaskFilter::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(runtime.store.schedule(job.id).unwrap().unwrap(), job);
    assert_eq!(runtime.store.last_seq().unwrap(), before);
}

#[tokio::test]
async fn prepared_schedule_spawn_rechecks_workspace_gc_admission() {
    for reserved in [true, false] {
        let (runtime, dir) = fixture().await;
        let workspace = dir.path().join("managed/collected");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut spec = spawn_spec(MissedRunPolicy::Coalesce);
        let ScheduleTarget::Spawn(template) = &mut spec.target else {
            panic!("spawn");
        };
        template.scope = Some(aether_core::agent::Scope {
            workspace_root: Some(workspace.clone()),
            ..Default::default()
        });
        let mut job = runtime.create_schedule(spec, Actor::user()).unwrap();
        let due = job.status.next_due().unwrap();
        let prepared = super::super::execution::prepare_spawn(&runtime, &job, due).unwrap();
        if reserved {
            *runtime.workspace_gc_active.lock() = Some(workspace);
        } else {
            std::fs::remove_dir(&workspace).unwrap();
        }
        let hold = runtime.hold();
        let mut commit = Commit::new();
        let result = super::super::execution::prepare_occurrence(
            &runtime,
            &hold,
            &mut commit,
            &mut job,
            due,
            due,
            Some(Ok(prepared)),
        );
        if reserved {
            assert!(matches!(result.unwrap().0, ScheduleOutcome::Skipped { .. }));
        } else {
            assert!(result.unwrap_err().to_string().contains("disappeared"));
        }
        assert!(commit.mutations.is_empty());
        assert!(roots(&runtime).is_empty());
    }
}
