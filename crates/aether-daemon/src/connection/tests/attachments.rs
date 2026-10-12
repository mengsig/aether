//! Images on `send_message` at the daemon boundary: checked, stored once,
//! and carried by the message as a reference.
use super::*;
use aether_core::attachment::{ImageAttachment, MAX_IMAGE_BYTES, MAX_IMAGES_PER_MESSAGE};
use aether_core::id::AgentId;
use aether_core::inbox::InboxKind;
use aether_core::model::ContentBlock;
use base64::Engine as _;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot a real picture";

fn image(media_type: &str, bytes: &[u8], name: &str) -> ImageAttachment {
    ImageAttachment {
        media_type: media_type.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: Some(name.into()),
    }
}

fn send(to: AgentId, attachments: Vec<ImageAttachment>, key: Option<&str>) -> Command {
    Command::SendMessage {
        to,
        content: "what is this?".into(),
        subtree: false,
        attachments,
        idempotency_key: key.map(str::to_owned),
    }
}

fn root(daemon: &Daemon) -> AgentId {
    let (agent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    agent.id
}

/// The files the daemon has stored, by name.
fn stored(daemon: &Daemon) -> Vec<String> {
    let dir = daemon.runtime.attachment_dir().expect("attachment dir");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[tokio::test]
async fn a_wrong_type_an_oversized_image_and_too_many_images_are_refused_by_name() {
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    let mut oversized = PNG.to_vec();
    oversized.resize(MAX_IMAGE_BYTES + 1, 0);
    let refused = [
        (
            vec![image("image/png", b"%PDF-1.7 a document", "report.pdf")],
            "image 1 (report.pdf) is not a supported type",
        ),
        (
            vec![image("image/jpeg", PNG, "shot.jpg")],
            "is declared as image/jpeg but its bytes are image/png",
        ),
        (
            vec![image("image/png", &oversized, "huge.png")],
            "image 1 (huge.png) is too large",
        ),
        (
            vec![image("image/png", PNG, "shot.png"); MAX_IMAGES_PER_MESSAGE + 1],
            "too many images: 11 attached, at most 10 per message",
        ),
    ];
    for (attachments, named) in refused {
        let error = dispatch(&daemon, send(agent, attachments, None), Actor::user())
            .await
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
        assert!(error.message.contains(named), "{}", error.message);
    }
    assert!(
        daemon
            .runtime
            .store()
            .inbox(agent, 0)
            .expect("inbox")
            .is_empty(),
        "a refused image refuses the whole message"
    );
    assert_eq!(stored(&daemon), Vec::<String>::new(), "nothing was stored");
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_message_stores_its_image_once_and_carries_a_reference() {
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    for _ in 0..2 {
        dispatch(
            &daemon,
            send(agent, vec![image("image/png", PNG, "shot.png")], None),
            Actor::user(),
        )
        .await
        .expect("sent");
    }
    let files = stored(&daemon);
    assert_eq!(files.len(), 1, "one file per digest: {files:?}");
    assert!(files[0].ends_with(".png"), "{files:?}");

    let inbox = daemon.runtime.store().inbox(agent, 0).expect("inbox");
    assert_eq!(inbox.len(), 2);
    for item in &inbox {
        let InboxKind::Message { content, .. } = &item.kind else {
            panic!("a message: {item:?}");
        };
        let [
            ContentBlock::Text { text },
            ContentBlock::ImageRef(reference),
        ] = content.as_slice()
        else {
            panic!("the text, then the reference: {content:?}");
        };
        assert_eq!(text, "what is this?");
        assert_eq!(reference.media_type, "image/png");
        assert_eq!(reference.size_bytes, PNG.len() as u64);
        assert_eq!(reference.name.as_deref(), Some("shot.png"));
        assert_eq!(reference.file_name(), files[0]);
    }
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_retry_repeats_its_image_and_another_image_under_the_key_is_refused() {
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    let keyed = |bytes: &[u8]| {
        send(
            agent,
            vec![image("image/png", bytes, "shot.png")],
            Some("outbox-1"),
        )
    };
    let first = dispatch(&daemon, keyed(PNG), Actor::user())
        .await
        .expect("first");
    let retry = dispatch(&daemon, keyed(PNG), Actor::user())
        .await
        .expect("retry");
    assert_eq!(first, retry, "the retry is answered, not delivered again");
    assert_eq!(
        daemon.runtime.store().inbox(agent, 0).expect("inbox").len(),
        1
    );

    let mut other = PNG.to_vec();
    other.push(0);
    let conflict = dispatch(&daemon, keyed(&other), Actor::user())
        .await
        .expect_err("the same key on another image is other work");
    assert_eq!(conflict.code, ErrorCode::InvalidArgument);
    assert!(conflict.message.contains("outbox-1"), "{conflict:?}");
    daemon.runtime.shutdown().await;
}

fn device(role: aether_protocol::remote::DeviceRole) -> AuthContext {
    AuthContext::Device(aether_remote::DeviceSession {
        identity: aether_protocol::remote::DeviceIdentity {
            id: aether_protocol::remote::DeviceId::new(),
            name: "test phone".into(),
            role,
        },
        closed: CancellationToken::new(),
    })
}

async fn run(daemon: &Daemon, auth: &AuthContext, command: Command) -> Result<Reply, ErrorReply> {
    let by = auth.actor(None, "test".into());
    dispatch_version(daemon, command, by, auth).await
}

/// Images do not change who may send: an observer is refused before any
/// image is looked at, and an operator device's message carries its image.
#[tokio::test]
async fn only_an_operator_device_sends_a_message_with_images() {
    use aether_protocol::remote::DeviceRole;
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    let with_image = || send(agent, vec![image("image/png", PNG, "shot.png")], None);

    let refused = run(&daemon, &device(DeviceRole::Observer), with_image())
        .await
        .expect_err("an observer changes nothing");
    assert_eq!(refused.code, ErrorCode::Forbidden, "{refused:?}");
    assert_eq!(stored(&daemon), Vec::<String>::new(), "nothing was stored");

    let sent = run(&daemon, &device(DeviceRole::Operator), with_image()).await;
    assert!(matches!(sent, Ok(Reply::MessageSent { .. })), "{sent:?}");
    let inbox = daemon.runtime.store().inbox(agent, 0).expect("inbox");
    assert_eq!(inbox.len(), 1, "only the operator's message arrived");
    let InboxKind::Message { from, content, .. } = &inbox[0].kind else {
        panic!("a message: {:?}", inbox[0]);
    };
    assert!(from.label().contains("test phone"), "{}", from.label());
    assert!(
        matches!(content.as_slice(), [_, ContentBlock::ImageRef(_)]),
        "{content:?}"
    );
    daemon.runtime.shutdown().await;
}

/// The ledger keys a message on its images too: the lost send resolves as
/// executed, and the same key on another image is not that submission.
#[tokio::test]
async fn a_lost_reply_to_a_message_with_images_is_resolved_from_the_ledger() {
    use aether_protocol::SubmissionResolution;
    let (daemon, _config_dir) = daemon().await;
    let agent = root(&daemon);
    let horizon =
        crate::connection::resolution::greeting_horizon(&daemon, aether_protocol::PROTOCOL_VERSION)
            .expect("a horizon");
    let resolving = |command: Command| Command::ResolveSubmission {
        command: Box::new(command),
        horizon: Some(horizon.clone()),
    };
    let keyed = |bytes: &[u8], key: &str| {
        send(
            agent,
            vec![image("image/png", bytes, "shot.png")],
            Some(key),
        )
    };

    let never_sent = run(
        &daemon,
        &AuthContext::Local,
        resolving(keyed(PNG, "outbox-0")),
    )
    .await;
    assert!(
        matches!(
            never_sent,
            Ok(Reply::SubmissionResolved(
                SubmissionResolution::AbsentSafe { .. }
            ))
        ),
        "{never_sent:?}"
    );
    assert_eq!(
        stored(&daemon),
        Vec::<String>::new(),
        "resolving stores nothing"
    );

    let first = run(&daemon, &AuthContext::Local, keyed(PNG, "outbox-1"))
        .await
        .expect("the send whose reply was lost");
    let resolved = run(
        &daemon,
        &AuthContext::Local,
        resolving(keyed(PNG, "outbox-1")),
    )
    .await;
    let Ok(Reply::SubmissionResolved(SubmissionResolution::Executed { reply, .. })) = resolved
    else {
        panic!("a held receipt resolves as executed: {resolved:?}");
    };
    assert_eq!(*reply, first);

    let mut other = PNG.to_vec();
    other.push(0);
    let conflict = run(
        &daemon,
        &AuthContext::Local,
        resolving(keyed(&other, "outbox-1")),
    )
    .await
    .expect_err("another image under the key is other work");
    assert_eq!(conflict.code, ErrorCode::InvalidArgument, "{conflict:?}");
    assert_eq!(
        daemon.runtime.store().inbox(agent, 0).expect("inbox").len(),
        1,
        "resolving delivered nothing"
    );
    daemon.runtime.shutdown().await;
}
