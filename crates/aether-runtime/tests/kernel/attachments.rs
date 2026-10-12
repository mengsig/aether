//! Images on a message, native model path: durable state keeps a reference,
//! and the request sent to the model carries the image itself.
use super::*;
use aether_core::attachment::ImageAttachment;
use aether_runtime::attachments::message_content;
use base64::Engine as _;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot a real picture";

fn png_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(PNG)
}

fn screenshot() -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: png_base64(),
        name: Some("shot.png".into()),
    }
}

fn chat_spec(model: Option<ModelRef>) -> CreateAgentSpec {
    CreateAgentSpec {
        role: "assistant".into(),
        start: true,
        model,
        config: Some(AgentConfig {
            long_lived: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Send `text` with the screenshot and wait until the model answered it.
async fn send_with_screenshot(h: &Harness, agent: AgentId, text: &str) {
    let images = h
        .runtime
        .store_attachments(&[screenshot()])
        .expect("store the image");
    assert_eq!(images.len(), 1);
    h.runtime
        .send_message(agent, message_content(text.into(), images), Actor::user())
        .expect("send");
    h.wait_for("the model to be asked", || h.client.remaining() == 0)
        .await;
    h.wait_until(agent, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
}

/// Every image block in a request, as (media type, base64 payload).
fn images_in(request: &aether_core::model::ModelRequest) -> Vec<(String, String)> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Image { media_type, data } => Some((media_type.clone(), data.clone())),
            _ => None,
        })
        .collect()
}

fn text_in(request: &aether_core::model::ModelRequest) -> String {
    request
        .messages
        .iter()
        .map(aether_core::model::Message::text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_reference(request: &aether_core::model::ModelRequest) -> bool {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .any(|block| matches!(block, ContentBlock::ImageRef(_)))
}

#[tokio::test]
async fn a_native_request_carries_the_image_and_durable_state_only_its_reference() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let dir = attachments.path().join("images");
    let h = harness_configured(
        vec![ScriptedClient::text("a login form")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    send_with_screenshot(&h, agent.id, "what is on this screen?").await;

    let requests = h.client.requests();
    let request = requests.last().expect("one request");
    assert_eq!(
        images_in(request),
        vec![("image/png".to_owned(), png_base64())]
    );
    assert!(!has_reference(request), "a reference never reaches a model");
    let text = text_in(request);
    assert!(text.contains("what is on this screen?"), "{text}");
    let stored = std::fs::read_dir(&dir)
        .expect("attachment dir")
        .map(|entry| entry.expect("entry").path())
        .collect::<Vec<_>>();
    assert_eq!(stored.len(), 1, "stored once: {stored:?}");
    assert_eq!(std::fs::read(&stored[0]).expect("stored bytes"), PNG);
    assert!(
        text.contains(&format!(
            "[image: shot.png, {} B, stored at {}]",
            PNG.len(),
            stored[0].display()
        )),
        "the text names the stored file: {text}"
    );

    // Events and the transcript carry the reference, never the bytes.
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events");
    let recorded = serde_json::to_string(&events).expect("events as json");
    assert!(recorded.contains("\"image_ref\""), "{recorded}");
    assert!(!recorded.contains(&png_base64()), "bytes reached the log");
    let transcript = h.store.transcript(agent.id, 0).expect("transcript");
    assert!(transcript.iter().any(|entry| {
        entry
            .message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ImageRef(image) if image.name.as_deref() == Some("shot.png")))
    }));
}

#[tokio::test]
async fn a_stored_image_survives_a_restart_and_rides_along_with_the_history() {
    let state = tempfile::tempdir().expect("state dir");
    let dir = state.path().join("images");
    let database = state.path().join("aether.db");
    let open = || -> Arc<dyn Store> { Arc::new(SqliteStore::open(&database).expect("store")) };
    let first = harness_full(
        vec![ScriptedClient::text("a login form")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
        None,
        None,
        Some(open()),
        |_| {},
    )
    .await;
    let (agent, _) = first
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    first
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    send_with_screenshot(&first, agent.id, "what is on this screen?").await;
    first.runtime.shutdown().await;
    drop(first);

    let second = harness_full(
        vec![ScriptedClient::text("still a login form")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
        None,
        None,
        Some(open()),
        |_| {},
    )
    .await;
    second
        .runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("and now?")],
            Actor::user(),
        )
        .expect("send after the restart");
    second
        .wait_for("the resumed agent to be asked", || {
            second.client.remaining() == 0
        })
        .await;
    let requests = second.client.requests();
    let request = requests.last().expect("one request after the restart");
    assert_eq!(
        images_in(request),
        vec![("image/png".to_owned(), png_base64())],
        "the earlier message still shows the model its image"
    );
    assert!(text_in(request).contains("and now?"));
}

#[tokio::test]
async fn a_model_without_vision_gets_the_stored_path_as_text() {
    let providers = r#"
[[providers.anthropic.add_models]]
id = "text-only"
[providers.anthropic.add_models.capabilities]
context_window = 200000
max_output_tokens = 8000
"#;
    let attachments = tempfile::tempdir().expect("attachment dir");
    let dir = attachments.path().to_path_buf();
    let h = harness_full(
        vec![ScriptedClient::text("I cannot see images")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
        Some(providers),
        None,
        None,
        |_| {},
    )
    .await;
    let blind = ModelRef::new("anthropic", "text-only");
    let (agent, _) = h
        .runtime
        .create_agent(chat_spec(Some(blind)), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    send_with_screenshot(&h, agent.id, "look").await;

    let request = h.client.requests().pop().expect("request");
    assert!(images_in(&request).is_empty(), "no image for a blind model");
    assert!(!has_reference(&request));
    let text = text_in(&request);
    assert!(text.contains("this model takes no images"), "{text}");
    assert!(text.contains("stored at"), "{text}");
}

#[tokio::test]
async fn an_image_whose_file_was_removed_does_not_break_the_turn() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let dir = attachments.path().to_path_buf();
    let h = harness_configured(
        vec![ScriptedClient::text("I see nothing")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let images = h.runtime.store_attachments(&[screenshot()]).expect("store");
    let stored = h.runtime.attachment_path(&images[0]).expect("path");
    std::fs::remove_file(&stored).expect("remove the stored image");
    h.runtime
        .send_message(
            agent.id,
            message_content("look".into(), images),
            Actor::user(),
        )
        .expect("send");
    h.wait_for("the model to be asked", || h.client.remaining() == 0)
        .await;

    let request = h.client.requests().pop().expect("request");
    assert!(images_in(&request).is_empty());
    assert!(!has_reference(&request));
    assert!(
        text_in(&request).contains("no longer stored"),
        "{}",
        text_in(&request)
    );
}

/// Two messages within the limits, queued before the turn: the model is asked
/// about one message's worth of images at a time, and the second is not lost.
#[tokio::test]
async fn queued_image_messages_reach_a_native_model_one_turn_each() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let dir = attachments.path().join("images");
    let h = harness_configured(
        vec![ScriptedClient::text("one"), ScriptedClient::text("two")],
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let largest = |fill: u8| {
        let mut bytes = PNG.to_vec();
        bytes.resize(aether_core::attachment::MAX_IMAGE_BYTES, fill);
        ImageAttachment {
            media_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            name: None,
        }
    };
    let pair = h
        .runtime
        .store_attachments(&[largest(1), largest(2)])
        .expect("store two images");
    // No await between the sends: both are in the inbox when the turn starts.
    for text in ["first pair", "second pair"] {
        h.runtime
            .send_message(
                agent.id,
                message_content(text.into(), pair.clone()),
                Actor::user(),
            )
            .expect("send");
    }
    h.wait_for("both turns to ask the model", || h.client.remaining() == 0)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let requests = h.client.requests();
    assert_eq!(requests.len(), 2, "one request per message");
    let first = text_in(&requests[0]);
    assert!(first.contains("first pair") && !first.contains("second pair"));
    assert_eq!(images_in(&requests[0]).len(), 2);
    assert!(text_in(&requests[1]).contains("second pair"));
    let stored = h.store.agent(agent.id).expect("agent").expect("exists");
    assert_eq!(
        stored.inbox_cursor,
        h.store.inbox_last_seq(agent.id).expect("seq"),
        "both messages are consumed"
    );
}

/// A PNG by its leading bytes, `len` bytes long, unlike any other `fill`.
fn png_of(len: usize, fill: u8) -> ImageAttachment {
    let mut bytes = PNG.to_vec();
    bytes.resize(len, fill);
    ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: Some(format!("shot-{fill}.png")),
    }
}

/// Send `text` with `images` and wait until the model answered that turn.
async fn send_and_wait(h: &Harness, agent: AgentId, text: &str, images: &[ImageAttachment]) {
    let before = h.client.remaining();
    assert!(before > 0, "a scripted answer is left for this turn");
    let stored = h.runtime.store_attachments(images).expect("store");
    h.runtime
        .send_message(agent, message_content(text.into(), stored), Actor::user())
        .expect("send");
    h.wait_for("the model to be asked", || {
        h.client.remaining() == before - 1
    })
    .await;
    h.wait_until(agent, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
}

/// The whole request as Anthropic receives it, in bytes.
fn anthropic_body_len(request: &aether_core::model::ModelRequest) -> usize {
    let profile = aether_core::model::CapabilityProfile::default();
    let built =
        aether_model::adapters::anthropic::request::build(request, &profile).expect("a body");
    serde_json::to_vec(&built.body).expect("json").len()
}

/// One scripted answer per expected turn.
fn answers(count: usize) -> Vec<aether_model::testing::ScriptedResponse> {
    (0..count).map(|_| ScriptedClient::text("seen")).collect()
}

/// What every complete request must hold to, whatever its history.
fn assert_images_bounded(request: &aether_core::model::ModelRequest) {
    use aether_core::attachment::{MAX_REQUEST_IMAGE_BYTES, MAX_REQUEST_IMAGES};
    let images = images_in(request);
    let decoded: usize = images.iter().map(|(_, data)| data.len() / 4 * 3).sum();
    assert!(
        images.len() <= MAX_REQUEST_IMAGES,
        "{} images",
        images.len()
    );
    assert!(decoded <= MAX_REQUEST_IMAGE_BYTES, "{decoded} image bytes");
    assert!(!has_reference(request), "a reference never reaches a model");
    let body = anthropic_body_len(request);
    assert!(
        body < 32_000_000,
        "a {body}-byte request passes no 32 MB cap"
    );
}

const LEFT_OUT: &str = "an earlier image, left out to keep this request in its limits";

/// Messages that each pass every limit must not add up to a request the
/// provider refuses: the newest images are sent, older ones named in text.
#[tokio::test]
async fn images_across_a_whole_request_stay_bounded_turn_after_turn_and_after_a_restart() {
    const IMAGE_BYTES: usize = 3_688_788;
    let state = tempfile::tempdir().expect("state dir");
    let dir = state.path().join("images");
    let database = state.path().join("aether.db");
    let open = || -> Arc<dyn Store> { Arc::new(SqliteStore::open(&database).expect("store")) };
    let first = harness_full(
        answers(4),
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
        None,
        None,
        Some(open()),
        |_| {},
    )
    .await;
    let (agent, _) = first
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    first
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let pairs: Vec<[ImageAttachment; 2]> = (0..4u8)
        .map(|n| [png_of(IMAGE_BYTES, 2 * n), png_of(IMAGE_BYTES, 2 * n + 1)])
        .collect();
    for (n, pair) in pairs.iter().enumerate() {
        send_and_wait(&first, agent.id, &format!("pair {n}"), pair).await;
    }
    let mut requests = first.client.requests();
    first.runtime.shutdown().await;
    drop(first);

    let second = harness_full(
        answers(1),
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
        None,
        None,
        Some(open()),
        |_| {},
    )
    .await;
    send_and_wait(&second, agent.id, "and now?", &[]).await;
    requests.extend(second.client.requests());

    assert_eq!(requests.len(), 5, "one request per message");
    // 15 MiB takes four of these images, so each later request drops a pair.
    let expected = [(2, 0), (4, 0), (4, 2), (4, 4), (4, 4)];
    for (n, request) in requests.iter().enumerate() {
        assert_images_bounded(request);
        let (sent, left_out) = expected[n];
        let images = images_in(request);
        let text = text_in(request);
        assert_eq!(images.len(), sent, "request {n}");
        assert_eq!(text.matches(LEFT_OUT).count(), left_out, "request {n}");
        // The newest pair is the one the model sees last, whole.
        let newest = &pairs[n.min(3)];
        let tail: Vec<&str> = images[sent - 2..].iter().map(|(_, d)| d.as_str()).collect();
        assert_eq!(tail, [newest[0].data.as_str(), newest[1].data.as_str()]);
    }
    // A left-out image is still named with the file an agent can open.
    let oldest = second
        .runtime
        .store_attachments(&pairs[0][..1])
        .expect("already stored");
    let path = second.runtime.attachment_path(&oldest[0]).expect("path");
    let last = text_in(&requests[4]);
    assert!(path.is_file(), "the stored image outlives the request");
    assert!(
        last.contains(&format!("stored at {}]", path.display())),
        "{}",
        last.chars().take(2000).collect::<String>()
    );
}

/// Small images are bounded by their number alone.
#[tokio::test]
async fn a_request_carries_at_most_the_newest_twenty_images() {
    use aether_core::attachment::{MAX_IMAGES_PER_MESSAGE, MAX_REQUEST_IMAGES};
    let attachments = tempfile::tempdir().expect("attachment dir");
    let dir = attachments.path().join("images");
    let h = harness_configured(
        answers(3),
        |_| {},
        |config| config.attachment_dir = Some(dir.clone()),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(chat_spec(None), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let ten = vec![screenshot(); MAX_IMAGES_PER_MESSAGE];
    for text in ["one", "two", "three"] {
        send_and_wait(&h, agent.id, text, &ten).await;
    }

    let requests = h.client.requests();
    let counts: Vec<usize> = requests.iter().map(|r| images_in(r).len()).collect();
    assert_eq!(counts, [10, MAX_REQUEST_IMAGES, MAX_REQUEST_IMAGES]);
    requests.iter().for_each(assert_images_bounded);
    let last = text_in(&requests[2]);
    assert_eq!(last.matches(LEFT_OUT).count(), MAX_IMAGES_PER_MESSAGE);
}
