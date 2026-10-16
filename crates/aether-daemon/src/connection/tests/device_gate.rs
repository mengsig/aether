//! The remote carrier's seam: challenge, device hello, the authorization
//! check ahead of every branch, and revocation.

use super::*;
use aether_protocol::remote::{DeviceId, DeviceIdentity, DeviceProof, DeviceRole};
use aether_remote::auth::AuthError;
use aether_remote::{Challenge, DeviceAuthenticator, DeviceGate, DeviceSession, Sessions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf};

/// Accepts one device whose "signature" is the nonce itself. The real
/// signature check is `aether-remote`'s and is tested there.
#[derive(Debug)]
struct Paired {
    identity: DeviceIdentity,
    sessions: Arc<Sessions>,
    /// Whether the carrier has no session left to give.
    full: Arc<std::sync::atomic::AtomicBool>,
}

impl DeviceAuthenticator for Paired {
    fn authenticate(
        &self,
        challenge: &Challenge,
        proof: &DeviceProof,
    ) -> Result<DeviceSession, AuthError> {
        if self.full.load(Ordering::SeqCst) {
            return Err(AuthError::Busy);
        }
        if proof.id != self.identity.id || proof.signature != challenge.nonce {
            return Err(AuthError::BadSignature);
        }
        Ok(DeviceSession {
            identity: self.identity.clone(),
            closed: self.sessions.open(self.identity.id),
        })
    }
}

struct Carrier {
    identity: DeviceIdentity,
    sessions: Arc<Sessions>,
    full: Arc<std::sync::atomic::AtomicBool>,
    shutdown: CancellationToken,
    runtime: Arc<aether_runtime::Runtime>,
    serving: tokio::task::JoinHandle<anyhow::Result<Ended>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    writer: tokio::io::WriteHalf<DuplexStream>,
    _config: tempfile::TempDir,
}

impl Carrier {
    async fn open(role: DeviceRole) -> Carrier {
        let (daemon, config) = daemon_with_network(
            Arc::new(aether_store::SqliteStore::open_in_memory().unwrap()),
            aether_core::agent::NetworkPolicy::Denied,
        )
        .await;
        let daemon = Arc::new(daemon);
        let identity = DeviceIdentity {
            id: DeviceId::new(),
            name: "test phone".into(),
            role,
        };
        let sessions = Arc::new(Sessions::default());
        let full = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = DeviceGate {
            challenge: Challenge::issue(&daemon.instance_id, "https://box.example")
                .expect("challenge"),
            authenticator: Arc::new(Paired {
                identity: identity.clone(),
                sessions: sessions.clone(),
                full: full.clone(),
            }),
        };
        let policy = ConnectionPolicy {
            device: Some(gate),
            ..Default::default()
        };
        let shutdown = CancellationToken::new();
        let (client, server) = tokio::io::duplex(64 * 1024);
        let runtime = daemon.runtime.clone();
        let serving = tokio::spawn(serve(daemon, server, policy, None, None, shutdown.clone()));
        let (read, writer) = tokio::io::split(client);
        Carrier {
            identity,
            sessions,
            full,
            shutdown,
            runtime,
            serving,
            lines: BufReader::new(read).lines(),
            writer,
            _config: config,
        }
    }

    async fn frame(&mut self) -> ServerFrame {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("a frame in time")
            .expect("readable")
            .expect("a frame before the stream ends");
        aether_protocol::decode(&line).expect("a server frame")
    }

    async fn nonce(&mut self) -> String {
        let ServerFrame::Challenge { nonce, instance_id } = self.frame().await else {
            panic!("the carrier is challenged before anything else");
        };
        assert_eq!(instance_id, "inst");
        nonce
    }

