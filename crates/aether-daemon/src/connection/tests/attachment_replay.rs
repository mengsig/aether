//! A keyed `send_message` with images that was already delivered is answered
//! from its receipt: attachment storage is neither read nor written for it.
use super::attachments::{PNG, image, root, run, stored};
use super::*;
use aether_core::id::AgentId;
use aether_core::inbox::InboxKind;
use aether_core::model::ContentBlock;
use aether_protocol::SubmissionResolution;

/// The PNG every test sends first, and others no two of which are alike.
fn png(variant: u8) -> Vec<u8> {
    [PNG, &[variant]].concat()
}

fn keyed(to: AgentId, bytes: &[u8], key: &str, subtree: bool) -> Command {
    Command::SendMessage {
        to,
        content: "what is this?".into(),
        subtree,
        attachments: vec![image("image/png", bytes, "shot.png")],
        idempotency_key: Some(key.into()),
    }
}

async fn send(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    run(daemon, &AuthContext::Local, command).await
}

/// What `resolve_submission` says became of `command`.
async fn resolve(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    let horizon =
        crate::connection::resolution::greeting_horizon(daemon, aether_protocol::PROTOCOL_VERSION)
            .expect("a horizon");
    let resolving = Command::ResolveSubmission {
        command: Box::new(command),
        horizon: Some(horizon),
    };
    send(daemon, resolving).await
}

/// The reply a resolution replays from a held receipt.
fn executed(resolved: Result<Reply, ErrorReply>) -> Reply {
    let Ok(Reply::SubmissionResolved(SubmissionResolution::Executed { reply, .. })) = resolved
    else {
        panic!("a held receipt resolves as executed: {resolved:?}");
    };
    *reply
}

/// The stored files the messages in `agent`'s inbox refer to, one per message.
fn referenced(daemon: &Daemon, agent: AgentId) -> Vec<String> {
    let inbox = daemon.runtime.store().inbox(agent, 0).expect("inbox");
    inbox
        .iter()
        .map(|item| {
            let InboxKind::Message { content, .. } = &item.kind else {
                panic!("a message: {item:?}");
            };
            let [_, ContentBlock::ImageRef(reference)] = content.as_slice() else {
                panic!("the text, then the reference: {content:?}");
            };
            reference.file_name()
        })
        .collect()
}

/// Put a plain file where the attachment directory was: nothing can be stored.
fn block_storage(daemon: &Daemon) {
    let dir = daemon.runtime.attachment_dir().expect("attachment dir");
    std::fs::remove_dir_all(dir).expect("remove the attachment directory");
    std::fs::write(dir, b"not a directory").expect("a file in its place");
    assert!(dir.is_file());
}

/// A root with one child, and the keyed image send that reached `recipients`.
struct Delivered {
    daemon: Daemon,
    _config_dir: tempfile::TempDir,
    recipients: Vec<AgentId>,
    command: Command,
    first: Reply,
}

