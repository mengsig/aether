use super::*;
use crate::{CreateAgentSpec, RuntimeConfig, RuntimeError};
use aether_core::event::EventKind;
use aether_core::model::ModelRef;
use aether_model::testing::ScriptedClient;
use aether_store::{EventFilter, Store, StoreError};
use std::time::Instant;

const FLOOR: u64 = 1000;

fn names(commit: &Commit) -> Vec<&str> {
    commit
        .events
        .iter()
        .map(|draft| match &draft.kind {
            EventKind::Custom { name, .. } => name.as_str(),
            other => panic!("not a disk event: {other:?}"),
        })
        .collect()
}

/// Records what it is given, or refuses it as a full disk would.
fn recorder(refuse: bool, into: &mut Vec<Commit>) -> impl FnOnce(Commit) -> crate::Result<()> + '_ {
    move |commit| {
        if refuse {
            return Err(RuntimeError::Store(StoreError::Io(std::io::Error::from(
                std::io::ErrorKind::StorageFull,
            ))));
        }
        into.push(commit);
        Ok(())
    }
}

#[test]
fn a_disk_hovering_at_its_floor_holds_turns_until_a_tenth_to_spare() {
    let disk = Mutex::new(DiskState::default());
    let path = Path::new("/data");
    let mut recorded = Vec::new();
    let mut observe = |available| {
        let released = disk.lock().observe(path, FLOOR, available);
        record(&disk, recorder(false, &mut recorded));
        (released, disk.lock().low.clone())
    };

    assert_eq!(observe(FLOOR).1, None, "at the floor is not below it");
    let (released, low) = observe(FLOOR - 1);
    let low = low.expect("below the floor holds new turns");
    assert!(!released);
    assert_eq!((low.available_bytes, low.floor_bytes), (FLOOR - 1, FLOOR));
    let (released, low) = observe(FLOOR + FLOOR / 10 - 1);
    assert!(
        !released,
        "back over the floor by less than a tenth still holds"
    );
    assert_eq!(
        low.expect("still low").available_bytes,
        FLOOR + FLOOR / 10 - 1
    );
    let (released, low) = observe(FLOOR + FLOOR / 10);
    assert!(released && low.is_none());
    assert!(!observe(FLOOR).0, "nothing was held to release");

    let names: Vec<_> = recorded.iter().flat_map(names).collect();
    assert_eq!(names, [DISK_LOW, DISK_RECOVERED], "one event per change");
}

#[test]
fn a_change_the_disk_refuses_to_record_is_kept_until_it_lands() {
    let disk = Mutex::new(DiskState::default());
    let path = Path::new("/data");
    let mut recorded = Vec::new();

    disk.lock().observe(path, FLOOR, 0);
    let since = disk.lock().low.as_ref().expect("low").since;
    record(&disk, recorder(true, &mut recorded));
    record(&disk, recorder(true, &mut recorded));
    assert!(recorded.is_empty());

    // Room again before the low ever landed: the recovery says when it began.
    assert!(disk.lock().observe(path, FLOOR, FLOOR * 2));
    record(&disk, recorder(false, &mut recorded));
    let [commit] = recorded.as_slice() else {
        panic!("one commit: {}", recorded.len());
    };
    assert_eq!(names(commit), [DISK_RECOVERED]);
    let EventKind::Custom { payload, .. } = &commit.events[0].kind else {
        unreachable!()
    };
    assert_eq!(payload["low_since"], serde_json::json!(since));
    assert_eq!(payload["available_bytes"], FLOOR * 2);

    record(&disk, recorder(false, &mut recorded));
    assert_eq!(recorded.len(), 1, "recorded once");
}

