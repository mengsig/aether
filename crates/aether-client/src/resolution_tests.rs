#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::Transport;
use aether_core::id::{AgentId, MessageId};
use aether_protocol::{ClientFrame, ErrorCode, ServerFrame};
use std::sync::Mutex;
use std::time::Duration;

const SENT_UNDER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV:3";
const GREETED_WITH: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV:9";

type Answer = std::result::Result<Reply, ErrorReply>;
type Seen = Arc<Mutex<Vec<Command>>>;

/// A host that answers each command from `script`, or says nothing at all
/// when the script does: the acknowledgement that gets lost.
fn host(
    mut peer: crate::MemoryPeer,
    script: impl Fn(&Command) -> Option<Answer> + Send + 'static,
) -> Seen {
    let seen = Seen::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(ClientFrame { id, command }) = peer.incoming.recv().await {
            log.lock().expect("log").push(command.clone());
            if let Some(result) = script(&command) {
                let _ = peer.outgoing.send(ServerFrame::Reply { id, result }).await;
            }
        }
    });
    seen
}

fn resolved(resolution: SubmissionResolution) -> Option<Answer> {
    Some(Ok(Reply::SubmissionResolved(resolution)))
}

fn absent_safe() -> Option<Answer> {
    resolved(SubmissionResolution::AbsentSafe {
        horizon: GREETED_WITH.into(),
        retained_since: aether_core::time::now(),
    })
}

fn unknown(reason: UnprovenAbsence) -> Option<Answer> {
    resolved(SubmissionResolution::Unknown {
        reason,
        horizon: GREETED_WITH.into(),
        retained_since: aether_core::time::now(),
    })
}

fn lost_send(key: &str) -> Command {
    Command::SendMessage {
        to: AgentId::new(),
        content: "ship it".into(),
        subtree: false,
        idempotency_key: Some(key.into()),
    }
}

fn asking(command: &Command, horizon: Option<&str>) -> Command {
    Command::ResolveSubmission {
        command: Box::new(command.clone()),
        horizon: horizon.map(str::to_owned),
    }
}

/// An SSH client that has just been greeted, before it adopts its outbox.
fn greeted(transport: Transport, horizon: Option<&str>) -> Client {
    let mut client =
        Client::from_transport(transport).with_request_timeout(Duration::from_millis(200));
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    std::mem::forget(cancelled);
    client.carrier = Some(Arc::new(crate::ssh::Carrier::test_carrier(cancel)));
    let mut welcome = crate::tests::welcome();
    welcome.submission_horizon = horizon.map(str::to_owned);
    welcome.daemon_protocol_version = Some(if horizon.is_some() { 18 } else { 17 });
    client.welcome = Some(welcome);
    client
}

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    /// An outbox holding `commands`, each first sent under [`SENT_UNDER`] and
    /// never answered.
    async fn holding(commands: &[Command]) -> Fixture {
        let directory = tempfile::tempdir().expect("directory");
        let outbox = Outbox::open(directory.path().into()).await.expect("outbox");
        for command in commands {
            outbox
                .record(command, Some(SENT_UNDER))
                .await
                .expect("record");
        }
        Fixture { directory }
    }

    async fn outbox(&self) -> Arc<Outbox> {
        let opened = Outbox::open(self.directory.path().into()).await;
        Arc::new(opened.expect("outbox"))
    }

    /// Connect: the client meets `script` and adopts the outbox.
    async fn connect(
        &self,
        horizon: Option<&str>,
        script: impl Fn(&Command) -> Option<Answer> + Send + 'static,
    ) -> (Result<()>, Client, Seen) {
        let (transport, peer) = Transport::memory(16);
        let seen = host(peer, script);
        let mut client = greeted(transport, horizon);
        let adopted = client.adopt_outbox(self.outbox().await).await;
        (adopted, client, seen)
    }

    async fn pending(&self) -> Vec<Unresolved> {
        self.outbox()
            .await
            .pending()
            .await
            .expect("pending")
            .entries
    }

    /// What the settled log says happened, oldest first.
    fn settled(&self) -> Vec<String> {
        let log = std::fs::read_to_string(self.directory.path().join("settled.jsonl"));
        log.unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("a log line"))
            .map(|line| line["settled"].as_str().expect("how").to_owned())
            .collect()
    }
}

fn seen(log: &Seen) -> Vec<Command> {
    log.lock().expect("log").clone()
}

