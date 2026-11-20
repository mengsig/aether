use super::*;

fn at(text: &str) -> Timestamp {
    text.parse().expect("fixture timestamp")
}

fn interval(seconds: u64) -> Recurrence {
    Recurrence::Interval {
        seconds: NonZeroU64::new(seconds).expect("positive interval"),
    }
}

fn cron(expression: &str, timezone: &str) -> Recurrence {
    Recurrence::Cron {
        expression: expression.into(),
        timezone: timezone.into(),
    }
}

#[test]
fn downtime_does_not_move_the_interval_cadence() {
    let due = at("2026-09-09T12:00:00.125Z");
    let now = at("2026-09-09T12:03:15Z");
    assert_eq!(
        interval(60).advance_past(due, now).unwrap(),
        at("2026-09-09T12:04:00.125Z")
    );
    assert_eq!(
        interval(60).advance_past(due, due).unwrap(),
        at("2026-09-09T12:01:00.125Z")
    );
    assert_eq!(
        interval(60)
            .advance_past(due, at("2026-09-09T11:00:00Z"))
            .unwrap(),
        due
    );
}

#[test]
fn missed_occurrences_are_coalesced_or_skipped_without_a_replay_burst() {
    let due = at("2026-09-09T12:00:00Z");
    let later = at("2026-09-10T12:00:15Z");
    let rule = interval(60);
    for (policy, deliver) in [
        (MissedRunPolicy::Coalesce, true),
        (MissedRunPolicy::Skip, false),
    ] {
        assert_eq!(
            rule.plan_due(due, later, policy).unwrap(),
            Some(DueOccurrence {
                due,
                next_due: at("2026-09-10T12:01:00Z"),
                deliver,
            })
        );
    }
    assert!(
        rule.plan_due(due, at("2026-09-09T11:59:59Z"), MissedRunPolicy::Coalesce)
            .unwrap()
            .is_none()
    );
    for now in [due, at("2026-09-09T12:00:59.999Z")] {
        assert!(
            rule.plan_due(due, now, MissedRunPolicy::Skip)
                .unwrap()
                .unwrap()
                .deliver
        );
    }
    assert!(
        !rule
            .plan_due(due, at("2026-09-09T12:01:00Z"), MissedRunPolicy::Skip)
            .unwrap()
            .unwrap()
            .deliver
    );
}

#[test]
fn calendar_missed_runs_use_real_occurrences_across_dst() {
    let rule = cron("0 9 * * *", "Europe/Copenhagen");
    let due = at("2026-03-28T08:00:00Z");
    // The next local 09:00 is only 23 hours later.
    assert!(
        rule.plan_due(due, at("2026-03-29T06:59:59Z"), MissedRunPolicy::Skip)
            .unwrap()
            .unwrap()
            .deliver
    );
    let missed = rule
        .plan_due(due, at("2026-03-29T07:00:00Z"), MissedRunPolicy::Skip)
        .unwrap()
        .unwrap();
    assert!(!missed.deliver);
    assert_eq!(missed.next_due, at("2026-03-30T07:00:00Z"));
}

