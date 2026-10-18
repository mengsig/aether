use super::*;
use aether_core::agent::DeadlineUpdate;
use aether_runtime::CreateAgentSpec;
use std::sync::Arc;

mod attachment_replay;
mod attachments;
#[cfg(unix)]
mod credential_directory;
mod device_gate;
mod history_expiry;
mod provider_usage;
mod read_artifact;
mod resolution;
mod shutdown;
mod shutdown_approval;
mod subscription_cancel;

#[tokio::test]
async fn model_catalogue_carries_aliases_and_marks_an_aliased_default() {
    let (daemon, _config_dir) = daemon().await;
    daemon
        .runtime
        .set_default_model(Some(aether_core::model::ModelRef::new(
            "claude-code",
            "opus",
        )));
    let Reply::Models(models) = dispatch(&daemon, Command::ListModels, Actor::user())
        .await
        .expect("model catalogue")
    else {
        panic!("model catalogue reply");
    };
    let opus = models
        .iter()
        .find(|info| {
            info.model == aether_core::model::ModelRef::new("claude-code", "claude-opus-5-5")
        })
        .expect("builtin opus");
    assert_eq!(opus.aliases, ["opus", "opus-5-5", "opus-5.5"]);
    assert_eq!(opus.capabilities.context_window, 200_000);
    assert!(opus.is_default);
    assert_eq!(opus.tier.as_deref(), Some("strong"));
    let default = models
        .iter()
        .find(|info| info.model.provider == "claude-code" && info.model.model == "default")
        .expect("default model");
    assert_eq!(
        serde_json::to_value(default).unwrap()["tier"],
        serde_json::Value::Null
    );
    assert_eq!(models.iter().filter(|info| info.is_default).count(), 1);
    daemon.runtime.shutdown().await;
}

#[test]
fn expired_history_is_a_visible_missing_history_error_not_internal_failure() {
    let error = aether_store::StoreError::HistoryExpired {
        first_seq: 2,
        last_seq: 10,
    };
    let reply = store_err(error);
    assert_eq!(reply.code, ErrorCode::NotFound);
    assert!(reply.message.contains("history expired"));
    assert!(reply.message.contains("2..=10"));
    let runtime = map_err(RuntimeError::Store(
        aether_store::StoreError::HistoryExpired {
            first_seq: 2,
            last_seq: 10,
        },
    ));
    assert_eq!(runtime, reply);
}

/// Clients send a person to sign in on `no_model` alone, so a model that is
/// named but unknown must not share that code.
#[tokio::test]
async fn creation_with_no_model_at_all_has_its_own_code() {
    let (daemon, _config_dir) = daemon().await;
    daemon.runtime.set_default_model(None);
    let refused = |model: Option<aether_core::model::ModelRef>| {
        let spec = CreateAgentSpec {
            role: "root".into(),
            model,
            ..Default::default()
        };
        map_err(
            daemon
                .runtime
                .create_agent(spec, Actor::user())
                .expect_err("refused"),
        )
    };
    let none = refused(None);
    assert_eq!(none.code, ErrorCode::NoModel, "{none:?}");
    assert!(none.message.contains("no model configured"), "{none:?}");
    let unknown = refused(Some(aether_core::model::ModelRef::new(
        "nowhere",
        "claude-nope",
    )));
    assert_ne!(unknown.code, ErrorCode::NoModel, "{unknown:?}");
    assert!(unknown.message.contains("nowhere"), "{unknown:?}");
}