#[tokio::test]
async fn an_executed_operation_is_retired_and_its_receipt_surfaced() {
    let lost = lost_send("ssh-executed");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let message = MessageId::new();
    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), move |_| {
            resolved(SubmissionResolution::Executed {
                reply: Box::new(Reply::MessageSent { message }),
                horizon: GREETED_WITH.into(),
            })
        })
        .await;

    adopted.expect("the connection is not held up");
    assert_eq!(
        seen(&log),
        [asking(&lost, Some(SENT_UNDER))],
        "asked under the horizon it was sent with, and never sent again"
    );
    assert_eq!(
        client.recovered_commands(),
        [(lost, Ok(Reply::MessageSent { message }))]
    );
    assert!(fx.pending().await.is_empty());
    assert_eq!(fx.settled(), ["executed"]);
}

#[tokio::test]
async fn an_operation_proven_absent_is_sent_again_with_the_same_key_and_payload() {
    let lost = lost_send("ssh-absent");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let message = MessageId::new();
    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), move |command| match command {
            Command::ResolveSubmission { .. } => absent_safe(),
            _ => Some(Ok(Reply::MessageSent { message })),
        })
        .await;

    adopted.expect("connected");
    assert_eq!(
        seen(&log),
        [asking(&lost, Some(SENT_UNDER)), lost.clone()],
        "the resend is the recorded command, key included"
    );
    assert_eq!(
        client.recovered_commands(),
        [(lost, Ok(Reply::MessageSent { message }))]
    );
    assert!(fx.pending().await.is_empty());
    assert_eq!(fx.settled(), ["resent"]);
}

/// The resend's own acknowledgement can be lost too. The entry stays exactly
/// as first written, so the next connection asks under the same horizon.
#[tokio::test]
async fn a_resend_that_is_not_answered_leaves_the_operation_as_it_was() {
    let lost = lost_send("ssh-twice-lost");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let before = fx.pending().await;
    let (adopted, _client, log) = fx
        .connect(Some(GREETED_WITH), |command| match command {
            Command::ResolveSubmission { .. } => absent_safe(),
            _ => None,
        })
        .await;

    assert!(
        matches!(adopted, Err(ClientError::DeliveryUnconfirmed(_))),
        "{adopted:?}"
    );
    assert_eq!(seen(&log).len(), 2);
    assert_eq!(fx.pending().await, before);
}

#[tokio::test]
async fn an_unknown_operation_stays_unconfirmed_without_holding_up_the_connection() {
    let lost = lost_send("ssh-unknown");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), |_| unknown(UnprovenAbsence::Forgotten))
        .await;

    adopted.expect("an unconfirmed operation does not refuse the connection");
    assert_eq!(
        seen(&log),
        [asking(&lost, Some(SENT_UNDER))],
        "never resent"
    );
    assert!(client.recovered_commands().is_empty());
    let unconfirmed = client.undecided().await.expect("undecided").unconfirmed;
    assert_eq!(unconfirmed.len(), 1);
    assert_eq!(unconfirmed[0].command, lost);
    assert_eq!(unconfirmed[0].horizon.as_deref(), Some(SENT_UNDER));
    assert_eq!(
        unconfirmed[0].unconfirmed.as_deref(),
        Some(unproven(UnprovenAbsence::Forgotten))
    );

    // New work goes out on the same connection regardless.
    let reply = client.request(Command::Status).await;
    assert!(
        matches!(reply, Err(ClientError::Protocol(_)) | Ok(_)),
        "{reply:?}"
    );
    assert_eq!(fx.settled(), ["unconfirmed"]);
}

/// Once the host has said it cannot tell, a later "never ran" - a rolled-back
/// store whose counter has grown past the horizon again - is not acted on.
#[tokio::test]
async fn an_unconfirmed_operation_is_never_resent_without_a_person() {
    let lost = lost_send("ssh-sticky");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let (first, _, _) = fx
        .connect(Some(GREETED_WITH), |_| {
            unknown(UnprovenAbsence::HorizonAhead)
        })
        .await;
    first.expect("connected");

    let (again, client, log) = fx.connect(Some(GREETED_WITH), |_| absent_safe()).await;
    again.expect("connected");
    assert_eq!(seen(&log), [asking(&lost, Some(SENT_UNDER))]);
    assert_eq!(
        client
            .undecided()
            .await
            .expect("undecided")
            .unconfirmed
            .len(),
        1
    );
    assert_eq!(fx.settled(), ["unconfirmed"], "and it is logged once");
}

