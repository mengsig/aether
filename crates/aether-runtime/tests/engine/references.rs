//! Task references, engine path: the reference images go with every turn
//! that sends the assignment, and never with a turn whose session holds it.
use std::collections::VecDeque;

use super::*;
use aether_core::reference::{ReferenceKind, ReferenceRequest};
use aether_engine::EngineImage;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\napproved mockup";

/// What one engine run does after it is handed its turn.
#[derive(Debug, Clone, Copy)]
enum Step {
    Answer,
    /// Folds its own session mid-turn, then answers.
    Compact,
    /// Refuses to resume the session, as a CLI that lost it does.
    LoseSession,
}

/// Runs the queued steps in order, answering once they run out, and keeps
/// every turn it was handed.
struct SteppedEngine {
    steps: Mutex<VecDeque<Step>>,
    turns: Mutex<Vec<EngineTurn>>,
}

impl SteppedEngine {
    fn new(steps: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into_iter().collect()),
            turns: Mutex::default(),
        })
    }

    fn images(&self) -> Vec<Vec<EngineImage>> {
        let turns = self.turns.lock().unwrap();
        turns.iter().map(|turn| turn.images.clone()).collect()
    }
}

fn step_events(step: Step) -> Vec<EngineEvent> {
    let session = EngineEvent::Session {
        id: "s-referenced".into(),
        model: None,
        tools: vec![],
    };
    let done = EngineEvent::Done(EngineOutcome {
        text: Some("done".into()),
        ..Default::default()
    });
    match step {
        Step::Answer => vec![session, done],
        Step::Compact => vec![
            session,
            EngineEvent::Compacted {
                automatic: true,
                before_tokens: Some(150_000),
                after_tokens: Some(20_000),
                duration_ms: None,
            },
            done,
        ],
        Step::LoseSession => vec![EngineEvent::Done(EngineOutcome {
            is_error: true,
            error: Some("no conversation found".into()),
            session_lost: true,
            ..Default::default()
        })],
    }
}

#[async_trait::async_trait]
impl Engine for SteppedEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "stepped/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.turns.lock().unwrap().push(turn);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Answer);
        let (events, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            for event in step_events(step) {
                let _ = events.send(event).await;
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// A started worker on `model` whose task carries one image reference.
fn referenced_worker(h: &Harness, model: ModelRef, mockup: &std::path::Path) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("build the settings page".into()),
                model: Some(model),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                references: vec![ReferenceRequest {
                    path: mockup.to_path_buf(),
                    kind: ReferenceKind::Image,
                    label: Some("approved mockup".into()),
                }],
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create a referenced worker");
    agent.id
}

fn assert_is_the_mockup(images: &[EngineImage], stored_in: &std::path::Path, turn: &str) {
    assert_eq!(images.len(), 1, "{turn}: {images:?}");
    assert!(
        images[0].path.starts_with(stored_in),
        "{:?}",
        images[0].path
    );
    assert_eq!(std::fs::read(&images[0].path).expect("stored"), PNG);
}

/// The first turn, a fold, and a lost session each send the assignment
/// again, so each takes the picture; a resumed session holding it does not.
#[tokio::test]
async fn reference_images_go_with_every_turn_that_sends_the_assignment() {
    let attachments = tempfile::tempdir().expect("attachment dir");
    let source = tempfile::tempdir().expect("source dir");
    let mockup = source.path().join("mockup.png");
    std::fs::write(&mockup, PNG).expect("write the mockup");
    let engine = SteppedEngine::new([
        Step::Answer,
        Step::Answer,
        Step::Compact,
        Step::Answer,
        Step::LoseSession,
        Step::Answer,
    ]);
    let stored_in = attachments.path().to_path_buf();
    let h = harness_with_engine_config(engine.clone(), |config| {
        config.attachment_dir = Some(stored_in)
    })
    .await;
    let id = referenced_worker(&h, ModelRef::new("claude-code", "haiku"), &mockup);
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    for text in ["carry on", "fold now", "after the fold", "lose it"] {
        engine_turn(&h, id, text).await;
    }

    let images = engine.images();
    assert_eq!(
        images.len(),
        6,
        "five turns, one of them retried: {images:?}"
    );
    assert_is_the_mockup(&images[0], attachments.path(), "the first turn");
    assert!(
        images[1].is_empty(),
        "the session holds it: {:?}",
        images[1]
    );
    assert!(
        images[2].is_empty(),
        "still held before the fold: {:?}",
        images[2]
    );
    assert_is_the_mockup(&images[3], attachments.path(), "after the fold");
    assert!(images[4].is_empty(), "the resume it lost: {:?}", images[4]);
    assert_is_the_mockup(&images[5], attachments.path(), "the fresh session");
    let first = engine.turns.lock().unwrap()[0].prompt.clone();
    assert!(first.contains("[image] approved mockup: "), "{first}");
}

/// A model without vision gets the reference by path, and no picture.
#[tokio::test]
async fn a_model_without_vision_is_named_the_reference_and_handed_no_image() {
    let providers_toml = r#"
[[providers.claude-code.add_models]]
id = "blind-claude"
[providers.claude-code.add_models.capabilities]
context_window = 200000
max_output_tokens = 64000
vision = false
"#;
    let attachments = tempfile::tempdir().expect("attachment dir");
    let source = tempfile::tempdir().expect("source dir");
    let mockup = source.path().join("mockup.png");
    std::fs::write(&mockup, PNG).expect("write the mockup");
    let engine = SteppedEngine::new([]);
    let stored_in = attachments.path().to_path_buf();
    let h = build_harness(
        engine.clone(),
        Arc::default(),
        Arc::default(),
        Some(providers_toml),
        None,
        None,
        |config| config.attachment_dir = Some(stored_in),
    )
    .await;
    let id = referenced_worker(&h, ModelRef::new("claude-code", "blind-claude"), &mockup);
    wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;

    let turns = engine.turns.lock().unwrap().clone();
    assert!(turns[0].images.is_empty(), "{:?}", turns[0].images);
    let canonical = mockup.canonicalize().expect("canonical");
    let named = format!("[image] approved mockup: {}", canonical.display());
    assert!(turns[0].prompt.contains(&named), "{}", turns[0].prompt);
}
