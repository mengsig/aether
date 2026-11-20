use std::num::NonZeroU64;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, RunId, ScheduleId};
use aether_core::schedule::{
    BusyPolicy, MissedRunPolicy, Recurrence, Schedule, ScheduleMode, ScheduleOccurrence,
    ScheduleOutcome, ScheduleStatus, SpawnSpec,
};
use aether_core::time::Timestamp;

use crate::{Commit, Mutation, SqliteStore, Store};

fn at(text: &str) -> Timestamp {
    text.parse().expect("timestamp")
}

fn job() -> Schedule {
    let due = at("2026-09-09T12:00:00Z");
    Schedule {
        id: ScheduleId::new(),
        name: "Read the docs".into(),
        owner: Actor::user(),
        mode: ScheduleMode::Deliver {
            target: AgentId::new(),
            run: RunId::new(),
            last_inbox_seq: None,
        },
        recurrence: Recurrence::Interval {
            seconds: NonZeroU64::new(60).expect("positive"),
        },
        missed: MissedRunPolicy::Coalesce,
        busy: BusyPolicy::Skip,
        payload: serde_json::json!({"task": "Read the docs"}),
        status: ScheduleStatus::Active { next_due: due },
        max_deliveries: NonZeroU64::new(3),
        deliveries: 0,
        last: None,
        created_at: due,
        updated_at: due,
    }
}

#[test]
fn schedule_and_occurrence_survive_reopen_and_duplicate_history_rolls_back() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("schedule.db");
    let job = job();
    let occurrence = ScheduleOccurrence {
        schedule: job.id,
        due: job.created_at,
        recorded_at: job.created_at,
        outcome: ScheduleOutcome::Skipped {
            reason: "missed run".into(),
        },
    };
    let store = SqliteStore::open(&path).expect("open");
    store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertSchedule(job.clone()))
                .mutate(Mutation::RecordScheduleOccurrence(occurrence.clone())),
        )
        .expect("commit");
    drop(store);
    let store = SqliteStore::open(&path).expect("reopen");
    assert_eq!(store.schedule(job.id).expect("job"), Some(job.clone()));
    assert_eq!(
        store.schedule_history(job.id, 10).expect("history"),
        vec![occurrence.clone()]
    );
    let before = store.last_seq().expect("seq");
    let mut changed = job.clone();
    changed.status = ScheduleStatus::Cancelled { at: job.created_at };
    let duplicate = Commit::new()
        .event(EventDraft::new(
            Actor::Scheduler,
            EventKind::Custom {
                name: "schedule.test".into(),
                payload: serde_json::Value::Null,
            },
        ))
        .mutate(Mutation::UpsertSchedule(changed))
        .mutate(Mutation::RecordScheduleOccurrence(occurrence));
    assert!(store.commit(duplicate).is_err());
    assert_eq!(store.schedule(job.id).expect("unchanged"), Some(job));
    assert_eq!(store.last_seq().expect("unchanged seq"), before);
}

#[test]
fn due_queue_orders_subseconds_and_excludes_paused_jobs() {
    let store = SqliteStore::open_in_memory().expect("store");
    let first = job();
    let mut later = job();
    later.status = ScheduleStatus::Active {
        next_due: at("2026-09-09T12:00:00.001Z"),
    };
    let mut paused = job();
    paused.status = ScheduleStatus::Paused {
        next_due: at("2026-09-09T11:00:00Z"),
    };
    // Spawn schedules have no target: due and listed, never matched by one.
    let mut spawn = job();
    let creator = AgentId::new();
    spawn.owner = Actor::agent(creator);
    spawn.mode = ScheduleMode::Spawn {
        template: SpawnSpec {
            role: "coordinator".into(),
            name: None,
            objective: "Triage".into(),
            model: None,
            fallbacks: Vec::new(),
            scope: None,
            config: None,
            approval: None,
        },
        last_spawned: None,
    };
    spawn.status = ScheduleStatus::Active {
        next_due: at("2026-09-09T12:00:00.002Z"),
    };
    for job in [&first, &later, &paused, &spawn] {
        store
            .commit(Commit::new().mutate(Mutation::UpsertSchedule(job.clone())))
            .expect("insert");
    }
    assert_eq!(store.schedule(spawn.id).unwrap(), Some(spawn.clone()));
    let raw: String = store
        .read_conn()
        .unwrap()
        .query_row(
            "SELECT body FROM schedules WHERE id=?1",
            [spawn.id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(stored["target"], serde_json::json!(creator));
    assert!(stored.get("spawn").is_some());
    assert!(store.schedules(Some(creator)).unwrap().is_empty());
    assert_eq!(
        store.due_schedules(first.created_at, 10).expect("due"),
        vec![first.clone()]
    );
    assert_eq!(
        store
            .due_schedules(at("2026-09-09T12:00:01Z"), 1)
            .expect("page"),
        vec![first.clone()]
    );
    assert_eq!(
        store
            .due_schedules(at("2026-09-09T12:00:01Z"), 10)
            .expect("all due"),
        vec![first.clone(), later, spawn]
    );
    assert_eq!(
        store
            .schedules(Some(paused.target().expect("target")))
            .expect("target"),
        vec![paused]
    );
    assert_eq!(store.schedules(None).expect("all").len(), 4);
    assert!(
        store
            .due_schedules(first.created_at, 0)
            .expect("empty page")
            .is_empty()
    );
}
