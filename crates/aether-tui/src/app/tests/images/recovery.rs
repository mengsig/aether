use super::*;

fn message_request(effects: Vec<Effect>) -> (Command, Then) {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: command @ Command::SendMessage { .. },
                then,
            } => Some((command, then)),
            _ => None,
        })
        .expect("message request")
}

fn image_rejection() -> aether_client::ClientError {
    aether_client::ClientError::Protocol(aether_protocol::ErrorReply::new(
        aether_protocol::ErrorCode::Unimplemented,
        "sending images requires protocol 19 and the host speaks 18; update Aether on the host. Nothing was sent",
    ))
}

#[test]
fn rejected_image_only_message_returns_to_the_composer() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    attach(&mut app, png("shot.png"));
    let (_, then) = message_request(app.reduce(key(KeyCode::Enter)));
    assert!(app.input.images.is_empty());
    let effects = app.reduce(AppEvent::Reply {
        then,
        result: Err(image_rejection()),
    });
    assert!(sent_none(&effects));
    assert_eq!(app.input.images, [png("shot.png")]);
    let flash = app.live_flash().expect("rejection");
    assert!(flash.contains("protocol 19"));
    assert!(flash.contains("/image --clear"));
}

#[test]
fn refused_text_message_returns_without_an_image_hint() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    app.input.set_text("hello");
    let (_, then) = message_request(app.reduce(key(KeyCode::Enter)));
    assert!(app.input.text.is_empty());
    let refusal = aether_client::ClientError::Protocol(aether_protocol::ErrorReply::new(
        aether_protocol::ErrorCode::InvalidTransition,
        "that agent has finished",
    ));
    let effects = app.reduce(AppEvent::Reply {
        then,
        result: Err(refusal),
    });
    assert!(sent_none(&effects));
    assert_eq!(app.input.text, "hello");
    let flash = app.live_flash().expect("refusal");
    assert!(flash.contains("back in the draft"), "{flash}");
    assert!(!flash.contains("image"), "{flash}");
}

#[test]
fn rejected_message_restores_the_original_agents_parked_draft() {
    let (mut app, first) = app_viewing(AgentStatus::Ready);
    attach(&mut app, png("shot.png"));
    app.input.set_text("original");
    let (_, then) = message_request(app.reduce(key(KeyCode::Enter)));
    app.input.set_text("typed since");
    let second = AgentId::new();
    app.select(second);
    app.input.set_text("second draft");
    app.reduce(AppEvent::Reply {
        then,
        result: Err(image_rejection()),
    });
    assert_eq!(app.input.text, "second draft");
    assert!(app.input.images.is_empty());
    app.select(first);
    assert_eq!(app.input.text, "original\ntyped since");
    assert_eq!(app.input.images, [png("shot.png")]);
}

#[test]
fn uncertain_send_restores_the_draft_without_resending() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    attach(&mut app, png("shot.png"));
    app.input.set_text("original");
    let (_, then) = message_request(app.reduce(key(KeyCode::Enter)));
    app.input.set_text("typed since");
    let effects = app.reduce(AppEvent::Reply {
        then,
        result: Err(aether_client::ClientError::Disconnected),
    });
    assert!(sent_none(&effects));
    assert_eq!(app.input.text, "original\ntyped since");
    assert_eq!(app.input.images, [png("shot.png")]);
    app.reduce(AppEvent::Disconnected { because: None });
    assert!(
        app.live_flash()
            .expect("uncertain")
            .contains("delivery uncertain: check the transcript before sending it again")
    );
}

