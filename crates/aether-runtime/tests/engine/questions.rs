//! Native permission/question reply channel semantics.
use super::*;
#[derive(Clone, Copy, Debug)]
enum QuestionChannel {
    Open,
    Closed,
    Missing,
}

struct QuestionEngine(QuestionChannel);

#[async_trait::async_trait]
impl Engine for QuestionEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "question-test/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (events, rx) = mpsc::channel(8);
        let (decisions, answers) = mpsc::channel::<aether_engine::PermissionDecision>(1);
        let mut answers = if matches!(self.0, QuestionChannel::Open) {
            Some(answers)
        } else {
            drop(answers);
            None
        };
        tokio::spawn(async move {
            events
                .send(EngineEvent::Question {
                    request_id: "question-1".into(),
                    prompt: "Which colour?".into(),
                })
                .await
                .expect("question delivered");
            if let Some(answers) = answers.as_mut() {
                let answer = tokio::select! {
                    answer = answers.recv() => answer.expect("answer delivered"),
                    () = cancel.cancelled() => return,
                };
                assert_eq!(answer.request_id, "question-1");
                let text = format!("Selected {}", answer.answer.expect("answer text"));
                events
                    .send(EngineEvent::Assistant {
                        content: vec![ContentBlock::text(&text)],
                        usage: None,
                        model: None,
                    })
                    .await
                    .expect("assistant delivered");
                events
                    .send(EngineEvent::Done(EngineOutcome {
                        text: Some(text),
                        ..Default::default()
                    }))
                    .await
                    .expect("outcome delivered");
            } else {
                cancel.cancelled().await;
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: (!matches!(self.0, QuestionChannel::Missing)).then_some(decisions),
            stop: None,
        })
    }
}

#[tokio::test]
async fn question_answers_without_steering_are_delivered_or_retained_on_channel_failure() {
    for channel in [
        QuestionChannel::Open,
        QuestionChannel::Closed,
        QuestionChannel::Missing,
    ] {
        let h = harness_with_engine(
            Arc::new(QuestionEngine(channel)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let id = chat_agent(&h);
        h.runtime
            .send_message(
                id,
                vec![ContentBlock::text("Choose a colour")],
                Actor::user(),
            )
            .expect("start");
        wait_for(h.store.as_ref(), id, |_| {
            spoken(h.store.as_ref(), id)
                .iter()
                .any(|(role, text)| *role == MessageRole::Assistant && text == "Which colour?")
        })
        .await;
        h.runtime
            .send_message(id, vec![ContentBlock::text("blue")], Actor::user())
            .expect("answer");
        let agent = wait_for(h.store.as_ref(), id, |a| match channel {
            QuestionChannel::Open => {
                a.status.kind() == AgentStatusKind::Waiting && a.stats.turns == 1
            }
            _ => a.status.kind() == AgentStatusKind::Failed,
        })
        .await;
        let last = h.store.inbox_last_seq(id).expect("last inbox seq");
        match channel {
            QuestionChannel::Open => {
                assert_eq!(agent.inbox_cursor, last);
                assert!(spoken(h.store.as_ref(), id).iter().any(|(role, text)| {
                    *role == MessageRole::Assistant && text.contains("blue")
                }));
            }
            _ => {
                assert!(agent.inbox_cursor < last, "{channel:?}: answer was lost");
                assert!(format!("{:?}", agent.status).contains("answer remains queued"));
                assert_eq!(
                    h.store
                        .inbox(id, agent.inbox_cursor)
                        .expect("pending")
                        .len(),
                    1
                );
            }
        }
        h.runtime.shutdown().await;
    }
}

/// Takes one steered message without acknowledging it, asks a question,
/// reads what follows the answer, then refuses the steered message (#306).
/// Every later turn just answers.
#[derive(Default)]
struct SteerThenQuestion {
    prompts: Arc<Mutex<Vec<String>>>,
    answers: Arc<Mutex<Vec<String>>>,
    inputs: Arc<Mutex<Vec<String>>>,
}

const STEER: &str = "also update the changelog";

#[async_trait::async_trait]
impl Engine for SteerThenQuestion {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "steer-then-question-test/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let first = {
            let mut prompts = self.prompts.lock().unwrap();
            prompts.push(turn.prompt.clone());
            prompts.len() == 1
        };
        let (events, rx) = mpsc::channel(16);
        let say = |text: &str| EngineEvent::Assistant {
            content: vec![ContentBlock::text(text)],
            usage: None,
            model: None,
        };
        let done = || {
            EngineEvent::Done(EngineOutcome {
                closing_accounting: true,
                text: Some("done".into()),
                ..Default::default()
            })
        };
        if !first {
            events.send(say("done")).await.expect("reply");
            events.send(done()).await.expect("outcome");
            return Ok(EngineRun {
                events: rx,
                input: None,
                decisions: None,
                stop: None,
            });
        }
        let (input_tx, mut input) = mpsc::channel::<aether_engine::EngineInput>(16);
        let (decisions, mut answers_rx) = mpsc::channel::<aether_engine::PermissionDecision>(1);
        let (answers, inputs) = (self.answers.clone(), self.inputs.clone());
        tokio::spawn(async move {
            events.send(say("working")).await.expect("working");
            let steered = tokio::select! {
                steered = input.recv() => steered.expect("the steered message"),
                () = cancel.cancelled() => return,
            };
            inputs.lock().unwrap().push(steered.text);
            events
                .send(EngineEvent::Question {
                    request_id: "question-1".into(),
                    prompt: "Which colour?".into(),
                })
                .await
                .expect("question");
            let answer = tokio::select! {
                answer = answers_rx.recv() => answer.expect("the answer"),
                () = cancel.cancelled() => return,
            };
            answers
                .lock()
                .unwrap()
                .push(answer.answer.expect("answer text"));
            // The reply, when it follows as input; bounded so a reply that
            // never comes fails the assertions instead of hanging.
            if let Ok(Some(next)) = tokio::time::timeout(Duration::from_secs(2), input.recv()).await
            {
                inputs.lock().unwrap().push(next.text);
            }
            events
                .send(EngineEvent::InputRejected {
                    reason: "turn is over".into(),
                })
                .await
                .expect("rejection");
            events.send(say("done")).await.expect("reply");
            events.send(done()).await.expect("outcome");
        });
        Ok(EngineRun {
            events: rx,
            input: Some(input_tx),
            decisions: Some(decisions),
            stop: None,
        })
    }
}