#[tokio::test]
async fn new_turns_wait_out_a_low_disk_and_start_once_it_has_room() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        "done",
    )]));
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
        Arc::new(aether_model::Models::load(Some(dir.path().to_path_buf())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        Some(client.clone()),
        Default::default(),
    )
    .await
    .expect("runtime");

    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    let low = runtime.status().expect("status").low_disk.expect("low");
    assert_eq!(low.available_bytes, FLOOR / 2);
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("work".into()),
                start: true,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    for _ in 0..3 {
        crate::scheduler::tick(&runtime).expect("tick");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(client.requests().is_empty(), "no turn starts on a low disk");
    assert!(!runtime.sched.lock().running.contains_key(&agent.id));

    observe(&runtime, dir.path(), FLOOR, FLOOR * 2);
    assert_eq!(runtime.status().expect("status").low_disk, None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.requests().is_empty() {
        assert!(Instant::now() < deadline, "the held turn never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let events = runtime
        .store
        .events(0, 1000, &EventFilter::default())
        .expect("events");
    let disk_events: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Custom { name, .. } if name.starts_with("disk.") => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(disk_events, [DISK_LOW, DISK_RECOVERED]);
    runtime.shutdown().await;
}

async fn runtime_in(dir: &Path) -> Arc<Runtime> {
    let store = aether_store::SqliteStore::open_in_memory().expect("store");
    runtime_on(dir, store).await
}

async fn runtime_on(dir: &Path, store: aether_store::SqliteStore) -> Arc<Runtime> {
    Runtime::start_with_engines(
        RuntimeConfig {
            workspace_root: Some(dir.to_path_buf()),
            ..Default::default()
        },
        Arc::new(store),
        Arc::new(aether_model::Models::load(Some(dir.to_path_buf())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("runtime")
}

fn disk_events(runtime: &Runtime) -> Vec<String> {
    runtime
        .store
        .events(0, 1000, &EventFilter::default())
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Custom { name, .. } if name.starts_with("disk.") => Some(name),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_restart_with_room_records_the_recovery_of_a_low_it_never_saw_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    *runtime.disk.lock() = DiskState::default();

    resume_low(&runtime);
    assert!(runtime.disk_is_low(), "turns wait until the first check");
    observe(&runtime, dir.path(), FLOOR, FLOOR * 2);
    assert!(!runtime.disk_is_low());
    assert_eq!(disk_events(&runtime), [DISK_LOW, DISK_RECOVERED]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_restart_that_is_still_low_does_not_announce_the_low_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    *runtime.disk.lock() = DiskState::default();

    resume_low(&runtime);
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    assert_eq!(disk_events(&runtime), [DISK_LOW]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn lifting_the_floor_entirely_still_records_the_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    *runtime.disk.lock() = DiskState::default();

    close_unrecovered_low(&runtime);
    close_unrecovered_low(&runtime);
    assert_eq!(disk_events(&runtime), [DISK_LOW, DISK_RECOVERED]);
    runtime.shutdown().await;
}

fn custom(name: &str, payload: serde_json::Value) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::Custom {
            name: name.into(),
            payload,
        },
    )
}

#[tokio::test]
async fn each_recorded_change_keeps_the_open_low_beside_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    let open = runtime
        .store
        .meta(OPEN_LOW_META)
        .expect("meta")
        .expect("kept");
    let low: LowDisk = serde_json::from_str(&open).expect("a low");
    assert_eq!(low.available_bytes, FLOOR / 2);
    assert_eq!(low.floor_bytes, FLOOR);
    observe(&runtime, dir.path(), FLOOR, FLOOR * 2);
    assert_eq!(
        runtime.store.meta(OPEN_LOW_META).expect("meta").as_deref(),
        Some("")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_restart_reads_the_kept_state_not_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    observe(&runtime, dir.path(), FLOOR, FLOOR / 2);
    observe(&runtime, dir.path(), FLOOR, FLOOR * 2);
    // Only the log says low now, as a daemon from before the key would leave it.
    let stale = serde_json::json!({"path": "/x", "available_bytes": 1, "floor_bytes": FLOOR, "since": now()});
    runtime
        .apply(Commit::new().event(custom(DISK_LOW, stale)))
        .expect("apply");
    *runtime.disk.lock() = DiskState::default();

    resume_low(&runtime);
    assert!(!runtime.disk_is_low(), "the kept state wins over the log");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_store_from_before_the_key_finds_its_low_beneath_unrelated_events_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = aether_store::SqliteStore::open_in_memory().expect("store");
    let payload = serde_json::json!({"path": "/x", "available_bytes": 5, "floor_bytes": FLOOR, "since": now()});
    store
        .commit(Commit::new().event(custom(DISK_LOW, payload)))
        .expect("commit");
    for _ in 0..3 {
        let commit = (0..200).fold(Commit::new(), |commit, _| {
            commit.event(custom("other.note", serde_json::json!({})))
        });
        store.commit(commit).expect("commit");
    }
    assert_eq!(store.meta(OPEN_LOW_META).expect("meta"), None);

    // No floor is set, so the start closes the low it found in the log.
    let runtime = runtime_on(dir.path(), store).await;
    assert_eq!(disk_events(&runtime), [DISK_LOW, DISK_RECOVERED]);
    assert_eq!(
        runtime.store.meta(OPEN_LOW_META).expect("meta").as_deref(),
        Some("")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_store_from_before_the_key_with_no_disk_events_keeps_an_empty_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime_in(dir.path()).await;
    assert!(disk_events(&runtime).is_empty());
    assert_eq!(
        runtime.store.meta(OPEN_LOW_META).expect("meta").as_deref(),
        Some("")
    );
    resume_low(&runtime);
    assert!(!runtime.disk_is_low());
    runtime.shutdown().await;
}
