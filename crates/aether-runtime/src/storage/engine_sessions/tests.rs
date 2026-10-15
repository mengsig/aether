use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    sync::Arc,
    time::Duration,
};

use aether_core::{
    agent::{AgentConfig, AgentStats, AgentStatus, Role, Scope},
    id::{AgentId, RunId},
    model::ModelRef,
};
use aether_store::{Commit, Mutation};
use serde_json::{Value, json};

use super::*;

const DAY: u64 = 86400;

fn at(days: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(days * DAY)
}

struct Fixture {
    runtime: Arc<Runtime>,
    _config: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let config = tempfile::tempdir().unwrap();
        fs::write(
            config.path().join("config.toml"),
            "[storage]\nworkspaces_days = 0\n",
        )
        .unwrap();
        let (runtime, config) = crate::test_runtime_in(config, |_| {}).await;
        runtime.shutdown.cancel();
        tokio::task::yield_now().await;
        Self {
            runtime,
            _config: config,
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn owner(&self, status: AgentStatus) -> Agent {
        Agent {
            id: AgentId::new(),
            parent: None,
            role: Role::new("worker"),
            name: "log owner".into(),
            scope: Scope::default(),
            model: ModelRef::new("mock", "test"),
            fallbacks: vec![],
            status,
            paused: false,
            paused_via: None,
            current_task: None,
            archived: false,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(at(30).into()),
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats::default(),
            run: RunId::new(),
            created_at: at(30).into(),
            updated_at: at(30).into(),
        }
    }

    fn store(&self, agent: &Agent) {
        self.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
    }

    fn record(&self, record: &EngineSessionOwnership) {
        self.runtime
            .store
            .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(record.clone())))
            .unwrap();
    }

    fn owned(&self, engine: &str, owner: &Agent) -> EngineSessionOwnership {
        let session_id = ulid::Ulid::generate().to_string();
        let home = fs::canonicalize(self.home.path()).unwrap();
        let path = match engine {
            "claude-code" => home
                .join(".claude/projects/-temporary-project")
                .join(format!("{session_id}.jsonl")),
            "codex" => home
                .join(".codex/sessions/2020/01/01")
                .join(format!("rollout-2020-01-01T00-00-00-{session_id}.jsonl")),
            _ => unreachable!(),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_log(&path, engine, &session_id);
        let record = EngineSessionOwnership {
            engine: engine.into(),
            session_id,
            agent_id: owner.id,
            log_paths: vec![path],
            created_at: at(30).into(),
            expired_owner: None,
        };
        self.record(&record);
        record
    }

    fn sweep(&self, days: u16, dry_run: bool) -> Value {
        self.runtime
            .gc_engine_sessions(days, at(70), dry_run)
            .unwrap()
    }
}

fn write_log(path: &Path, engine: &str, id: &str) {
    let header = if engine == "codex" {
        json!({"type": "session_meta", "payload": {"id": id}})
    } else {
        json!({"type": "user", "sessionId": id, "message": {"role": "user", "content": "hello"}})
    };
    fs::write(path, format!("{header}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    set_modified(path, at(30));
}

/// The head of a Claude Code 2.1.299 log, whose queue records precede the first prompt.
fn write_queued_claude_log(path: &Path, id: &str) {
    let records = [
        json!({"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-09-24T09:12:03.418Z", "sessionId": id}),
        json!({"type": "queue-operation", "operation": "dequeue", "timestamp": "2026-09-24T09:12:03.419Z", "sessionId": id}),
        json!({
            "parentUuid": null, "isSidechain": false, "promptId": "3f6c1d2e-0a4b-4c5d-9e8f-1a2b3c4d5e6f",
            "type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]},
            "uuid": "7b1e2c3d-4f5a-4b6c-8d7e-9f0a1b2c3d4e", "timestamp": "2026-09-24T09:12:03.425Z",
            "permissionMode": "bypassPermissions", "promptSource": "sdk", "turnOrigin": "sdk",
            "turnPosition": {"promptIndex": 0, "turnIndex": 1}, "userType": "external",
            "entrypoint": "sdk-cli", "cwd": "/work", "sessionId": id, "version": "2.1.299", "gitBranch": "main"
        }),
    ];
    let lines: String = records.iter().map(|record| format!("{record}\n")).collect();
    fs::write(path, lines).unwrap();
    set_modified(path, at(30));
}

fn set_modified(path: &Path, time: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn reason<'a>(report: &'a Value, path: &Path) -> &'a str {
    report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path.to_str().unwrap())
        .unwrap()["reason"]
        .as_str()
        .unwrap()
}