#[test]
fn no_model_for_an_abandoned_start_leaves_the_current_draft_alone() {
    let mut app = app_with_status(TuiOptions::default());
    attach(&mut app, png("shot.png"));
    app.input.set_text("first request");
    let then = start_then(&app.reduce(key(KeyCode::Enter)));
    let other = AgentId::new();
    app.select(other);
    assert!(app.awaited_start().is_none());
    app.input.set_text("other draft");
    app.reduce(AppEvent::Reply {
        then,
        result: Err(aether_client::ClientError::Protocol(
            aether_protocol::ErrorReply::new(
                aether_protocol::ErrorCode::NoModel,
                "no model configured",
            ),
        )),
    });
    assert_eq!(app.input.text, "other draft");
    assert!(app.input.images.is_empty());
    assert!(app.root_starts.is_empty());
    let flash = app.live_flash().expect("refusal");
    assert!(flash.contains("/login to sign in"), "{flash}");
}

#[test]
fn no_model_restores_queued_text_and_images_and_keeps_the_hint_until_a_key() {
    for defaults in [false, true] {
        let mut app = app_with_status(TuiOptions::default());
        if defaults {
            app.status = None;
        }
        attach(&mut app, png("shot.png"));
        app.input.set_text("first request");
        let then = start_then(&app.reduce(key(KeyCode::Enter)));
        app.input.set_text("second request");
        assert!(app.reduce(key(KeyCode::Enter)).is_empty());
        app.input.set_text("typed since");
        app.reduce(AppEvent::Reply {
            then,
            result: Err(aether_client::ClientError::Protocol(
                aether_protocol::ErrorReply::new(
                    aether_protocol::ErrorCode::NoModel,
                    "no model configured",
                ),
            )),
        });
        assert_eq!(app.input.text, "first request\nsecond request\ntyped since");
        assert_eq!(app.input.images, [png("shot.png")]);
        app.now += chrono::Duration::seconds(10);
        assert!(
            app.live_flash()
                .expect("sticky hint")
                .contains("/login to sign in")
        );
        app.reduce(key(KeyCode::Char('x')));
        assert!(app.live_flash().is_none());
        assert!(app.input.text.ends_with("typed sincex"));
        app.status = Some(crate::ui::tests_support::quiet_status());
        let effects = app.reduce(key(KeyCode::Enter));
        let fresh = AgentId::new();
        let outgoing = app.reduce(AppEvent::Reply {
            then: start_then(&effects),
            result: Ok(Reply::Agent(agent(fresh, "assistant"))),
        });
        let (_, text, images) = sent(&outgoing);
        assert_eq!(text, "first request\nsecond request\ntyped sincex");
        assert_eq!(images.len(), 1);
    }
}

#[tokio::test]
async fn protocol_18_refuses_an_image_before_any_send_frame_and_restores_it() {
    use aether_client::{Client, Transport};
    use aether_protocol::{ServerFrame, Welcome};
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    attach(&mut app, png("shot.png"));
    let (command, then) = message_request(app.reduce(key(KeyCode::Enter)));
    let (transport, mut peer) = Transport::memory(4);
    let daemon = tokio::spawn(async move {
        let hello = peer.incoming.recv().await.expect("hello");
        assert!(matches!(hello.command, Command::Hello(_)));
        peer.outgoing
            .send(ServerFrame::Reply {
                id: hello.id,
                result: Ok(Reply::Welcome(Welcome {
                    protocol_version: aether_protocol::PROTOCOL_VERSION,
                    daemon_protocol_version: Some(18),
                    daemon_version: "fixture".into(),
                    instance_id: "fixture".into(),
                    schema_version: 1,
                    pid: 1,
                    features: vec![],
                    min_protocol_version: None,
                    idle_timeout_secs: None,
                    device: None,
                    submission_horizon: None,
                })),
            })
            .await
            .expect("welcome");
        peer
    });
    let client = Client::open(transport).await.expect("connected");
    let mut peer = daemon.await.expect("daemon");
    let result = client.request(command).await;
    assert!(
        matches!(&result, Err(aether_client::ClientError::Protocol(error)) if error.code == aether_protocol::ErrorCode::Unimplemented)
    );
    assert!(matches!(
        peer.incoming.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    app.reduce(AppEvent::Reply { then, result });
    assert_eq!(app.input.images, [png("shot.png")]);
    assert!(
        app.live_flash()
            .expect("rejection")
            .contains("host speaks 18")
    );
}