#[tokio::test]
async fn an_answer_never_consumes_past_a_steered_message_the_engine_then_refuses() {
    let engine = Arc::new(SteerThenQuestion::default());
    let h = harness_with_engine(
        engine.clone(),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    let store = h.store.clone();
    let said = |text: &'static str| {
        let store = store.clone();
        move |_: &aether_core::agent::Agent| {
            spoken(store.as_ref(), id)
                .iter()
                .any(|(role, t)| *role == MessageRole::Assistant && t == text)
        }
    };
    h.runtime
        .send_message(id, vec![ContentBlock::text("start")], Actor::user())
        .expect("start");
    wait_for(h.store.as_ref(), id, said("working")).await;
    h.runtime
        .send_message(id, vec![ContentBlock::text(STEER)], Actor::user())
        .expect("steer");
    wait_for(h.store.as_ref(), id, said("Which colour?")).await;
    h.runtime
        .send_message(id, vec![ContentBlock::text("blue")], Actor::user())
        .expect("answer");

    // The refused steer and the reply that followed it open the next turn.
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 2
    })
    .await;
    assert_eq!(agent.inbox_cursor, h.store.inbox_last_seq(id).expect("seq"));
    let answers = engine.answers.lock().unwrap().clone();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert!(!answers[0].contains("blue"), "{answers:?}");
    let inputs = engine.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2, "the reply followed the steer: {inputs:?}");
    assert!(
        inputs[0].contains(STEER) && inputs[1].contains("blue"),
        "{inputs:?}"
    );
    let prompts = engine.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(
        prompts[1].contains(STEER) && prompts[1].contains("blue"),
        "{prompts:?}"
    );

    // The cursor never passed the steer without recording it consumed.
    let steer = h
        .store
        .inbox(id, 0)
        .expect("inbox")
        .into_iter()
        .find(|item| format!("{:?}", item.kind).contains(STEER))
        .expect("the steered item");
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let consumed: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::InboxConsumed { up_to, items } => Some((*up_to, items.clone())),
            _ => None,
        })
        .collect();
    assert!(
        consumed
            .iter()
            .all(|(up_to, items)| *up_to < steer.seq || items.contains(&steer.id)),
        "{consumed:?}"
    );
    assert!(
        consumed.iter().any(|(_, items)| items.contains(&steer.id)),
        "the steer is consumed by the next turn: {consumed:?}"
    );
    h.runtime.shutdown().await;
}
