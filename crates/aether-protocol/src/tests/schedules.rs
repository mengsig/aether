use super::*;
use aether_core::event::Actor;
use aether_core::schedule::{
    Schedule, ScheduleMode, ScheduleStatus, ScheduleWire, SpawnSpec, StoredSchedule,
};
use serde::Deserialize;

// Freeze the schedule mode check from the Aether 1.4.2 (48669d5ac) v26 decoder.
#[derive(Debug, Deserialize)]
#[serde(try_from = "ScheduleWire")]
struct V26Schedule(ScheduleWire);

impl TryFrom<ScheduleWire> for V26Schedule {
    type Error = &'static str;

    fn try_from(wire: ScheduleWire) -> Result<Self, Self::Error> {
        match (wire.target, wire.run, &wire.spawn) {
            (Some(_), Some(_), None) if wire.last_spawned.is_none() => Ok(Self(wire)),
            (None, None, Some(_)) if wire.last_inbox_seq.is_none() => Ok(Self(wire)),
            _ => Err("a schedule has either `target` and `run` or a `spawn` template"),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "reply", content = "data", rename_all = "snake_case")]
enum V26Reply {
    Schedule(Box<V26Schedule>),
    Schedules(Vec<V26Schedule>),
}

#[derive(Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum V26Frame {
    Reply {
        id: u64,
        result: Result<V26Reply, ErrorReply>,
    },
}

fn active_agent_spawn() -> Schedule {
    let at = aether_core::time::now();
    let owner = Actor::agent(AgentId::new());
    Schedule {
        id: aether_core::id::ScheduleId::new(),
        name: "poll".into(),
        owner,
        mode: ScheduleMode::Spawn {
            template: SpawnSpec {
                role: "poller".into(),
                objective: "Check the queue".into(),
                name: None,
                model: Some(aether_core::model::ModelRef::new(
                    "anthropic",
                    "claude-sonnet-5-5",
                )),
                fallbacks: Vec::new(),
                scope: None,
                config: None,
                approval: None,
            },
            last_spawned: None,
        },
        recurrence: aether_core::schedule::Recurrence::Interval {
            seconds: std::num::NonZeroU64::new(3600).unwrap(),
        },
        missed: Default::default(),
        busy: Default::default(),
        payload: serde_json::Value::Null,
        status: ScheduleStatus::Active { next_due: at },
        deliveries: 0,
        last: None,
        max_deliveries: None,
        created_at: at,
        updated_at: at,
    }
}

#[test]
fn v26_client_decodes_active_agent_spawn_schedule_replies() {
    let hello = Hello {
        protocol_version: 26,
        ..Hello::new("aether", "1.4.2")
    };
    let version = compat::negotiate(&hello).unwrap();
    assert_eq!(version, 26);
    let job = active_agent_spawn();

    let stored = serde_json::to_value(StoredSchedule(job.clone())).unwrap();
    assert!(serde_json::from_value::<V26Schedule>(stored).is_err());

    for reply in [
        Reply::Schedule(job.clone()),
        Reply::Schedules(vec![job.clone()]),
    ] {
        let frame = ServerFrame::Reply {
            id: 42,
            result: Ok(reply),
        };
        let line = compat::encode_for(version, &frame).unwrap();
        assert_eq!(decode::<ServerFrame>(&line).unwrap(), frame);
        let V26Frame::Reply { id, result } = decode(&line).unwrap();
        assert_eq!(id, 42);
        let entries = match result.unwrap() {
            V26Reply::Schedule(entry) => vec![*entry],
            V26Reply::Schedules(entries) => entries,
        };
        assert_eq!(entries.len(), 1);
        let schedule = &entries[0].0;
        assert_eq!(schedule.id, job.id);
        assert_eq!(schedule.owner, job.owner);
        assert_eq!(schedule.status, job.status);
        assert!(schedule.target.is_none() && schedule.run.is_none());
        assert_eq!(schedule.spawn.as_ref().unwrap().role, "poller");
    }
}
