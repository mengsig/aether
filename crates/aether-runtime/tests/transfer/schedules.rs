use super::*;

#[tokio::test]
async fn schedules_and_history_travel_without_reactivating_archived_work() {
    use aether_core::id::ScheduleId;
    use aether_core::schedule::{
        BusyPolicy, MissedRunPolicy, Recurrence, Schedule, ScheduleMode, ScheduleOccurrence,
        ScheduleOutcome, ScheduleStatus,
    };
    use aether_store::{Commit, Mutation};
    let (source, run) = finished_run().await;
    let agent = source
        .store
        .agents(&AgentFilter {
            run: Some(run),
            ..Default::default()
        })
        .unwrap()
        .remove(0);
    let at = now();
    let schedule = Schedule {
        id: ScheduleId::new(),
        name: "Documentation timer".into(),
        owner: Actor::Agent { id: agent.id },
        mode: ScheduleMode::Deliver {
            target: agent.id,
            run,
            last_inbox_seq: None,
        },
        recurrence: Recurrence::Interval {
            seconds: 60.try_into().unwrap(),
        },
        missed: MissedRunPolicy::Coalesce,
        busy: BusyPolicy::Skip,
        payload: json!("Read docs"),
        status: ScheduleStatus::Active {
            next_due: at + chrono::TimeDelta::hours(1),
        },
        max_deliveries: None,
        deliveries: 0,
        last: None,
        created_at: at,
        updated_at: at,
    };
    let history = ScheduleOccurrence {
        schedule: schedule.id,
        due: at,
        recorded_at: at,
        outcome: ScheduleOutcome::Skipped {
            reason: "target was busy".into(),
        },
    };
    let mut commit = Commit::new();
    commit.push_mutation(Mutation::UpsertSchedule(schedule.clone()));
    commit.push_mutation(Mutation::RecordScheduleOccurrence(history.clone()));
    source.store.commit(commit).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("timers.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).unwrap();
    let jobs = target.store.schedules(None).unwrap();
    assert_eq!(jobs.len(), 1, "the timer must travel with its run");
    let job = &jobs[0];
    assert_ne!(job.id, schedule.id);
    assert_eq!(job.run(), Some(imported.run));
    assert_ne!(job.target(), schedule.target());
    let target_id = job.target().expect("delivery schedule");
    assert_eq!(job.owner, Actor::Agent { id: target_id }.imported());
    assert!(matches!(job.status, ScheduleStatus::Cancelled { .. }));
    assert!(
        target
            .store
            .due_schedules(at + chrono::TimeDelta::days(1), 100)
            .unwrap()
            .is_empty()
    );
    let mut imported_history = history;
    imported_history.schedule = job.id;
    assert_eq!(
        target.store.schedule_history(job.id, 100).unwrap(),
        vec![imported_history]
    );

    let original = std::fs::read_to_string(&file).unwrap();
    let before_agents = target.store.agents(&AgentFilter::default()).unwrap().len();
    let before_events = target
        .store
        .events(0, 100_000, &EventFilter::default())
        .unwrap()
        .len();
    for (case, expected) in [
        ("missing_schedule", "no schedule record"),
        ("foreign_target", "agent absent"),
        ("foreign_run", "manifest names"),
        ("duplicate_schedule", "duplicate schedule"),
        ("duplicate_occurrence", "duplicate schedule occurrence"),
    ] {
        let mut lines = Vec::new();
        for line in original.lines() {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            let kind = value
                .get("record")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if kind == "schedule" {
                match case {
                    "missing_schedule" => continue,
                    "foreign_target" => value["target"] = json!(AgentId::new()),
                    "foreign_run" => value["run"] = json!(RunId::new()),
                    "duplicate_schedule" => lines.push(value.to_string()),
                    _ => {}
                }
            }
            if kind == "schedule_occurrence" && case == "duplicate_occurrence" {
                lines.push(value.to_string());
            }
            lines.push(value.to_string());
        }
        std::fs::write(&file, lines.join("\n")).unwrap();
        let error = transfer::import_run(&target.runtime, &file).unwrap_err();
        assert!(error.to_string().contains(expected), "{case}: {error}");
        assert_eq!(target.store.schedules(None).unwrap(), jobs, "{case}");
        assert_eq!(
            target.store.agents(&AgentFilter::default()).unwrap().len(),
            before_agents,
            "{case}"
        );
        assert_eq!(
            target
                .store
                .events(0, 100_000, &EventFilter::default())
                .unwrap()
                .len(),
            before_events,
            "{case}"
        );
    }
}