/// A receipt found later still settles it: `executed` needs no one's leave.
#[tokio::test]
async fn an_unconfirmed_operation_is_retired_when_its_receipt_turns_up() {
    let lost = lost_send("ssh-found");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let (first, _, _) = fx.connect(None, |_| None).await;
    first.expect("connected");
    assert_eq!(fx.pending().await.len(), 1);

    let message = MessageId::new();
    let (again, client, _) = fx
        .connect(Some(GREETED_WITH), move |_| {
            resolved(SubmissionResolution::Executed {
                reply: Box::new(Reply::MessageSent { message }),
                horizon: GREETED_WITH.into(),
            })
        })
        .await;
    again.expect("connected");
    assert_eq!(client.recovered_commands().len(), 1);
    assert!(fx.pending().await.is_empty());
}

/// A protocol 17 host has no `resolve_submission`. Nothing is asked, nothing
/// is resent, and the connection is made.
#[tokio::test]
async fn an_older_host_leaves_every_operation_unconfirmed_and_still_connects() {
    let lost = [lost_send("ssh-old-1"), lost_send("ssh-old-2")];
    let fx = Fixture::holding(&lost).await;
    let (adopted, client, log) = fx.connect(None, |_| None).await;

    adopted.expect("an older host is not refused over its outbox");
    assert!(
        seen(&log).is_empty(),
        "nothing is asked of a host that cannot answer"
    );
    let unconfirmed = client.undecided().await.expect("undecided").unconfirmed;
    assert_eq!(unconfirmed.len(), 2);
    for entry in &unconfirmed {
        assert_eq!(entry.unconfirmed.as_deref(), Some(HOST_CANNOT_RESOLVE));
    }
}

#[tokio::test]
async fn a_host_that_refuses_to_resolve_leaves_the_operation_unconfirmed() {
    let lost = lost_send("ssh-refused");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let (adopted, client, _) = fx
        .connect(Some(GREETED_WITH), |_| {
            Some(Err(ErrorReply::new(ErrorCode::Forbidden, "not for you")))
        })
        .await;
    adopted.expect("connected");
    let unconfirmed = client.undecided().await.expect("undecided").unconfirmed;
    let reason = unconfirmed[0].unconfirmed.as_deref().expect("a reason");
    assert!(reason.contains("not for you"), "{reason}");
}

#[tokio::test]
async fn every_operator_choice_settles_the_operation_and_is_recorded() {
    let lost = [
        lost_send("ssh-deliver"),
        lost_send("ssh-discard"),
        lost_send("ssh-again"),
    ];
    let fx = Fixture::holding(&lost).await;
    let message = MessageId::new();
    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), move |command| match command {
            Command::ResolveSubmission { .. } => unknown(UnprovenAbsence::Forgotten),
            _ => Some(Ok(Reply::MessageSent { message })),
        })
        .await;
    adopted.expect("connected");
    assert_eq!(
        client
            .undecided()
            .await
            .expect("undecided")
            .unconfirmed
            .len(),
        3
    );
    log.lock().expect("log").clear();

    let delivered = client
        .settle("ssh-deliver", Settlement::MarkDelivered)
        .await;
    assert_eq!(delivered.expect("marked"), None);
    let discarded = client.settle("ssh-discard", Settlement::Discard).await;
    assert_eq!(discarded.expect("discarded"), None);
    assert!(seen(&log).is_empty(), "neither choice sends anything");

    let again = client.settle("ssh-again", Settlement::SendAgain).await;
    assert_eq!(again.expect("sent"), Some(Reply::MessageSent { message }));
    assert_eq!(seen(&log), [lost[2].clone()], "under the same key");

    assert!(fx.pending().await.is_empty());
    assert_eq!(
        fx.settled()[3..],
        ["marked_delivered", "discarded", "sent_again"]
    );
    let gone = client.settle("ssh-again", Settlement::Discard).await;
    assert!(
        matches!(gone, Err(ClientError::InvalidUsage(_))),
        "{gone:?}"
    );
}

