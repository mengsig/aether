//! `resolve_submission` over the daemon's own seam: the frame's key, the
//! caller's namespace, the checks sending would pass, and the three answers.

use super::*;
use crate::connection::resolution::greeting_horizon;
use aether_core::id::AgentId;
use aether_protocol::remote::{DeviceId, DeviceIdentity, DeviceRole};
use aether_protocol::{SubmissionResolution, UnprovenAbsence};
use aether_remote::DeviceSession;

fn device(role: DeviceRole) -> AuthContext {
    AuthContext::Device(DeviceSession {
        identity: DeviceIdentity {
            id: DeviceId::new(),
            name: "test phone".into(),
            role,
        },
        closed: CancellationToken::new(),
    })
}

fn send(to: AgentId, content: &str, key: &str) -> Command {
    Command::SendMessage {
        to,
        content: content.into(),
        subtree: false,
        attachments: Vec::new(),
        idempotency_key: Some(key.into()),
    }
}

fn resolving(command: &Command, horizon: Option<&str>) -> Command {
    Command::ResolveSubmission {
        command: Box::new(command.clone()),
        horizon: horizon.map(str::to_owned),
    }
}

async fn run(daemon: &Daemon, auth: &AuthContext, command: Command) -> Result<Reply, ErrorReply> {
    let by = auth.actor(None, "test".into());
    dispatch_version(daemon, command, by, auth).await
}

async fn resolved(
    daemon: &Daemon,
    auth: &AuthContext,
    command: &Command,
    horizon: &str,
) -> SubmissionResolution {
    match run(daemon, auth, resolving(command, Some(horizon))).await {
        Ok(Reply::SubmissionResolved(resolution)) => resolution,
        other => panic!("expected a resolution, got {other:?}"),
    }
}

fn horizon(daemon: &Daemon) -> String {
    greeting_horizon(daemon, aether_protocol::PROTOCOL_VERSION)
        .expect("a protocol 18 client is greeted with a horizon")
}

fn worker(daemon: &Daemon) -> AgentId {
    let spec = CreateAgentSpec {
        role: "worker".into(),
        start: false,
        ..Default::default()
    };
    let (agent, _) = daemon
        .runtime
        .create_agent(spec, Actor::user())
        .expect("agent");
    agent.id
}

fn deliveries(daemon: &Daemon, agent: AgentId) -> usize {
    let inbox = daemon.runtime.store().inbox(agent, 0).expect("inbox");
    inbox.len()
}

#[tokio::test]
async fn an_executed_submission_is_answered_with_the_reply_a_duplicate_gets() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    let sent_under = horizon(&daemon);
    let lost = send(agent, "ship it", "outbox-1");
    let first = run(&daemon, &AuthContext::Local, lost.clone())
        .await
        .expect("the send whose reply was lost");

    let resolution = resolved(&daemon, &AuthContext::Local, &lost, &sent_under).await;
    let SubmissionResolution::Executed {
        reply,
        horizon: now,
    } = resolution
    else {
        panic!("a held receipt resolves as executed: {resolution:?}");
    };
    let duplicate = run(&daemon, &AuthContext::Local, lost)
        .await
        .expect("retry");
    assert_eq!(*reply, first);
    assert_eq!(*reply, duplicate, "exactly what a duplicate is answered");
    assert_eq!(now, horizon(&daemon));
    assert_eq!(deliveries(&daemon, agent), 1, "resolving delivered nothing");
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn an_absent_submission_is_safe_to_resend_and_the_resend_runs_once() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    let sent_under = horizon(&daemon);
    let lost = send(agent, "ship it", "outbox-2");

    let resolution = resolved(&daemon, &AuthContext::Local, &lost, &sent_under).await;
    assert!(
        matches!(resolution, SubmissionResolution::AbsentSafe { .. }),
        "{resolution:?}"
    );
    assert_eq!(deliveries(&daemon, agent), 0, "resolving executes nothing");

    let resent = run(&daemon, &AuthContext::Local, lost.clone())
        .await
        .expect("the same key and payload");
    let again = resolved(&daemon, &AuthContext::Local, &lost, &sent_under).await;
    assert!(
        matches!(&again, SubmissionResolution::Executed { reply, .. } if **reply == resent),
        "{again:?}"
    );
    assert_eq!(deliveries(&daemon, agent), 1);
    daemon.runtime.shutdown().await;
}