async fn delivered(subtree: bool) -> Delivered {
    let (daemon, config_dir) = daemon().await;
    let parent = root(&daemon);
    let spec = CreateAgentSpec {
        role: "child".into(),
        parent: Some(parent),
        start: false,
        config: Some(aether_core::agent::AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (child, _) = daemon
        .runtime
        .create_agent(spec, Actor::user())
        .expect("child");
    let command = keyed(parent, &png(0), "outbox-1", subtree);
    let first = send(&daemon, command.clone()).await.expect("delivered");
    let recipients = match &first {
        Reply::MessageSent { .. } if !subtree => vec![parent],
        Reply::MessagesSent { deliveries } if subtree => {
            deliveries.iter().map(|delivery| delivery.agent).collect()
        }
        other => panic!("not the reply of this send: {other:?}"),
    };
    if subtree {
        assert_eq!(recipients, [parent, child.id], "the whole subtree heard it");
    }
    assert_eq!(stored(&daemon).len(), 1, "the image is stored once");
    Delivered {
        daemon,
        _config_dir: config_dir,
        recipients,
        command,
        first,
    }
}

impl Delivered {
    /// Resolving and retrying both answer the first reply and deliver nothing.
    async fn assert_answered_from_the_receipt(&self) {
        let resolved = executed(resolve(&self.daemon, self.command.clone()).await);
        assert_eq!(resolved, self.first, "resolved from the receipt");
        let retried = send(&self.daemon, self.command.clone()).await;
        assert_eq!(
            retried.as_ref(),
            Ok(&self.first),
            "retried from the receipt"
        );
        for recipient in &self.recipients {
            let inbox = self.daemon.runtime.store().inbox(*recipient, 0);
            assert_eq!(inbox.expect("inbox").len(), 1, "one delivery each");
        }
    }
}

#[tokio::test]
async fn a_removed_image_is_not_stored_again_by_a_resolution_or_a_retry() {
    for subtree in [false, true] {
        let sent = delivered(subtree).await;
        let dir = sent.daemon.runtime.attachment_dir().expect("dir");
        let file = dir.join(&stored(&sent.daemon)[0]);
        std::fs::remove_file(&file).expect("remove the stored image");

        sent.assert_answered_from_the_receipt().await;
        assert_eq!(
            stored(&sent.daemon),
            Vec::<String>::new(),
            "subtree {subtree}: a replay writes nothing"
        );
        sent.daemon.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_delivered_send_is_answered_while_nothing_can_be_stored() {
    for subtree in [false, true] {
        let sent = delivered(subtree).await;
        block_storage(&sent.daemon);

        sent.assert_answered_from_the_receipt().await;

        // The storage really is unusable, and a new send says so.
        let to = sent.recipients[0];
        let refused = send(&sent.daemon, keyed(to, &png(0), "outbox-2", subtree))
            .await
            .expect_err("a new image cannot be stored");
        assert_eq!(refused.code, ErrorCode::Internal, "{refused:?}");
        assert!(
            refused.message.contains("could not store an image"),
            "{refused:?}"
        );
        let inbox = sent.daemon.runtime.store().inbox(to, 0).expect("inbox");
        assert_eq!(inbox.len(), 1, "an image that is not stored is not sent");
        sent.daemon.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn another_image_under_a_delivered_key_is_refused_and_not_stored() {
    let sent = delivered(false).await;
    let agent = sent.recipients[0];
    let other = keyed(agent, &png(1), "outbox-1", false);
    let before = stored(&sent.daemon);

    let retried = send(&sent.daemon, other.clone())
        .await
        .expect_err("the same key on another image is other work");
    let resolved = resolve(&sent.daemon, other)
        .await
        .expect_err("and resolves as that refusal");
    for conflict in [retried, resolved] {
        assert_eq!(conflict.code, ErrorCode::InvalidArgument, "{conflict:?}");
        assert!(conflict.message.contains("outbox-1"), "{conflict:?}");
    }
    assert_eq!(
        stored(&sent.daemon),
        before,
        "the other image is not stored"
    );
    assert_eq!(referenced(&sent.daemon, agent), before);
    sent.daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_send_to_an_unknown_agent_stores_nothing() {
    let (daemon, _config_dir) = daemon().await;
    let nobody = AgentId::new();
    for subtree in [false, true] {
        let refused = send(&daemon, keyed(nobody, &png(0), "outbox-1", subtree))
            .await
            .expect_err("nobody to deliver to");
        assert_eq!(refused.code, ErrorCode::NotFound, "{refused:?}");
    }
    assert_eq!(stored(&daemon), Vec::<String>::new());
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn two_identical_sends_at_once_deliver_once_and_store_one_file() {
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    for round in 0..8u8 {
        let command = keyed(agent, &png(round), &format!("outbox-{round}"), false);
        let (one, other) = tokio::join!(send(&daemon, command.clone()), send(&daemon, command));
        assert_eq!(one.expect("sent"), other.expect("sent"), "one message");

        let delivered = referenced(&daemon, agent);
        assert_eq!(delivered.len(), usize::from(round) + 1, "delivered once");
        let mut files = stored(&daemon);
        files.sort();
        let mut expected = delivered;
        expected.sort();
        assert_eq!(files, expected, "every stored file belongs to a message");
    }
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn two_images_at_once_under_one_key_store_only_the_delivered_one() {
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    for round in 0..8u8 {
        let key = format!("outbox-{round}");
        let (one, other) = tokio::join!(
            send(&daemon, keyed(agent, &png(2 * round), &key, false)),
            send(&daemon, keyed(agent, &png(2 * round + 1), &key, false)),
        );
        let (sent, conflict) = match (one, other) {
            (Ok(sent), Err(conflict)) | (Err(conflict), Ok(sent)) => (sent, conflict),
            both => panic!("one is delivered and one refused: {both:?}"),
        };
        assert!(matches!(sent, Reply::MessageSent { .. }), "{sent:?}");
        assert_eq!(conflict.code, ErrorCode::InvalidArgument, "{conflict:?}");
        assert!(conflict.message.contains(&key), "{conflict:?}");

        let mut files = stored(&daemon);
        files.sort();
        let mut expected = referenced(&daemon, agent);
        expected.sort();
        assert_eq!(files, expected, "the refused image left no file behind");
    }
    daemon.runtime.shutdown().await;
}
