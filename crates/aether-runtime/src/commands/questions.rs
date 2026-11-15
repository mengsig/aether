//! Operator questions, each open until its own answer: `ask_user` records a
//! `question.asked`; a `question.answered` custom event closes one by its seq.
//!
//! A targeted `reply_to_question` closes only its question. Any other message
//! from an operator closes every question pending when it lands. History from
//! before answers were recorded has none: there, an operator's message closed
//! every earlier question, and an untargeted message still means exactly that.

use crate::Result;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, MessageId, RunId};
use aether_store::{EventFilter, Store};
use std::collections::{BTreeMap, HashSet};

/// The custom event that closes one question.
pub const QUESTION_ANSWERED: &str = "question.answered";

/// Events read per page while walking an agent's history back.
const PAGE: usize = 128;

/// Every question `agent` asked that is still open, oldest first.
pub fn pending_questions(store: &dyn Store, agent: AgentId) -> Result<Vec<Event>> {
    let mut filter = EventFilter::own_events(agent);
    filter.types = vec!["question.asked".into()];
    // Nothing older than the first question can hold one open.
    let Some(first) = store.events(0, 1, &filter)?.pop() else {
        return Ok(Vec::new());
    };
    filter.types = vec![
        "question.asked".into(),
        "message.sent".into(),
        "custom".into(),
    ];
    let mut walk = Walk::new(agent);
    let mut before = u64::MAX;
    loop {
        let page = store.events_before(before, PAGE, &filter)?;
        for event in &page {
            debug_assert!(event.seq < before, "a page runs newest first");
            before = event.seq;
            if event.seq < first.seq || walk.reached_barrier(event) {
                return Ok(walk.open());
            }
        }
        if page.len() < PAGE {
            return Ok(walk.open());
        }
    }
}

/// The question `agent` asked last that is still open.
pub fn pending_question(store: &dyn Store, agent: AgentId) -> Result<Option<Event>> {
    Ok(pending_questions(store, agent)?.pop())
}

/// Whether `agent`'s question `seq` is still waiting for its answer.
pub fn question_is_open(store: &dyn Store, agent: AgentId, seq: u64) -> Result<bool> {
    Ok(pending_questions(store, agent)?
        .iter()
        .any(|asked| asked.seq == seq))
}

/// What closes `questions` of `agent` with `message`, recorded right after
/// that message's `message.sent` in the same commit. `reply`: a targeted
/// reply to its one question, which closes nothing else.
pub(crate) fn answers(
    by: &Actor,
    agent: AgentId,
    run: RunId,
    questions: &[u64],
    message: MessageId,
    reply: bool,
) -> Vec<EventDraft> {
    assert!(
        !reply || questions.len() == 1,
        "a targeted reply answers exactly one question"
    );
    debug_assert!(
        questions.windows(2).all(|pair| pair[0] < pair[1]),
        "questions close in the order they were asked"
    );
    questions
        .iter()
        .map(|seq| {
            let payload = serde_json::json!({
                "question_seq": seq,
                "message": message,
                "reply": reply,
            });
            EventDraft::new(
                by.clone(),
                EventKind::Custom {
                    name: QUESTION_ANSWERED.into(),
                    payload,
                },
            )
            .agent(agent)
            .run(run)
        })
        .collect()
}

/// Whether `from` speaks for the operator, whose message answers questions.
pub(crate) fn is_operator(from: &Actor) -> bool {
    matches!(from, Actor::User { .. } | Actor::Client { .. })
}

/// An agent's history read newest first. Answers follow their message, so
/// they are seen before it: a message none of them targets closes everything.
struct Walk {
    agent: AgentId,
    asked: BTreeMap<u64, Event>,
    answered: HashSet<u64>,
    targeted: HashSet<MessageId>,
}

impl Walk {
    fn new(agent: AgentId) -> Self {
        Self {
            agent,
            asked: BTreeMap::new(),
            answered: HashSet::new(),
            targeted: HashSet::new(),
        }
    }

    /// Takes in one event; `true` at an operator message that closed every
    /// question before it, where the walk stops.
    fn reached_barrier(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::QuestionAsked { .. } if event.actor.agent_id() == Some(self.agent) => {
                if !self.answered.contains(&event.seq) {
                    self.asked.insert(event.seq, event.clone());
                }
                false
            }
            EventKind::Custom { name, payload } if name == QUESTION_ANSWERED => {
                self.take_answer(payload);
                false
            }
            EventKind::MessageSent {
                message, from, to, ..
            } => *to == self.agent && is_operator(from) && !self.targeted.contains(message),
            _ => false,
        }
    }

    fn take_answer(&mut self, payload: &serde_json::Value) {
        let Some(seq) = payload.get("question_seq").and_then(|v| v.as_u64()) else {
            tracing::warn!(%payload, "a question.answered event names no question");
            return;
        };
        self.answered.insert(seq);
        let targeted = payload.get("reply").and_then(|v| v.as_bool()) == Some(true);
        let message = payload
            .get("message")
            .and_then(|v| serde_json::from_value::<MessageId>(v.clone()).ok());
        if let (true, Some(message)) = (targeted, message) {
            self.targeted.insert(message);
        }
    }

    fn open(self) -> Vec<Event> {
        let open: Vec<Event> = self.asked.into_values().collect();
        debug_assert!(
            open.windows(2).all(|pair| pair[0].seq < pair[1].seq),
            "open questions come oldest first"
        );
        open
    }
}