/// The cap evicted the receipt of a command that ran. The answer must not be
/// "safe to resend", and resolving must not run it again.
#[tokio::test]
async fn a_submission_whose_receipt_was_evicted_is_unknown() {
    let store = aether_store::SqliteStore::open_in_memory()
        .expect("store")
        .with_submission_limits(2, 1);
    let (daemon, _config) = daemon_with_store(Arc::new(store)).await;
    let agent = worker(&daemon);
    let sent_under = horizon(&daemon);
    let lost = send(agent, "ship it", "evicted");
    run(&daemon, &AuthContext::Local, lost.clone())
        .await
        .expect("it ran");
    for i in 0..3 {
        run(
            &daemon,
            &AuthContext::Local,
            send(agent, "more", &format!("filler-{i}")),
        )
        .await
        .expect("filler");
    }

    let resolution = resolved(&daemon, &AuthContext::Local, &lost, &sent_under).await;
    let SubmissionResolution::Unknown { reason, .. } = resolution else {
        panic!("an evicted receipt proves nothing: {resolution:?}");
    };
    assert_eq!(reason, UnprovenAbsence::Forgotten);
    assert_eq!(deliveries(&daemon, agent), 4, "and nothing ran again");
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_request_without_a_usable_horizon_is_never_told_it_is_safe() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    let lost = send(agent, "ship it", "outbox-3");

    let none = run(&daemon, &AuthContext::Local, resolving(&lost, None)).await;
    assert!(
        matches!(
            none,
            Ok(Reply::SubmissionResolved(SubmissionResolution::Unknown {
                reason: UnprovenAbsence::NoHorizon,
                ..
            }))
        ),
        "{none:?}"
    );
    let foreign = resolved(
        &daemon,
        &AuthContext::Local,
        &lost,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV:0",
    )
    .await;
    assert!(
        matches!(
            foreign,
            SubmissionResolution::Unknown {
                reason: UnprovenAbsence::ForeignHorizon,
                ..
            }
        ),
        "{foreign:?}"
    );
    let malformed = run(
        &daemon,
        &AuthContext::Local,
        resolving(&lost, Some("not a horizon")),
    )
    .await
    .expect_err("a horizon this daemon never printed");
    assert_eq!(malformed.code, ErrorCode::InvalidArgument);
    daemon.runtime.shutdown().await;
}

/// A key is the caller's own: the same key and payload from another device,
/// or from the host's socket, is a different submission entirely.
#[tokio::test]
async fn a_resolution_reads_only_the_callers_own_namespace() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    let sent_under = horizon(&daemon);
    let (phone, tablet) = (device(DeviceRole::Operator), device(DeviceRole::Operator));
    let lost = send(agent, "ship it", "same-key");
    let sent = run(&daemon, &phone, lost.clone()).await.expect("phone");

    let own = resolved(&daemon, &phone, &lost, &sent_under).await;
    assert!(
        matches!(&own, SubmissionResolution::Executed { reply, .. } if **reply == sent),
        "{own:?}"
    );
    for (who, other) in [
        ("another device", &tablet),
        ("the host", &AuthContext::Local),
    ] {
        let theirs = resolved(&daemon, other, &lost, &sent_under).await;
        assert!(
            matches!(theirs, SubmissionResolution::AbsentSafe { .. }),
            "{who} sent nothing under that key: {theirs:?}"
        );
    }

    // And the other way: the host's key says nothing about a device's.
    let hosts = send(agent, "from the host", "host-key");
    run(&daemon, &AuthContext::Local, hosts.clone())
        .await
        .expect("host");
    let seen = resolved(&daemon, &phone, &hosts, &sent_under).await;
    assert!(
        matches!(seen, SubmissionResolution::AbsentSafe { .. }),
        "{seen:?}"
    );
    assert_eq!(deliveries(&daemon, agent), 2);
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_resolution_is_refused_wherever_sending_the_command_would_be() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    let lost = send(agent, "ship it", "outbox-4");
    run(&daemon, &AuthContext::Local, lost.clone())
        .await
        .expect("it ran");
    let sent_under = horizon(&daemon);
    let ask = |command: &Command| resolving(command, Some(&sent_under));

    let observer = run(&daemon, &device(DeviceRole::Observer), ask(&lost)).await;
    assert_eq!(observer.expect_err("observer").code, ErrorCode::Forbidden);

    let host_only = Command::AddApprovalRule(aether_protocol::NewApprovalRule {
        decision: aether_core::tool::ApprovalDecision::Approved,
        matcher: Default::default(),
        scope: aether_core::approval::RuleScope::Everywhere,
        note: None,
        idempotency_key: Some("rule-1".into()),
    });
    let operator = run(&daemon, &device(DeviceRole::Operator), ask(&host_only)).await;
    assert_eq!(operator.expect_err("host only").code, ErrorCode::Forbidden);

    let unkeyed = Command::Interrupt {
        agent,
        idempotency_key: None,
    };
    for (what, command) in [
        ("an unkeyed command", ask(&unkeyed)),
        ("a read", ask(&Command::Status)),
        ("a resolution of a resolution", ask(&ask(&lost))),
        (
            "the key on other work",
            ask(&send(agent, "edited", "outbox-4")),
        ),
    ] {
        let refused = run(&daemon, &AuthContext::Local, command)
            .await
            .expect_err(what);
        assert_eq!(
            refused.code,
            ErrorCode::InvalidArgument,
            "{what}: {refused:?}"
        );
    }
    assert_eq!(deliveries(&daemon, agent), 1);
    daemon.runtime.shutdown().await;
}

/// The horizon rides the welcome only for a client new enough to keep it.
#[tokio::test]
async fn only_a_protocol_18_client_is_greeted_with_a_horizon() {
    let (daemon, _config) = daemon().await;
    let agent = worker(&daemon);
    assert_eq!(greeting_horizon(&daemon, 17), None);
    let before = horizon(&daemon);
    run(&daemon, &AuthContext::Local, send(agent, "one", "outbox-5"))
        .await
        .expect("send");
    let after = horizon(&daemon);
    assert_ne!(before, after, "the horizon counts every receipt recorded");
    let (epoch, count) = after.rsplit_once(':').expect("epoch:count");
    assert_eq!(
        before,
        format!("{epoch}:{}", count.parse::<u64>().expect("count") - 1)
    );
    daemon.runtime.shutdown().await;
}