#[test]
fn invalid_and_overflowing_intervals_are_visible_errors() {
    assert!(serde_json::from_str::<Recurrence>(r#"{"kind":"interval","seconds":0}"#).is_err());
    assert!(
        interval(u64::MAX)
            .next_after(at("2026-09-09T00:00:00Z"))
            .is_err()
    );
    assert!(interval(1).next_after(Timestamp::MAX_UTC).is_err());
}

#[test]
fn calendar_recurrence_obeys_the_named_timezone() {
    let rule = cron("0 9 * * *", "Europe/Copenhagen");
    assert_eq!(
        rule.next_after(at("2026-01-01T08:00:00Z")).unwrap(),
        at("2026-01-02T08:00:00Z")
    );
    assert_eq!(
        rule.next_after(at("2026-07-01T06:59:59Z")).unwrap(),
        at("2026-07-01T07:00:00Z")
    );
}

#[test]
fn fixed_time_jobs_cross_dst_gaps_and_overlaps_once() {
    assert_eq!(
        cron("30 2 * * *", "America/New_York")
            .next_after(at("2026-03-08T06:59:00Z"))
            .unwrap(),
        at("2026-03-08T07:00:00Z")
    );
    assert_eq!(
        cron("30 1 * * *", "America/New_York")
            .next_after(at("2026-11-01T05:30:00Z"))
            .unwrap(),
        at("2026-11-02T06:30:00Z")
    );
}

#[test]
fn invalid_calendar_fields_and_zones_are_rejected() {
    let now = at("2026-09-09T00:00:00Z");
    assert!(matches!(
        cron("* * * *", "UTC").next_after(now),
        Err(RecurrenceError::Cron { .. })
    ));
    assert!(matches!(
        cron("61 * * * *", "UTC").next_after(now),
        Err(RecurrenceError::Cron { .. })
    ));
    assert!(matches!(
        cron("* * * * *", "Mars/Base").next_after(now),
        Err(RecurrenceError::Timezone(_))
    ));
}

// Protocol 8 wrote exactly this for a delivery schedule. Its body must
// survive a round trip byte for byte, and so must the create request.
const V8_SCHEDULE: &str = r#"{"id":"sch_01K4QZ5B6X9Q1N6D7W8E3R2T4Y","name":"Read docs","owner":{"actor":"user"},"target":"agt_01K4QZ5B6X9Q1N6D7W8E3R2T4Y","run":"run_01K4QZ5B6X9Q1N6D7W8E3R2T4Y","recurrence":{"kind":"interval","seconds":60},"missed":"coalesce","payload":{"task":"read"},"status":{"state":"active","next_due":"2026-09-09T12:00:00Z"},"max_deliveries":null,"deliveries":0,"last_inbox_seq":7,"created_at":"2026-09-09T11:59:00Z","updated_at":"2026-09-09T11:59:00Z"}"#;
const V8_SPEC: &str = r#"{"name":"Read docs","target":"agt_01K4QZ5B6X9Q1N6D7W8E3R2T4Y","recurrence":{"kind":"interval","seconds":60},"missed":"coalesce","payload":{"task":"read"},"max_deliveries":2}"#;

fn template() -> SpawnSpec {
    SpawnSpec {
        role: "coordinator".into(),
        name: None,
        objective: "Summarise overnight failures".into(),
        model: Some(ModelRef::new("anthropic", "claude-sonnet-5")),
        fallbacks: Vec::new(),
        scope: None,
        config: None,
        approval: None,
    }
}

#[test]
fn delivery_schedules_keep_the_protocol_8_shape() {
    let job: Schedule = serde_json::from_str(V8_SCHEDULE).unwrap();
    assert!(matches!(
        job.mode,
        ScheduleMode::Deliver {
            last_inbox_seq: Some(7),
            ..
        }
    ));
    assert_eq!(job.busy, BusyPolicy::Skip);
    assert_eq!(serde_json::to_string(&job).unwrap(), V8_SCHEDULE);
    let spec: CreateScheduleSpec = serde_json::from_str(V8_SPEC).unwrap();
    assert!(matches!(spec.target, ScheduleTarget::Agent(_)));
    assert_eq!(serde_json::to_string(&spec).unwrap(), V8_SPEC);
}

#[test]
fn spawn_schedules_round_trip_and_mixed_shapes_are_refused() {
    let mut spec: CreateScheduleSpec = serde_json::from_str(V8_SPEC).unwrap();
    spec.target = ScheduleTarget::Spawn(template());
    spec.busy = BusyPolicy::QueueOne;
    let text = serde_json::to_value(&spec).unwrap();
    assert!(text.get("target").is_none());
    assert_eq!(text["busy"], "queue_one");
    assert_eq!(
        serde_json::from_value::<CreateScheduleSpec>(text).unwrap(),
        spec
    );

    let mut both: serde_json::Value = serde_json::from_str(V8_SPEC).unwrap();
    both["spawn"] = serde_json::json!(template());
    let neither =
        serde_json::json!({"name": "x", "recurrence": {"kind": "interval", "seconds": 60}});
    for body in [both, neither] {
        let error = serde_json::from_value::<CreateScheduleSpec>(body).unwrap_err();
        assert!(error.to_string().contains("exactly one"), "{error}");
    }

    let mut job: Schedule = serde_json::from_str(V8_SCHEDULE).unwrap();
    job.mode = ScheduleMode::Spawn {
        template: template(),
        last_spawned: None,
    };
    let body = serde_json::to_value(&job).unwrap();
    assert!(body.get("target").is_none() && body.get("run").is_none());
    assert_eq!(
        serde_json::from_value::<Schedule>(body.clone()).unwrap(),
        job
    );
    let mut mixed = body;
    mixed["last_inbox_seq"] = serde_json::json!(3);
    assert!(serde_json::from_value::<Schedule>(mixed).is_err());
}

const CREATOR: &str = "agt_01K4QZ5B6X9Q1N6D7W8E3R2T4Y";

fn agents_spawn_schedule(status: ScheduleStatus) -> Schedule {
    let mut job: Schedule = serde_json::from_str(V8_SCHEDULE).unwrap();
    job.owner = Actor::agent(CREATOR.parse().unwrap());
    job.mode = ScheduleMode::Spawn {
        template: template(),
        last_spawned: None,
    };
    job.status = status;
    job
}

#[test]
fn an_agents_spawn_schedule_is_stored_in_a_shape_older_releases_refuse() {
    let at = at("2026-09-09T12:00:00Z");
    let failed = ScheduleStatus::Failed {
        at,
        error: "its creator ended".into(),
    };
    let can_fire_again = [
        ScheduleStatus::Active { next_due: at },
        ScheduleStatus::Paused { next_due: at },
        failed,
    ];
    for status in can_fire_again {
        let job = agents_spawn_schedule(status);
        let body = serde_json::to_value(StoredSchedule(job.clone())).unwrap();
        // Releases without agent spawn schedules read a `spawn` only with no
        // `target`, and a `target` only with a `run`: they refuse this one.
        assert_eq!(body["target"], CREATOR, "{body}");
        assert!(body.get("run").is_none() && body.get("spawn").is_some());
        assert_eq!(
            serde_json::from_value::<StoredSchedule>(body).unwrap().0,
            job
        );
    }
    for status in [
        ScheduleStatus::Cancelled { at },
        ScheduleStatus::Exhausted { at },
    ] {
        let job = agents_spawn_schedule(status);
        let body = serde_json::to_value(StoredSchedule(job.clone())).unwrap();
        assert!(
            body.get("target").is_none(),
            "nothing left to guard: {body}"
        );
        assert_eq!(
            serde_json::from_value::<StoredSchedule>(body).unwrap().0,
            job
        );
    }
}

#[test]
fn a_spawn_schedule_with_the_wrong_creator_mark_is_refused() {
    let at = at("2026-09-09T12:00:00Z");
    let active = agents_spawn_schedule(ScheduleStatus::Active { next_due: at });
    let refused = |body: serde_json::Value| {
        let error = serde_json::from_value::<StoredSchedule>(body).unwrap_err();
        assert!(error.to_string().contains("names its creator"), "{error}");
    };
    let marked = serde_json::to_value(StoredSchedule(active)).unwrap();

    let mut unmarked = marked.clone();
    unmarked.as_object_mut().unwrap().remove("target");
    refused(unmarked);
    let mut another = marked.clone();
    another["target"] = serde_json::json!("agt_01K4QZ5B6X9Q1N6D7W8E3R2T4Z");
    refused(another);
    let mut operators = marked.clone();
    operators["owner"] = serde_json::json!({"actor": "user"});
    refused(operators);
    let mut cancelled = marked;
    cancelled["status"] = serde_json::json!({"state": "cancelled", "at": "2026-09-09T12:00:00Z"});
    refused(cancelled);
}