#[tokio::test]
async fn owned_final_logs_preview_matches_deletion_and_user_logs_survive() {
    let f = Fixture::new().await;
    let completed = f.owner(AgentStatus::Completed { summary: None });
    let cancelled = f.owner(AgentStatus::Cancelled { reason: None });
    f.store(&completed);
    f.store(&cancelled);
    let claude = f.owned("claude-code", &completed);
    let codex = f.owned("codex", &cancelled);
    let user = claude.log_paths[0].with_file_name("user-session.jsonl");
    write_log(&user, "claude-code", "user-session");
    let preview = f.sweep(14, true);
    let bytes = fs::metadata(&claude.log_paths[0]).unwrap().len()
        + fs::metadata(&codex.log_paths[0]).unwrap().len();
    assert_eq!(preview["bytes"], bytes);
    assert_eq!(preview["bytes_eligible"], bytes);
    assert_eq!(preview["bytes_removed"], 0);
    assert!(claude.log_paths[0].exists() && codex.log_paths[0].exists());
    assert!(
        preview["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["action"] == "would_remove")
    );
    let actual = f.sweep(14, false);
    assert_eq!(actual["bytes_removed"], preview["bytes_eligible"]);
    assert!(!claude.log_paths[0].exists() && !codex.log_paths[0].exists());
    assert!(user.exists());
    assert_eq!(
        f.runtime.store.engine_session_ownerships().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn claude_logs_opening_with_queue_records_are_reclaimed() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Cancelled { reason: None });
    f.store(&owner);
    let queued = f.owned("claude-code", &owner);
    write_queued_claude_log(&queued.log_paths[0], &queued.session_id);
    let other = f.owned("claude-code", &owner);
    write_queued_claude_log(&other.log_paths[0], "another-session");
    let bytes = fs::metadata(&queued.log_paths[0]).unwrap().len();
    let preview = f.sweep(14, true);
    assert_eq!(
        reason(&preview, &queued.log_paths[0]),
        "owned final session older than retention"
    );
    assert_eq!(
        reason(&preview, &other.log_paths[0]),
        "native session identity missing or mismatched"
    );
    assert_eq!(preview["bytes_eligible"], bytes);
    let actual = f.sweep(14, false);
    assert_eq!(actual["bytes_removed"], bytes);
    assert!(!queued.log_paths[0].exists() && other.log_paths[0].exists());
}

#[tokio::test]
async fn live_imported_and_wakeable_failed_owners_are_preserved() {
    let f = Fixture::new().await;
    let statuses = [
        AgentStatus::Created,
        AgentStatus::Ready,
        AgentStatus::Running,
        AgentStatus::Failed {
            error: "temporarily blocked".into(),
        },
        AgentStatus::Imported {
            was: AgentStatusKind::Completed,
        },
    ];
    let mut paths = vec![];
    for status in statuses {
        let mut owner = f.owner(status);
        owner.archived = true;
        owner.config.max_turns = Some(1);
        owner.stats.turns = 1;
        f.store(&owner);
        paths.push(f.owned("claude-code", &owner).log_paths[0].clone());
    }
    let report = f.sweep(14, false);
    assert_eq!(report["bytes_removed"], 0);
    for path in paths {
        assert!(path.exists());
        assert_eq!(reason(&report, &path), "owner is live or resumable");
    }
}

#[tokio::test]
async fn failed_with_successor_is_final_but_missing_owner_is_unknown() {
    let f = Fixture::new().await;
    let mut owner = f.owner(AgentStatus::Failed {
        error: "restarted".into(),
    });
    owner.restarted_as = Some(AgentId::new());
    f.store(&owner);
    let restarted = f.owned("codex", &owner);
    let absent = f.owned("codex", &f.owner(AgentStatus::Completed { summary: None }));
    let report = f.sweep(14, false);
    assert!(!restarted.log_paths[0].exists());
    assert!(absent.log_paths[0].exists());
    assert_eq!(
        reason(&report, &absent.log_paths[0]),
        "owner history unavailable; finality unknown"
    );
}

#[tokio::test]
async fn disabled_recent_future_and_retention_boundary_are_respected() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let records: Vec<_> = (0..4).map(|_| f.owned("codex", &owner)).collect();
    set_modified(&records[0].log_paths[0], at(56));
    set_modified(&records[1].log_paths[0], at(56) + Duration::from_nanos(1));
    set_modified(&records[2].log_paths[0], at(71));
    let mut recent = f.owner(AgentStatus::Completed { summary: None });
    recent.updated_at = at(69).into();
    f.store(&recent);
    let recent_record = f.owned("claude-code", &recent);
    let disabled = f.sweep(0, false);
    assert_eq!(disabled["bytes_eligible"], 0);
    assert!(records.iter().all(|record| record.log_paths[0].exists()));
    let report = f.sweep(14, false);
    assert!(!records[0].log_paths[0].exists());
    assert!(records[1].log_paths[0].exists() && records[2].log_paths[0].exists());
    assert!(!records[3].log_paths[0].exists());
    assert!(recent_record.log_paths[0].exists());
    assert_eq!(
        reason(&report, &recent_record.log_paths[0]),
        "owner changed within retention"
    );
}

