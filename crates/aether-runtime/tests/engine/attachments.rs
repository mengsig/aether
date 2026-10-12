//! Images on a message, engine path: the engine is handed the stored file,
//! at the turn's start and when a message arrives mid-turn.
use super::*;
use aether_core::attachment::ImageAttachment;
use aether_engine::{EngineImage, EngineInput};
use aether_runtime::attachments::message_content;
use base64::Engine as _;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot a real picture";

/// Records what each turn was launched with and what it was steered with.
/// With `steerable` it holds the turn open for one more message.
struct RecordingEngine {
    steerable: bool,
    turns: Mutex<Vec<EngineTurn>>,
    steered: Arc<Mutex<Vec<EngineInput>>>,
}

impl RecordingEngine {
    fn new(steerable: bool) -> Arc<Self> {
        Arc::new(Self {
            steerable,
            turns: Mutex::new(Vec::new()),
            steered: Arc::default(),
        })
    }
}

#[async_trait::async_trait]
impl Engine for RecordingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "recording/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.turns.lock().unwrap().push(turn);
        let (events, rx) = mpsc::channel(64);
        let (input_tx, mut input) = mpsc::channel::<EngineInput>(16);
        let steered = self.steered.clone();
        let steerable = self.steerable;
        tokio::spawn(async move {
            let session = EngineEvent::Session {
                id: "s-recorded".into(),
                model: None,
                tools: vec![],
            };
            let _ = events.send(session).await;
            if steerable && let Some(message) = input.recv().await {
                steered.lock().unwrap().push(message);
                let _ = events.send(EngineEvent::InputAccepted).await;
            }
            for event in answered("seen") {
                let _ = events.send(event).await;
            }
        });
        Ok(EngineRun {
            events: rx,
            input: self.steerable.then_some(input_tx),
            decisions: None,
            stop: None,
        })
    }
}

fn screenshot() -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(PNG),
        name: Some("shot.png".into()),
    }
}

async fn recording_harness(engine: Arc<RecordingEngine>, dir: &std::path::Path) -> Harness {
    let dir = dir.to_path_buf();
    harness_with_engine_config(engine, |config| config.attachment_dir = Some(dir)).await
}

fn send_screenshot(h: &Harness, agent: AgentId, text: &str) {
    let images = h
        .runtime
        .store_attachments(&[screenshot()])
        .expect("store the image");
    h.runtime
        .send_message(agent, message_content(text.into(), images), Actor::user())
        .expect("send");
}

/// The one stored file an engine was handed, checked against what was sent.
fn assert_is_the_screenshot(images: &[EngineImage], dir: &std::path::Path) {
    assert_eq!(images.len(), 1, "{images:?}");
    assert_eq!(images[0].media_type, "image/png");
    assert!(images[0].path.starts_with(dir), "{:?}", images[0].path);
    // The engine opens it from the agent's workspace, not the daemon's directory.
    assert!(images[0].path.is_absolute(), "{:?}", images[0].path);
    assert_eq!(std::fs::read(&images[0].path).expect("stored bytes"), PNG);
}

#[tokio::test]
async fn an_engine_turn_is_handed_the_stored_image_and_its_prompt_names_the_file() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let engine = RecordingEngine::new(false);
    let h = recording_harness(engine.clone(), attachments.path()).await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    send_screenshot(&h, id, "what is on this screen?");
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    let first = engine.turns.lock().unwrap()[0].clone();
    assert_is_the_screenshot(&first.images, attachments.path());
    assert!(first.prompt.contains("what is on this screen?"));
    let named = format!("stored at {}]", first.images[0].path.display());
    assert!(first.prompt.contains(&named), "{}", first.prompt);

    // The resumed session takes the next message; the image is not sent twice.
    engine_turn(&h, id, "thanks").await;
    let second = engine.turns.lock().unwrap()[1].clone();
    assert_eq!(second.session.as_deref(), Some("s-recorded"));
    assert!(second.images.is_empty(), "{:?}", second.images);
    assert!(second.prompt.contains("thanks"));
}

#[tokio::test]
async fn a_message_with_an_image_sent_mid_turn_reaches_the_engine_with_its_image() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let engine = RecordingEngine::new(true);
    let h = recording_harness(engine.clone(), attachments.path()).await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("start")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Running
    })
    .await;
    send_screenshot(&h, id, "and look at this");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    let steered = engine.steered.lock().unwrap().clone();
    assert_eq!(steered.len(), 1, "{steered:?}");
    assert!(steered[0].text.contains("and look at this"));
    assert_is_the_screenshot(&steered[0].images, attachments.path());
    assert_eq!(
        agent.inbox_cursor,
        h.store.inbox_last_seq(id).expect("seq"),
        "the acknowledged message is consumed"
    );
}

/// A PNG of the largest size one image may have, told apart by `fill`.
fn largest_image(fill: u8) -> ImageAttachment {
    let mut bytes = PNG.to_vec();
    bytes.resize(aether_core::attachment::MAX_IMAGE_BYTES, fill);
    ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: None,
    }
}

fn stored_bytes(images: &[EngineImage]) -> u64 {
    images
        .iter()
        .map(|image| std::fs::metadata(&image.path).expect("stored").len())
        .sum()
}

/// Two messages, each within the limits, queued before the turn: folded into
/// one input line their images would exceed what the engine's echo may be.
#[tokio::test]
async fn queued_image_messages_reach_the_engine_one_messages_worth_at_a_time() {
    let limit = aether_core::attachment::MAX_MESSAGE_IMAGE_BYTES as u64;
    let attachments = tempfile::tempdir().expect("attachment dir");
    let engine = RecordingEngine::new(true);
    let h = recording_harness(engine.clone(), attachments.path()).await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let pair = h
        .runtime
        .store_attachments(&[largest_image(1), largest_image(2)])
        .expect("store two images");
    // No await between the sends: both are in the inbox when the turn starts.
    for text in ["first pair", "second pair"] {
        h.runtime
            .send_message(
                id,
                message_content(text.into(), pair.clone()),
                Actor::user(),
            )
            .expect("send");
    }
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    let turns = engine.turns.lock().unwrap().clone();
    assert_eq!(
        turns.len(),
        1,
        "the second message is steered, not a new turn"
    );
    assert!(
        turns[0].prompt.contains("first pair"),
        "{}",
        turns[0].prompt
    );
    assert!(
        !turns[0].prompt.contains("second pair"),
        "{}",
        turns[0].prompt
    );
    assert_eq!(turns[0].images.len(), 2);
    assert!(stored_bytes(&turns[0].images) <= limit);

    let steered = engine.steered.lock().unwrap().clone();
    assert_eq!(steered.len(), 1, "{} inputs", steered.len());
    assert!(
        steered[0].text.contains("second pair"),
        "{}",
        steered[0].text
    );
    assert_eq!(steered[0].images.len(), 2);
    assert!(stored_bytes(&steered[0].images) <= limit);
    assert_eq!(
        agent.inbox_cursor,
        h.store.inbox_last_seq(id).expect("seq"),
        "both messages are consumed, each once"
    );
}