/// Sending again is the one choice that needs the host. If it does not
/// answer, the operation is still unconfirmed and nothing is recorded.
#[tokio::test]
async fn sending_again_without_an_answer_keeps_the_operation_unconfirmed() {
    let lost = lost_send("ssh-silent");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let (adopted, client, _) = fx
        .connect(Some(GREETED_WITH), |command| match command {
            Command::ResolveSubmission { .. } => unknown(UnprovenAbsence::Forgotten),
            _ => None,
        })
        .await;
    adopted.expect("connected");

    let again = client.settle("ssh-silent", Settlement::SendAgain).await;
    assert!(
        matches!(again, Err(ClientError::DeliveryUnconfirmed(_))),
        "{again:?}"
    );
    assert_eq!(
        client
            .undecided()
            .await
            .expect("undecided")
            .unconfirmed
            .len(),
        1
    );
    assert_eq!(fx.settled(), ["unconfirmed"]);
}

/// A new send is written down under the horizon the connection was greeted
/// with, before it leaves, and retired by its reply.
#[tokio::test]
async fn a_new_send_is_recorded_under_the_greeting_horizon_until_it_is_answered() {
    let fx = Fixture::holding(&[]).await;
    let (adopted, client, log) = fx.connect(Some(GREETED_WITH), |_| None).await;
    adopted.expect("connected");

    let sending = client.request(lost_send("ssh-new")).await;
    assert!(
        matches!(sending, Err(ClientError::DeliveryUnconfirmed(_))),
        "{sending:?}"
    );
    let pending = fx.pending().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].horizon.as_deref(), Some(GREETED_WITH));
    assert_eq!(
        pending[0].unconfirmed, None,
        "the next connection resolves it"
    );
    assert_eq!(seen(&log), [pending[0].command.clone()]);
    assert!(
        client
            .undecided()
            .await
            .expect("undecided")
            .unconfirmed
            .is_empty()
    );
}

#[tokio::test]
async fn an_unreadable_outbox_file_neither_refuses_the_connection_nor_hides_good_entries() {
    let lost = lost_send("ssh-fine");
    let fx = Fixture::holding(std::slice::from_ref(&lost)).await;
    let garbage = fx.directory.path().join("garbage.json");
    std::fs::write(&garbage, b"{ not json").expect("undecodable entry");
    let message = MessageId::new();
    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), move |command| match command {
            Command::ResolveSubmission { .. } => resolved(SubmissionResolution::Executed {
                reply: Box::new(Reply::MessageSent { message }),
                horizon: GREETED_WITH.into(),
            }),
            _ => None,
        })
        .await;

    adopted.expect("an unreadable file does not refuse the connection");
    assert_eq!(seen(&log), [asking(&lost, Some(SENT_UNDER))]);
    let undecided = client.undecided().await.expect("undecided");
    assert!(undecided.unconfirmed.is_empty());
    assert_eq!(undecided.unreadable.len(), 1);
    assert_eq!(undecided.unreadable[0].path, garbage);

    let refused = client.settle("garbage", Settlement::SendAgain).await;
    assert!(
        matches!(refused, Err(ClientError::InvalidUsage(_))),
        "{refused:?}"
    );
    assert!(garbage.exists(), "a refused decision removes nothing");
    let discarded = client.settle("garbage", Settlement::Discard).await;
    assert_eq!(discarded.expect("discarded"), None);
    assert!(!garbage.exists());
    assert!(client.undecided().await.expect("undecided").is_empty());
}

/// Another process's send is in flight, so its entry is its own: asking the
/// host about it, or marking it, could resurrect it after it is answered.
#[tokio::test]
async fn an_entry_another_process_is_sending_is_left_alone_and_never_resurrected() {
    let fx = Fixture::holding(&[]).await;
    let sender = fx.outbox().await;
    let in_flight = sender
        .record(&lost_send("ssh-in-flight"), Some(SENT_UNDER))
        .await
        .expect("record");

    let (adopted, client, log) = fx
        .connect(Some(GREETED_WITH), |_| unknown(UnprovenAbsence::Forgotten))
        .await;
    adopted.expect("connected");
    assert!(seen(&log).is_empty(), "the host is not asked about it");
    assert_eq!(fx.pending().await.len(), 1);
    assert!(fx.pending().await[0].unconfirmed.is_none());

    sender.acknowledge(&in_flight).await.expect("answered");
    drop(in_flight);
    let (adopted, client_after, _) = fx.connect(Some(GREETED_WITH), |_| None).await;
    adopted.expect("connected");
    assert!(
        fx.pending().await.is_empty(),
        "an answered entry stays gone"
    );
    assert!(client.undecided().await.expect("undecided").is_empty());
    assert!(
        client_after
            .undecided()
            .await
            .expect("undecided")
            .is_empty()
    );
}