#[tokio::test]
async fn current_owner_state_is_used_after_preview() {
    let f = Fixture::new().await;
    let mut owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("codex", &owner);
    assert!(f.sweep(14, true)["bytes_eligible"].as_u64().unwrap() > 0);
    owner.status = AgentStatus::Ready;
    f.store(&owner);
    assert_eq!(f.sweep(14, false)["bytes_removed"], 0);
    assert!(record.log_paths[0].exists());
}

#[tokio::test]
async fn state_and_file_are_rechecked_after_inspection_before_unlink() {
    let f = Fixture::new().await;
    let mut owner = f.owner(AgentStatus::Failed {
        error: "restarted".into(),
    });
    owner.restarted_as = Some(AgentId::new());
    f.store(&owner);
    let record = f.owned("codex", &owner);
    let scan = f.runtime.inspect_engine_sessions().unwrap();
    owner.restarted_as = None;
    f.store(&owner);
    let report = f
        .runtime
        .finish_engine_session_gc(scan, 14, at(70), false)
        .unwrap();
    assert_eq!(
        reason(&report, &record.log_paths[0]),
        "owner is live or resumable"
    );
    owner.status = AgentStatus::Completed { summary: None };
    f.store(&owner);
    let scan = f.runtime.inspect_engine_sessions().unwrap();
    fs::write(&record.log_paths[0], "replacement user data").unwrap();
    let report = f
        .runtime
        .finish_engine_session_gc(scan, 14, at(70), false)
        .unwrap();
    assert_eq!(
        reason(&report, &record.log_paths[0]),
        "file or parent changed since inspection"
    );
    assert_eq!(
        fs::read_to_string(&record.log_paths[0]).unwrap(),
        "replacement user data"
    );
}