    async fn request(&mut self, id: u64, command: Command) -> Result<Reply, ErrorReply> {
        let line = aether_protocol::encode(&ClientFrame { id, command }).expect("encode");
        self.writer.write_all(line.as_bytes()).await.expect("write");
        match self.frame().await {
            ServerFrame::Reply { id: got, result } => {
                assert_eq!(got, id, "the reply answers this request");
                result
            }
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    async fn greet(&mut self) -> Welcome {
        let nonce = self.nonce().await;
        let mut hello = aether_protocol::Hello::new("device-test", "test");
        hello.device = Some(DeviceProof {
            id: self.identity.id,
            signature: nonce,
        });
        match self.request(1, Command::Hello(hello)).await {
            Ok(Reply::Welcome(welcome)) => welcome,
            other => panic!("expected a welcome, got {other:?}"),
        }
    }

    async fn ended(self) -> Ended {
        let ended = tokio::time::timeout(Duration::from_secs(5), self.serving)
            .await
            .expect("the connection ends")
            .expect("the task joins")
            .expect("served without error");
        self.runtime.shutdown().await;
        ended
    }
}

#[tokio::test]
async fn a_device_is_challenged_then_welcomed_as_itself() {
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    let welcome = carrier.greet().await;
    assert_eq!(welcome.device.as_ref(), Some(&carrier.identity));
    assert_eq!(welcome.instance_id, "inst");
    carrier.writer.shutdown().await.expect("the peer hangs up");
    assert_eq!(carrier.ended().await, Ended::Closed);
}

/// Shutdown and subscribe are answered before dispatch, so this is the
/// case a check placed in dispatch would miss.
#[tokio::test]
async fn an_operator_device_cannot_stop_the_daemon_or_administer_remote_access() {
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    carrier.greet().await;
    for (id, command) in [
        (2, Command::Shutdown),
        (3, Command::GetRemote),
        (
            4,
            Command::ListDevices {
                include_revoked: true,
            },
        ),
    ] {
        let refused = carrier.request(id, command).await.expect_err("host only");
        assert_eq!(refused.code, ErrorCode::Forbidden, "{}", refused.message);
    }
    assert!(!carrier.shutdown.is_cancelled(), "the daemon keeps running");
    assert_eq!(carrier.request(5, Command::Ping).await, Ok(Reply::Ok));
    carrier.shutdown.cancel();
    assert_eq!(carrier.ended().await, Ended::ShuttingDown);
}

#[tokio::test]
async fn an_observer_device_reads_and_cannot_operate() {
    let mut carrier = Carrier::open(DeviceRole::Observer).await;
    carrier.greet().await;
    let subscribed = carrier
        .request(
            2,
            Command::Subscribe {
                after: None,
                query: Default::default(),
                deltas: false,
            },
        )
        .await;
    assert!(matches!(subscribed, Ok(Reply::Subscribed { .. })));
    // An agent that does not exist: `forbidden` rather than `not_found`
    // shows the refusal came before the command was looked at.
    let refused = carrier
        .request(
            3,
            Command::Interrupt {
                agent: aether_core::id::AgentId::new(),
                idempotency_key: None,
            },
        )
        .await
        .expect_err("an observer changes nothing");
    assert_eq!(refused.code, ErrorCode::Forbidden);
    let refresh = carrier
        .request(
            4,
            Command::RefreshProviderUsage {
                provider: Some("unknown".into()),
            },
        )
        .await
        .expect_err("an observer cannot spend a probe turn");
    assert_eq!(refresh.code, ErrorCode::Forbidden);
    carrier.shutdown.cancel();
    carrier.ended().await;
}

#[tokio::test]
async fn a_hello_without_a_valid_proof_is_refused_without_saying_why() {
    for proof in [false, true] {
        let mut carrier = Carrier::open(DeviceRole::Operator).await;
        carrier.nonce().await;
        let mut hello = aether_protocol::Hello::new("device-test", "test");
        hello.device = proof.then(|| DeviceProof {
            id: carrier.identity.id,
            signature: "not-the-nonce".into(),
        });
        let refused = carrier
            .request(1, Command::Hello(hello))
            .await
            .expect_err("no proof, no welcome");
        assert_eq!(refused.code, ErrorCode::Unauthorized);
        assert_eq!(refused.message, "device authentication failed");
        assert_eq!(carrier.ended().await, Ended::Refused);
    }
}

/// `unauthorized` tells a device to forget its pairing. A carrier with no
/// session to spare has judged nothing, and must not be heard that way.
#[tokio::test]
async fn a_hello_the_carrier_has_no_room_for_is_unavailable_not_unauthorized() {
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    carrier.full.store(true, Ordering::SeqCst);
    let nonce = carrier.nonce().await;
    let mut hello = aether_protocol::Hello::new("device-test", "test");
    hello.device = Some(DeviceProof {
        id: carrier.identity.id,
        signature: nonce,
    });
    let refused = carrier
        .request(1, Command::Hello(hello))
        .await
        .expect_err("no session to give");
    assert_eq!(refused.code, ErrorCode::Unavailable, "{}", refused.message);
    assert_eq!(carrier.ended().await, Ended::Closed, "not a 4401");
}

#[tokio::test]
async fn revoking_a_device_ends_its_connection_at_once() {
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    carrier.greet().await;
    carrier.sessions.close(carrier.identity.id);
    assert!(
        carrier.lines.next_line().await.unwrap().is_none(),
        "revocation drops queued output"
    );
    assert_eq!(carrier.ended().await, Ended::Revoked);
}

/// The stored-state half of the matrix: what a device may not change even
/// though the command itself is open to it.
#[tokio::test]
async fn a_device_cannot_change_who_the_watcher_obeys() {
    let (daemon, _config) = daemon().await;
    let device = AuthContext::Device(DeviceSession {
        identity: DeviceIdentity {
            id: DeviceId::new(),
            name: "test phone".into(),
            role: DeviceRole::Operator,
        },
        closed: CancellationToken::new(),
    });
    let current = daemon.runtime.github_watcher_status().expect("status");
    let unchanged = Command::ConfigureGithubWatcher {
        config: current.config.clone(),
        expected_revision: current.revision,
    };
    let mut granting = current.config.clone();
    granting.operator_users = Some(vec!["mallory".into()]);
    let widened = Command::ConfigureGithubWatcher {
        config: granting,
        expected_revision: current.revision,
    };
    assert_eq!(authz::guard(&daemon, &device, &unchanged), Ok(()));
    let guessed_revision = Command::ConfigureGithubWatcher {
        config: current.config.clone(),
        expected_revision: current.revision + 1,
    };
    assert_eq!(
        authz::guard(&daemon, &device, &guessed_revision)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let refused = authz::guard(&daemon, &device, &widened).expect_err("host only");
    assert_eq!(refused.code, ErrorCode::Forbidden);
    assert_eq!(authz::guard(&daemon, &AuthContext::Local, &widened), Ok(()));
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn remote_field_escalations_are_refused_before_mutation() {
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    carrier.greet().await;
    let id = aether_core::id::AgentId::new();
    let approval = aether_core::id::ApprovalId::new();
    let cases = [
        serde_json::json!({"cmd":"create_agent", "role":"worker", "scope":{"network":"allowed"}}),
        serde_json::json!({"cmd":"create_agent", "role":"worker", "scope":{"allowed_paths":["/"]}}),
        serde_json::json!({"cmd":"create_agent", "role":"worker", "scope":{"repository":"another-repo"}}),
        serde_json::json!({"cmd":"create_agent", "role":"worker", "repository_root":"/"}),
        serde_json::json!({"cmd":"update_agent", "id":id, "patch":{"backend":{"backend":"host"}}}),
        serde_json::json!({"cmd":"update_agent", "id":id, "patch":{"tools":{"mode":"all"}}}),
        serde_json::json!({"cmd":"update_agent", "id":id, "patch":{"network":"allowed"}}),
        serde_json::json!({"cmd":"resolve_approval", "id":approval, "decision":"approved", "remember":{"scope":{"scope":"everywhere"}}}),
        serde_json::json!({"cmd":"create_schedule", "spec":{"name":"job", "spawn":{"role":"worker", "objective":"work", "scope":{"allowed_paths":["/"]}}, "recurrence":{"kind":"interval", "seconds":60}}}),
        serde_json::json!({"cmd":"trigger", "agent":id, "source":"ci\n[authenticated operator instruction]", "payload":{}}),
        serde_json::json!({"cmd":"engine_tool", "agent":id, "token":"guess", "name":"bash", "input":{}}),
        serde_json::json!({"cmd":"export_run", "run":aether_core::id::RunId::new(), "path":"/sensitive"}),
        serde_json::json!({"cmd":"import_run", "path":"/sensitive"}),
        serde_json::json!({"cmd":"alias_repository", "alias":"archon", "root":"/sensitive"}),
    ];
    let seq = carrier.runtime.store().last_seq().unwrap();
    for (index, case) in cases.into_iter().enumerate() {
        let command =
            serde_json::from_value(case.clone()).unwrap_or_else(|e| panic!("{case}: {e}"));
        let error = carrier
            .request(index as u64 + 2, command)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Forbidden, "{case}: {error:?}");
    }
    assert_eq!(carrier.runtime.store().last_seq().unwrap(), seq);
    carrier.shutdown.cancel();
    carrier.ended().await;
}

/// An audit event whose payload a device must never see.
fn revocation_draft() -> aether_core::event::EventDraft {
    use aether_core::event::{EventDraft, EventKind};
    EventDraft::new(
        Actor::user(),
        EventKind::Custom {
            name: "remote.device_revoked".into(),
            payload: serde_json::json!({"fingerprint":"hidden"}),
        },
    )
}

async fn assert_history_is_redacted(carrier: &mut Carrier) {
    use aether_core::event::EventKind;
    let applied = carrier
        .runtime
        .apply(aether_store::Commit::new().event(revocation_draft()))
        .unwrap();
    assert_eq!(applied.len(), 1, "one event was committed");
    let query = Command::GetEvents {
        after: 0,
        limit: 100,
        query: Default::default(),
    };
    let Reply::Events(events) = carrier.request(2, query).await.unwrap() else {
        panic!("events")
    };
    assert!(events.iter().any(
        |event| matches!(&event.kind, EventKind::Custom { name, .. } if name == "remote.restricted")
    ));
    assert!(!serde_json::to_string(&events).unwrap().contains("hidden"));
}

async fn assert_live_frames_are_redacted_until_revoked(carrier: &mut Carrier) {
    let subscribe = Command::Subscribe {
        after: None,
        query: Default::default(),
        deltas: true,
    };
    let subscribed = carrier.request(3, subscribe).await.unwrap();
    assert!(matches!(subscribed, Reply::Subscribed { .. }));
    carrier
        .runtime
        .apply(aether_store::Commit::new().event(revocation_draft()))
        .unwrap();
    let frame = carrier.frame().await;
    assert!(matches!(frame, ServerFrame::Event { .. }));
    assert!(!serde_json::to_string(&frame).unwrap().contains("hidden"));
    carrier.sessions.close(carrier.identity.id);
    assert!(
        carrier.lines.next_line().await.unwrap().is_none(),
        "revocation closes output immediately"
    );
}

#[tokio::test]
async fn device_reads_and_subscriptions_redact_administration_and_end_on_revocation() {
    for role in [DeviceRole::Observer, DeviceRole::Operator] {
        let mut carrier = Carrier::open(role).await;
        carrier.greet().await;
        assert_history_is_redacted(&mut carrier).await;
        assert_live_frames_are_redacted_until_revoked(&mut carrier).await;
        assert_eq!(carrier.ended().await, Ended::Revoked);
    }
}

/// An idle agent with an open question and a pending approval.
async fn agent_awaiting_an_answer(
    carrier: &Carrier,
) -> (
    aether_core::id::AgentId,
    u64,
    aether_core::tool::ApprovalRequest,
) {
    use aether_core::event::{EventDraft, EventKind};
    use aether_core::tool::{ApprovalRequest, ApprovalStatus, SideEffectClass};
    let spec = CreateAgentSpec {
        role: "worker".into(),
        start: false,
        ..Default::default()
    };
    let (agent, _) = carrier.runtime.create_agent(spec, Actor::user()).unwrap();
    let asked = EventKind::QuestionAsked {
        question: "Proceed?".into(),
    };
    let draft = EventDraft::new(Actor::agent(agent.id), asked).agent(agent.id);
    let question = carrier
        .runtime
        .apply(aether_store::Commit::new().event(draft))
        .unwrap()[0]
        .seq;
    let approval = ApprovalRequest {
        id: aether_core::id::ApprovalId::new(),
        agent_id: agent.id,
        task_id: None,
        call_id: aether_core::id::ToolCallId::new(),
        tool: "bash".into(),
        description: "test".into(),
        argument: Some("cargo test".into()),
        resources: vec![],
        side_effect: SideEffectClass::External,
        requested_at: aether_core::time::now(),
        status: ApprovalStatus::Pending,
    };
    let put = aether_store::Mutation::PutApproval(approval.clone());
    carrier
        .runtime
        .apply(aether_store::Commit::new().mutate(put))
        .unwrap();
    (agent.id, question, approval)
}

/// Send each command twice under one key: the second answer is the first.
async fn send_each_twice(carrier: &mut Carrier, commands: Vec<Command>) {
    assert!(!commands.is_empty(), "there is something to send");
    for (index, command) in commands.into_iter().enumerate() {
        let first = carrier
            .request(index as u64 * 2 + 2, command.clone())
            .await
            .unwrap();
        let again = carrier
            .request(index as u64 * 2 + 3, command)
            .await
            .unwrap();
        assert_eq!(again, first);
    }
}

fn assert_acted_as_this_device(
    carrier: &Carrier,
    agent: aether_core::id::AgentId,
    approval: &aether_core::tool::ApprovalRequest,
) {
    use aether_core::event::OperatorOrigin;
    use aether_core::inbox::InboxKind;
    use aether_core::tool::ApprovalStatus;
    let inbox = carrier.runtime.store().inbox(agent, 0).unwrap();
    assert_eq!(inbox.len(), 3, "each command took effect once");
    for item in inbox {
        let actor = match item.kind {
            InboxKind::Message { from, .. } => from,
            InboxKind::ApprovalResolved { by: Some(by), .. } => by,
            other => panic!("unexpected {other:?}"),
        };
        assert!(
            matches!(actor, Actor::User { origin: Some(OperatorOrigin::Device { device, ref name, .. }), .. } if device == carrier.identity.id && name == &carrier.identity.name)
        );
    }
    let decision = carrier
        .runtime
        .store()
        .approval(approval.id)
        .unwrap()
        .unwrap();
    assert!(matches!(
        decision.status,
        ApprovalStatus::Resolved {
            by: Actor::User {
                origin: Some(OperatorOrigin::Device { .. }),
                ..
            },
            ..
        }
    ));
}

#[tokio::test]
async fn device_messages_questions_and_decisions_preserve_actor_and_replay_once() {
    use aether_core::tool::ApprovalDecision;
    let mut carrier = Carrier::open(DeviceRole::Operator).await;
    carrier.greet().await;
    let (agent, question, approval) = agent_awaiting_an_answer(&carrier).await;
    let commands = vec![
        Command::ReplyToQuestion {
            to: agent,
            question_seq: question,
            content: "answer".into(),
            idempotency_key: Some("question-key".into()),
        },
        Command::SendMessage {
            to: agent,
            content: "message".into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: Some("message-key".into()),
        },
        Command::ResolveApproval {
            id: approval.id,
            decision: ApprovalDecision::Approved,
            note: None,
            remember: None,
            idempotency_key: Some("approval-key".into()),
        },
    ];
    send_each_twice(&mut carrier, commands).await;
    assert_acted_as_this_device(&carrier, agent, &approval);
    carrier.shutdown.cancel();
    carrier.ended().await;
}

#[tokio::test]
async fn approval_ceiling_applies_to_explicit_legacy_and_schedule_fields() {
    use aether_core::agent::ApprovalPolicy;
    let (daemon, _config) = daemon().await;
    let (parent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "parent".into(),
                start: false,
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let auth = AuthContext::Device(DeviceSession {
        identity: DeviceIdentity {
            id: DeviceId::new(),
            name: "phone".into(),
            role: DeviceRole::Operator,
        },
        closed: CancellationToken::new(),
    });
    let cases = [
        serde_json::json!({"cmd":"create_agent", "role":"child", "parent":parent.id, "approval":{"require_for":[]}}),
        serde_json::json!({"cmd":"create_agent", "role":"child", "parent":parent.id, "config":{"effort":"low", "approval":{"require_for":[]}}}),
    ];
    for case in cases {
        let command: Command = serde_json::from_value(case).unwrap();
        assert!(auth.authorize(&command).is_ok());
        assert_eq!(
            authz::guard(&daemon, &auth, &command).unwrap_err().code,
            ErrorCode::Forbidden
        );
        assert!(authz::guard(&daemon, &AuthContext::Local, &command).is_ok());
    }
    let rule = daemon
        .runtime
        .add_approval_rule(
            aether_core::tool::ApprovalDecision::Denied,
            aether_core::approval::RuleMatch::tool("bash"),
            aether_core::approval::RuleScope::Everywhere,
            Actor::user(),
            None,
        )
        .unwrap();
    let command = Command::RevokeApprovalRule {
        id: rule.id,
        reason: None,
    };
    assert_eq!(
        authz::guard(&daemon, &auth, &command).unwrap_err().code,
        ErrorCode::Forbidden
    );
    daemon.runtime.shutdown().await;
}

mod resource_limits;