#[tokio::test]
async fn github_watcher_configuration_is_operator_only_and_revision_checked() {
    let (daemon, _config) = daemon().await;
    let Reply::GithubWatcher(initial) = dispatch(&daemon, Command::GetGithubWatcher, Actor::user())
        .await
        .expect("status")
    else {
        panic!("watcher status reply");
    };
    assert!(!initial.config.enabled);
    let request = Command::ConfigureGithubWatcher {
        config: initial.config.clone(),
        expected_revision: initial.revision,
    };
    let denied = dispatch(
        &daemon,
        request.clone(),
        Actor::agent(aether_core::id::AgentId::new()),
    )
    .await
    .expect_err("operator only");
    assert_eq!(denied.code, ErrorCode::Unauthorized);
    let mut granting = initial.config.clone();
    granting.operator_users = Some(vec!["mallory".into()]);
    let denied = dispatch(
        &daemon,
        Command::ConfigureGithubWatcher {
            config: granting,
            expected_revision: initial.revision,
        },
        Actor::agent(aether_core::id::AgentId::new()),
    )
    .await
    .expect_err("an agent cannot name operator logins");
    assert_eq!(denied.code, ErrorCode::Unauthorized);
    let Reply::GithubWatcher(unchanged) =
        dispatch(&daemon, Command::GetGithubWatcher, Actor::user())
            .await
            .expect("status")
    else {
        panic!("watcher status reply");
    };
    assert_eq!(unchanged, initial);
    let Reply::GithubWatcher(updated) = dispatch(&daemon, request.clone(), Actor::user())
        .await
        .expect("save")
    else {
        panic!("watcher status reply");
    };
    assert_eq!(updated.revision, initial.revision + 1);
    assert!(
        dispatch(&daemon, request, Actor::user()).await.is_err(),
        "stale writes cannot overwrite another operator"
    );
}

/// A daemon over an in-memory store, so `dispatch` can be exercised
/// without a socket. The config directory is returned with it: dropped
/// here it would be removed while the daemon still names it, and leaked
/// it would stay on disk after the run.
pub(super) async fn daemon() -> (Daemon, tempfile::TempDir) {
    let store: Arc<dyn aether_store::Store> =
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store"));
    daemon_with_store(store).await
}

async fn daemon_with_store(store: Arc<dyn aether_store::Store>) -> (Daemon, tempfile::TempDir) {
    daemon_with_network(store, aether_core::agent::NetworkPolicy::Allowed).await
}