#[tokio::test]
async fn recent_status_or_ownership_and_unrecognized_paths_are_retained() {
    let f = Fixture::new().await;
    let mut recent = f.owner(AgentStatus::Completed { summary: None });
    recent.status_since = Some(at(69).into());
    f.store(&recent);
    let recent_log = f.owned("codex", &recent);
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let mut new_record = f.owned("claude-code", &owner);
    new_record.session_id = "recent-ownership".into();
    new_record.log_paths = vec![new_record.log_paths[0].with_file_name("recent-ownership.jsonl")];
    new_record.created_at = at(69).into();
    write_log(
        &new_record.log_paths[0],
        "claude-code",
        &new_record.session_id,
    );
    f.record(&new_record);
    let mut unknown = new_record.clone();
    unknown.session_id = "personal-file".into();
    unknown.log_paths = vec![f.home.path().join("personal-file.txt")];
    fs::write(&unknown.log_paths[0], "user data").unwrap();
    f.record(&unknown);
    let report = f.sweep(14, false);
    assert_eq!(
        reason(&report, &recent_log.log_paths[0]),
        "owner changed within retention"
    );
    assert_eq!(
        reason(&report, &new_record.log_paths[0]),
        "within retention"
    );
    assert_eq!(
        reason(&report, &unknown.log_paths[0]),
        "unrecognized engine log path"
    );
    assert!(
        recent_log.log_paths[0].exists()
            && new_record.log_paths[0].exists()
            && unknown.log_paths[0].exists()
    );
}

#[tokio::test]
async fn winding_down_turn_and_cross_agent_reference_protect_final_logs() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("codex", &owner);
    let cancel = tokio_util::sync::CancellationToken::new();
    f.runtime.sched.lock().running.insert(
        owner.id,
        crate::RunningTurn {
            task: None,
            interrupt: cancel.child_token(),
            cancel,
            cancel_request: None,
            stop_reason: None,
            addressed: false,
            provider: "mock".into(),
            engine: true,
            on_provider: false,
        },
    );
    assert_eq!(
        reason(&f.sweep(14, false), &record.log_paths[0]),
        "owner has an in-flight turn"
    );
    f.runtime.sched.lock().running.clear();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    f.runtime.engine_turns.lock().insert(
        owner.id,
        crate::turn_engine::EngineTurnHandle {
            token: "test".into(),
            tx,
        },
    );
    assert_eq!(
        reason(&f.sweep(14, false), &record.log_paths[0]),
        "owner has an in-flight turn"
    );
    f.runtime.engine_turns.lock().clear();
    let mut borrower = f.owner(AgentStatus::Ready);
    borrower.engine_session = Some(record.session_id.clone());
    f.store(&borrower);
    assert_eq!(
        reason(&f.sweep(14, false), &record.log_paths[0]),
        "session referenced by another agent"
    );
    assert!(record.log_paths[0].exists());
}

#[tokio::test]
async fn symlinks_directories_hardlinks_and_writable_files_are_preserved() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let records: Vec<_> = (0..5).map(|_| f.owned("codex", &owner)).collect();
    let outside = f.home.path().join("user-file");
    fs::write(&outside, "user data").unwrap();
    fs::remove_file(&records[0].log_paths[0]).unwrap();
    symlink(&outside, &records[0].log_paths[0]).unwrap();
    fs::remove_file(&records[1].log_paths[0]).unwrap();
    fs::create_dir(&records[1].log_paths[0]).unwrap();
    fs::hard_link(&records[2].log_paths[0], f.home.path().join("hardlink")).unwrap();
    fs::set_permissions(&records[3].log_paths[0], fs::Permissions::from_mode(0o666)).unwrap();
    let parent = records[4].log_paths[0].parent().unwrap();
    let moved = parent.with_file_name("moved");
    fs::rename(parent, &moved).unwrap();
    symlink(&moved, parent).unwrap();
    let report = f.sweep(14, false);
    assert_eq!(report["bytes_removed"], 0);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "user data");
    assert!(records.iter().all(|record| record.log_paths[0].exists()));
    assert!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["reason"] == "path cannot be safely inspected")
    );
    fs::remove_file(parent).unwrap();
    fs::rename(moved, parent).unwrap();
    let report = f.sweep(14, false);
    assert_eq!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["action"] == "removed")
            .count(),
        1
    );
    assert!(
        records[..4]
            .iter()
            .all(|record| record.log_paths[0].exists())
    );
}

#[tokio::test]
async fn mismatched_malformed_unknown_and_oversized_headers_are_preserved() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    for engine in ["claude-code", "codex"] {
        for contents in [
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"another-session\"}}\n".to_owned(),
            "{\"type\":\"user\",\"sessionId\":\"another-session\"}\n".to_owned(),
            "not json\n".into(),
            "{}\n".into(),
            " ".repeat(70 * 1024),
        ] {
            let record = f.owned(engine, &owner);
            fs::write(&record.log_paths[0], contents).unwrap();
            set_modified(&record.log_paths[0], at(30));
        }
    }
    let report = f.sweep(14, false);
    assert_eq!(report["bytes_removed"], 0);
    assert!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["reason"] == "native session identity missing or mismatched")
    );
}

#[tokio::test]
async fn ambiguous_paths_are_kept_and_missing_or_unknown_paths_are_reported() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("codex", &owner);
    let mut conflicting = record.clone();
    conflicting.session_id = "another-session".into();
    f.record(&conflicting);
    let missing = f.owned("claude-code", &owner);
    fs::remove_file(&missing.log_paths[0]).unwrap();
    let mut unknown = record.clone();
    unknown.session_id = "unknown-path".into();
    unknown.log_paths.clear();
    f.record(&unknown);
    let report = f.sweep(14, false);
    assert_eq!(report["unknown_paths"], 1);
    assert_eq!(report["owned_sessions"], 4);
    assert_eq!(report["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        reason(&report, &record.log_paths[0]),
        "ambiguous or changed ownership"
    );
    assert_eq!(
        reason(&report, &missing.log_paths[0]),
        "recorded log is missing"
    );
    assert!(record.log_paths[0].exists());
}

#[tokio::test]
async fn file_and_parent_replacement_after_inspection_prevent_unlink() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    let record = f.owned("claude-code", &owner);
    let path = &record.log_paths[0];
    let file = LogFile::inspect(path, &[&record]).unwrap();
    fs::rename(path, path.with_extension("saved")).unwrap();
    write_log(path, "claude-code", &record.session_id);
    assert!(file.recheck().unwrap().is_some());
    assert!(file.remove().is_err());
    let file = LogFile::inspect(path, &[&record]).unwrap();
    let parent = path.parent().unwrap();
    let moved = parent.with_file_name("moved-project");
    fs::rename(parent, &moved).unwrap();
    fs::create_dir(parent).unwrap();
    write_log(path, "claude-code", &record.session_id);
    assert!(file.recheck().unwrap().is_some());
    assert!(file.remove().is_err());
    assert!(path.exists() && moved.join(path.file_name().unwrap()).exists());
}

#[tokio::test]
async fn bounded_sweeps_rotate_past_immutable_missing_records() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let mut missing = f.owned("claude-code", &owner);
    let path = missing.log_paths.remove(0);
    fs::remove_file(path).unwrap();
    missing.session_id = "many-unknown-paths".into();
    missing.log_paths = (0..MAX_ENTRIES)
        .map(|i| f.home.path().join(format!("a-missing-{i:05}.jsonl")))
        .collect();
    f.record(&missing);
    let candidate = f.owned("codex", &owner);
    let mut conflict = candidate.clone();
    conflict.session_id = "z-conflicting-session".into();
    f.record(&conflict);
    // Put the owned log after the first full page of retained missing paths.
    let mut late = candidate.clone();
    late.session_id = "late-session".into();
    late.log_paths = vec![
        f.home
            .path()
            .join("z")
            .join("rollout-2020-late-session.jsonl"),
    ];
    fs::create_dir(late.log_paths[0].parent().unwrap()).unwrap();
    write_log(&late.log_paths[0], "codex", &late.session_id);
    f.record(&late);
    let first = f.runtime.storage_gc(false).unwrap();
    assert_eq!(first["engine_sessions"]["truncated"], true);
    assert_eq!(
        reason(&first["engine_sessions"], &candidate.log_paths[0]),
        "ambiguous or changed ownership"
    );
    assert!(candidate.log_paths[0].exists());
    assert!(late.log_paths[0].exists());
    let preview = f.runtime.storage_gc(true).unwrap();
    assert!(
        preview["engine_sessions"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == late.log_paths[0].to_str().unwrap()
                && entry["action"] == "would_remove")
    );
    let second = f.runtime.storage_gc(false).unwrap();
    assert_eq!(
        second["engine_sessions"]["bytes_removed"],
        preview["engine_sessions"]["bytes_eligible"]
    );
    assert!(!late.log_paths[0].exists());
}