async fn daemon_with_network(
    store: Arc<dyn aether_store::Store>,
    network: aether_core::agent::NetworkPolicy,
) -> (Daemon, tempfile::TempDir) {
    let config_dir = tempfile::tempdir().expect("config dir");
    let models = Arc::new(
        aether_model::Models::load(Some(config_dir.path().to_path_buf())).expect("models"),
    );
    let runtime = aether_runtime::Runtime::start_with_engines(
        aether_runtime::RuntimeConfig {
            holder: "test".into(),
            network_default: network,
            default_model: Some(aether_core::model::ModelRef::new(
                "anthropic",
                "claude-sonnet-5-5",
            )),
            attachment_dir: Some(config_dir.path().join("attachments")),
            ..Default::default()
        },
        store,
        models.clone(),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("runtime");
    (
        Daemon {
            runtime,
            models,
            instance_id: "inst".into(),
            version: "test".into(),
            push: None,
            remote: Default::default(),
        },
        config_dir,
    )
}

/// The other half of the client/daemon seam: the reply variant each of
/// these commands actually produces. `aether-client`'s own test pins what
/// its methods accept; a mismatch here means the command cannot work, and
/// nothing below the socket would have caught it.
#[tokio::test]
async fn pause_resume_and_set_answer_the_shapes_the_client_expects() {
    let (daemon, _config_dir) = daemon().await;
    let (root, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");

    let paused = dispatch(
        &daemon,
        Command::Pause {
            agent: root.id,
            subtree: true,
            idempotency_key: None,
        },
        Actor::user(),
    )
    .await
    .expect("pause");
    let Reply::Paused { agents, mid_turn } = paused else {
        panic!("pause must answer Reply::Paused, got {paused:?}");
    };
    assert_eq!(
        agents.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![root.id]
    );
    assert!(mid_turn.is_empty());

    let resumed = dispatch(
        &daemon,
        Command::Resume {
            agent: root.id,
            subtree: true,
            idempotency_key: None,
        },
        Actor::user(),
    )
    .await
    .expect("resume");
    assert!(
        matches!(&resumed, Reply::Agents(a) if a.len() == 1),
        "resume must answer Reply::Agents, got {resumed:?}"
    );

    let sent = dispatch(
        &daemon,
        Command::SendMessage {
            to: root.id,
            content: "stand down".into(),
            subtree: true,
            attachments: Vec::new(),
            idempotency_key: None,
        },
        Actor::user(),
    )
    .await
    .expect("fan out");
    assert!(
        matches!(&sent, Reply::MessagesSent { deliveries } if deliveries.len() == 1),
        "a subtree send must answer Reply::MessagesSent, got {sent:?}"
    );

    let updated = dispatch(
        &daemon,
        Command::UpdateAgent {
            id: root.id,
            patch: aether_protocol::AgentPatch {
                deadline: Some(DeadlineUpdate::Clear),
                ..Default::default()
            },
            idempotency_key: None,
        },
        Actor::user(),
    )
    .await
    .expect("set");
    let Reply::Agent(agent) = updated else {
        panic!("set must answer Reply::Agent, got {updated:?}");
    };
    assert_eq!(agent.config.deadline, None);
    daemon.runtime.shutdown().await;
}

/// Idempotency over the whole seam: the key is read off the frame, the
/// digest is taken from the frame, and a retry of that frame is answered
/// with the first submission's identity.
#[tokio::test]
async fn a_retried_frame_is_answered_with_the_first_submissions_identity() {
    let (daemon, _config_dir) = daemon().await;
    let (agent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    let frame = |content: &str, key: &str| Command::SendMessage {
        to: agent.id,
        content: content.into(),
        subtree: false,
        attachments: Vec::new(),
        idempotency_key: Some(key.into()),
    };

    let first = dispatch(&daemon, frame("ship it", "outbox-1"), Actor::user())
        .await
        .expect("first");
    let retry = dispatch(&daemon, frame("ship it", "outbox-1"), Actor::user())
        .await
        .expect("retry");
    assert_eq!(first, retry, "the retry is answered, not re-executed");
    assert_eq!(
        daemon
            .runtime
            .store()
            .inbox(agent.id, 0)
            .expect("inbox")
            .len(),
        1
    );

    // The same key on different work is a client bug, and it is named.
    let conflict = dispatch(&daemon, frame("hold on", "outbox-1"), Actor::user())
        .await
        .expect_err("a reused key is refused");
    assert_eq!(conflict.code, ErrorCode::InvalidArgument);
    assert!(conflict.message.contains("outbox-1"), "{conflict:?}");

    // A malformed key never reaches the runtime.
    let malformed = dispatch(&daemon, frame("ship it", "with space"), Actor::user())
        .await
        .expect_err("a malformed key is refused");
    assert_eq!(malformed.code, ErrorCode::InvalidArgument);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn conditional_reply_dispatch_preserves_origin_and_replays_only_identical_requests() {
    use aether_core::event::{EventDraft, EventKind, OperatorOrigin};
    let (daemon, _config_dir) = daemon().await;
    let (agent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let question_seq = daemon
        .runtime
        .apply(
            aether_store::Commit::new().event(
                EventDraft::new(
                    Actor::agent(agent.id),
                    EventKind::QuestionAsked {
                        question: "Proceed?".into(),
                    },
                )
                .agent(agent.id),
            ),
        )
        .expect("question")[0]
        .seq;
    let frame = |content: &str| Command::ReplyToQuestion {
        to: agent.id,
        question_seq,
        content: content.into(),
        idempotency_key: Some("voice-1".into()),
    };
    let denied = dispatch(&daemon, frame("yes"), Actor::agent(agent.id))
        .await
        .expect_err("operator only");
    assert_eq!(denied.code, ErrorCode::InvalidArgument);
    assert!(
        daemon
            .runtime
            .store()
            .inbox(agent.id, 0)
            .expect("inbox")
            .is_empty()
    );
    let operator = Actor::connected_user(OperatorOrigin::LocalSocket {
        connection: "voice-test".into(),
    });
    let first = dispatch(&daemon, frame("yes"), operator.clone())
        .await
        .expect("fresh");
    assert!(matches!(first, Reply::MessageSent { .. }));
    let retry = dispatch(&daemon, frame("yes"), operator.clone())
        .await
        .expect("retry");
    assert_eq!(retry, first);
    assert_eq!(
        dispatch(&daemon, frame("no"), operator)
            .await
            .expect_err("conflict")
            .code,
        ErrorCode::InvalidArgument
    );
    let inbox = daemon.runtime.store().inbox(agent.id, 0).expect("inbox");
    assert_eq!(inbox.len(), 1);
    assert!(
        matches!(&inbox[0].kind, aether_core::inbox::InboxKind::Message {
        from: Actor::User { origin: Some(OperatorOrigin::LocalSocket { connection }), .. }, ..
    } if connection == "voice-test")
    );
    daemon.runtime.shutdown().await;
}

/// A byte cut through a multi-byte character makes a text artifact decode
/// as binary at the other end, so the cut backs off to the character
/// boundary. Genuinely invalid bytes are not text and keep the full cut.
#[test]
fn truncation_lands_on_a_character_boundary() {
    // 4 ASCII bytes then a 3-byte character: cutting at 6 splits it.
    let text = "abcd\u{4e16}\u{754c}".as_bytes();
    assert_eq!(super::dispatch::text_safe_end(text, 6), 4);
    assert_eq!(
        std::str::from_utf8(&text[..super::dispatch::text_safe_end(text, 6)]),
        Ok("abcd"),
        "the kept prefix must still be text"
    );
    // A clean boundary is not moved.
    assert_eq!(super::dispatch::text_safe_end(text, 7), 7);
    // Binary: an invalid sequence that is not our own cut stays put.
    let binary = [0x41u8, 0xff, 0xfe, 0x42, 0x43];
    assert_eq!(super::dispatch::text_safe_end(&binary, 4), 4);
}

#[tokio::test]
async fn history_before_rejects_conflicting_after_and_preserves_default_order() {
    let (daemon, _config) = daemon().await;
    for n in 0..3 {
        daemon
            .runtime
            .store()
            .commit(
                aether_store::Commit::new().event(aether_core::event::EventDraft::new(
                    Actor::System,
                    aether_core::event::EventKind::Custom {
                        name: format!("history.{n}"),
                        payload: serde_json::json!({}),
                    },
                )),
            )
            .unwrap();
    }
    let query = aether_protocol::EventQuery {
        before: Some(u64::MAX),
        ..Default::default()
    };
    let error = dispatch(
        &daemon,
        Command::GetEvents {
            after: 1,
            limit: 2,
            query: query.clone(),
        },
        Actor::user(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let Reply::Events(latest) = dispatch(
        &daemon,
        Command::GetEvents {
            after: 0,
            limit: 2,
            query,
        },
        Actor::user(),
    )
    .await
    .unwrap() else {
        panic!("events")
    };
    assert_eq!(latest.len(), 2);
    assert!(latest[0].seq > latest[1].seq);
    let Reply::Events(oldest) = dispatch(
        &daemon,
        Command::GetEvents {
            after: 0,
            limit: 2,
            query: Default::default(),
        },
        Actor::user(),
    )
    .await
    .unwrap() else {
        panic!("events")
    };
    assert!(oldest[0].seq < oldest[1].seq);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn retention_preview_dispatch_returns_read_only_json() {
    use aether_core::{agent::AgentStatus, time::now};
    use aether_store::{Commit, Mutation};
    let (daemon, _config_dir) = daemon().await;
    daemon.runtime.shutdown().await;
    let (mut agent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                name: Some("old history".into()),
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    agent.status = AgentStatus::Completed { summary: None };
    agent.updated_at = now() - chrono::Duration::days(14);
    daemon
        .runtime
        .store()
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    let store = daemon.runtime.store();
    let seq = store.last_seq().unwrap();
    let reply = dispatch(&daemon, Command::PreviewRetention, Actor::user())
        .await
        .unwrap();
    let wire = serde_json::to_value(&reply).unwrap();
    assert_eq!(wire["reply"], "retention_preview");
    assert!(wire["data"]["at"].is_string());
    assert!(wire["data"]["history_before"].is_string());
    assert!(wire["data"]["runs"].is_array());
    assert!(wire["data"]["artifact_payload_bytes"].is_u64());
    assert!(wire["data"]["history_payload_bytes"].is_u64());
    assert_eq!(wire["data"]["eligible_artifacts"], serde_json::json!([]));
    assert_eq!(
        wire["data"]["pending_cleanup"]["pending_unlinks"],
        serde_json::json!([])
    );
    assert_eq!(
        wire["data"]["pending_cleanup"]["staged_publications"],
        serde_json::json!([])
    );
    // Creating the fixture emitted a recent event, so the real guard protects it.
    assert_eq!(wire["data"]["runs"][0]["decision"]["status"], "protected");
    assert_eq!(
        wire["data"]["runs"][0]["decision"]["reason"],
        "recent event or invalid event timestamp"
    );
    assert_eq!(store.agent(agent.id).unwrap(), Some(agent));
    assert_eq!(store.last_seq().unwrap(), seq);
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);
}

#[tokio::test]
async fn retention_preview_empty_store_has_explicit_empty_lists() {
    let (daemon, _config_dir) = daemon().await;
    let Reply::RetentionPreview(preview) =
        dispatch(&daemon, Command::PreviewRetention, Actor::user())
            .await
            .unwrap()
    else {
        panic!("retention preview reply");
    };
    assert!(preview.runs.is_empty());
    assert!(preview.eligible_artifacts.is_empty());
    assert_eq!(preview.history_payload_bytes, 0);
    assert_eq!(preview.artifact_payload_bytes, 0);
    daemon.runtime.shutdown().await;
}

#[test]
fn single_pass_frame_decoding_preserves_correlation_and_duplicate_validation() {
    for line in [
        r#"{"req":9,"cmd":"create_agent","role":"x","role":"y"}"#,
        r#"{"cmd":"create_agent","role":"x","config":{"effort":"low","effort":"high"},"req":9}"#,
        r#"{"cmd":"unknown_command","req":9}"#,
    ] {
        let (id, decoded) = parse_frame(line).unwrap();
        assert_eq!(id, 9);
        assert!(decoded.is_err(), "{line}");
    }
    for line in [
        r#"{"req":9,"req":10,"cmd":"ping"}"#,
        r#"{"req":9,"cmd":"ping","bad":[}"#,
        r#"[9]"#,
    ] {
        assert!(parse_frame(line).is_err(), "{line}");
    }
    assert!(matches!(
        parse_frame(r#"{"cmd":"ping","req":9}"#).unwrap(),
        (
            9,
            Ok(ClientFrame {
                command: Command::Ping,
                ..
            })
        )
    ));
}

/// A host-only read is still the operator's: an agent acting through the
/// daemon is refused before anything is looked up.
#[tokio::test]
async fn an_agent_cannot_ask_which_memories_were_in_context() {
    let (daemon, _config) = daemon().await;
    let command = Command::MemoriesInContext {
        task: Some(aether_core::id::TaskId::new()),
        invocation: None,
    };
    let agent = Actor::agent(aether_core::id::AgentId::new());
    let refused = dispatch(&daemon, command.clone(), agent)
        .await
        .expect_err("operators only");
    assert_eq!(refused.code, ErrorCode::InvalidArgument);
    assert!(refused.message.contains("operator"), "{}", refused.message);
    let unknown = dispatch(&daemon, command, Actor::user())
        .await
        .expect_err("no such task");
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn providers_reply_and_usage_tool_share_unknown_headroom() {
    let (daemon, _config) = daemon().await;
    let Reply::Providers(rows) = dispatch(&daemon, Command::Providers, Actor::user())
        .await
        .unwrap()
    else {
        panic!("providers reply");
    };
    let tool = daemon.runtime.provider_usage();
    let enabled = tool["providers"].as_array().unwrap();
    assert_eq!(enabled.len(), rows.iter().filter(|row| row.enabled).count());
    for provider in rows.iter().filter(|row| row.enabled) {
        let entry = enabled
            .iter()
            .find(|row| row["provider"] == provider.provider)
            .unwrap();
        assert_eq!(
            entry["headroom"],
            serde_json::to_value(&provider.headroom).unwrap()
        );
        assert!(provider.headroom.stale && provider.headroom.windows.is_empty());
    }
    daemon.runtime.shutdown().await;
}