#[tokio::test]
async fn default_history_expiry_preserves_finality_until_native_retention() {
    let f = Fixture::new().await;
    for status in [
        AgentStatus::Completed { summary: None },
        AgentStatus::Cancelled { reason: None },
        AgentStatus::Failed {
            error: "expired".into(),
        },
    ] {
        let owner = f.owner(status);
        f.store(&owner);
        let record = f.owned("codex", &owner);
        assert!(matches!(
            f.runtime
                .expire_run_history(owner.run, at(37).into())
                .unwrap(),
            aether_store::RunExpiry::Expired { .. }
        ));
        assert!(f.runtime.store.agent(owner.id).unwrap().is_none());
        let preview = f.runtime.gc_engine_sessions(14, at(43), true).unwrap();
        assert_eq!(
            reason(&preview, &record.log_paths[0]),
            "owner changed within retention"
        );
        assert!(record.log_paths[0].exists());
        let report = f.runtime.gc_engine_sessions(14, at(44), false).unwrap();
        assert!(report["bytes_removed"].as_u64().unwrap() > 0);
        assert!(!record.log_paths[0].exists());
    }
}

#[tokio::test]
async fn current_agent_and_other_session_references_override_expired_finality() {
    let f = Fixture::new().await;
    let mut owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("claude-code", &owner);
    assert!(matches!(
        f.runtime
            .expire_run_history(owner.run, at(50).into())
            .unwrap(),
        aether_store::RunExpiry::Expired { .. }
    ));
    let mut borrower = f.owner(AgentStatus::Ready);
    borrower.engine_session = Some(record.session_id.clone());
    f.store(&borrower);
    assert_eq!(
        reason(&f.sweep(14, false), &record.log_paths[0]),
        "session referenced by another agent"
    );
    borrower.engine_session = None;
    f.store(&borrower);
    // A restored row always takes precedence over earlier expiry evidence.
    owner.status = AgentStatus::Ready;
    f.store(&owner);
    assert_eq!(
        reason(&f.sweep(14, false), &record.log_paths[0]),
        "owner is live or resumable"
    );
    assert!(record.log_paths[0].exists());
}

#[tokio::test]
async fn durable_lease_and_quarantined_resource_protect_native_log() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("codex", &owner);
    let lease = aether_core::lease::Lease::new(
        owner.id,
        "test",
        1,
        aether_core::time::now(),
        Duration::from_secs(60),
    );
    f.runtime
        .apply(Commit::new().mutate(Mutation::SetLease(lease)))
        .unwrap();
    assert_eq!(f.sweep(14, false)["bytes_removed"], 0);
    f.runtime
        .apply(Commit::new().mutate(Mutation::ClearLease(owner.id)))
        .unwrap();
    f.runtime.resources.quarantine(
        owner.id,
        vec![aether_core::tool::ResourceKey::file(&record.log_paths[0])],
    );
    assert_eq!(f.sweep(14, false)["bytes_removed"], 0);
    assert!(record.log_paths[0].exists());
    f.runtime.resources.release_quarantine(owner.id);
    assert!(f.sweep(14, false)["bytes_removed"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn preview_of_eligible_native_log_does_not_wait_for_mutation_locks() {
    let f = Fixture::new().await;
    let owner = f.owner(AgentStatus::Completed { summary: None });
    f.store(&owner);
    let record = f.owned("codex", &owner);
    let sweep = f.runtime.storage_gc_lock.lock();
    let hold = f.runtime.hold();
    let runtime = f.runtime.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(runtime.storage_gc(true)).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(hold);
    drop(sweep);
    worker.join().unwrap();
    let report = result
        .expect("preview must not wait for mutation locks")
        .unwrap();
    assert!(
        report["engine_sessions"]["bytes_eligible"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(report["engine_sessions"]["bytes_removed"], 0);
    assert!(record.log_paths[0].exists());
}
