//! Which events are worth a notification and what it says: a pure reading of one event and the
//! fleet's facts. Content reaches a payload only through [`preview`].

use aether_core::event::{Actor, Event, EventKind, OperatorOrigin};
use aether_core::id::{AgentId, ApprovalId};
use aether_protocol::remote::{AttentionKind, DeviceId, PushPayload};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest as _;

use super::{MAX_PREVIEW_CHARS, PAYLOAD_VERSION, PushError};

/// The audit event a revocation records; its payload names the device.
pub const DEVICE_REVOKED_EVENT: &str = "remote.device_revoked";
/// Longest agent name a payload carries, in characters.
pub const MAX_AGENT_NAME_CHARS: usize = 80;
/// RFC 8030: a topic is at most 32 characters of the base64url alphabet.
const TOPIC_BYTES: usize = 24;

/// Why an agent is blocked, when only the operator's attention helps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorBlock {
    /// A tool call started without an outcome; a person must look.
    Inspection,
    /// A provider's allowance is spent. `first` is false once a probe of the
    /// same episode has failed: the operator was already told.
    Exhausted { provider: String, first: bool },
}

/// What the notifier needs to know about one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    pub name: String,
    /// Whether the agent has no parent: its task is the operator's own.
    pub root: bool,
    /// Set while the agent is blocked on something the operator clears.
    pub block: Option<OperatorBlock>,
}

/// The fleet as the notifier reads it. The daemon answers from its store.
pub trait Fleet: Send + Sync + std::fmt::Debug + 'static {
    fn agent(&self, agent: AgentId) -> Result<Option<AgentFacts>, PushError>;
    /// Whether the approval is still waiting for a decision.
    fn approval_pending(&self, approval: ApprovalId) -> Result<bool, PushError>;
}

/// The fixed title for a kind: what a payload says without a preview.
pub fn title(kind: AttentionKind) -> &'static str {
    match kind {
        AttentionKind::Approval => "Approval needed",
        AttentionKind::Question => "An agent has a question",
        AttentionKind::AgentFailed => "An agent failed",
        AttentionKind::AgentBlocked => "An agent is blocked",
        AttentionKind::TaskCompleted => "Task completed",
        AttentionKind::TaskFailed => "Task failed",
        AttentionKind::Test => "Aether notifications are working",
    }
}

/// How soon a push service should deliver (RFC 8030 section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// Something waits on an answer: an approval or a question.
    High,
    Normal,
}

impl Urgency {
    pub fn of(kind: AttentionKind) -> Self {
        match kind {
            AttentionKind::Approval | AttentionKind::Question => Self::High,
            _ => Self::Normal,
        }
    }

    pub fn header(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
        }
    }
}

/// One notification, ready to be queued for every device that wants it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub payload: PushPayload,
    pub urgency: Urgency,
    /// The `Topic` header, and the key notices coalesce under.
    pub topic: String,
    /// The device whose own action caused the event; it is not notified.
    pub origin: Option<DeviceId>,
}

/// What one event means to the notifier, read from the event alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Nothing,
    /// One of the trigger kinds; [`compose`] decides whether it still matters.
    Candidate(AttentionKind),
    /// The approval was decided: a notification still queued for it is moot.
    Retract(ApprovalId),
    /// The device was revoked: its subscription goes.
    Revoked(DeviceId),
}

/// What a notification is about, before it becomes a payload.
struct Subject<'a> {
    kind: AttentionKind,
    agent: Option<AgentId>,
    facts: Option<AgentFacts>,
    approval: Option<ApprovalId>,
    /// The content a preview may quote, when previews are on.
    content: Option<&'a str>,
}

/// Read one event without looking anything up: this runs for every event
/// on the bus. Imported history happened elsewhere and notifies nobody.
pub fn classify(event: &Event) -> Signal {
    if matches!(event.actor, Actor::Imported { .. }) {
        return Signal::Nothing;
    }
    match &event.kind {
        EventKind::ApprovalRequested { .. } => Signal::Candidate(AttentionKind::Approval),
        EventKind::QuestionAsked { .. } => Signal::Candidate(AttentionKind::Question),
        EventKind::AgentFailed { .. } => Signal::Candidate(AttentionKind::AgentFailed),
        EventKind::AgentBlocked { .. } => Signal::Candidate(AttentionKind::AgentBlocked),
        EventKind::TaskCompleted { .. } => Signal::Candidate(AttentionKind::TaskCompleted),
        EventKind::TaskFailed { .. } => Signal::Candidate(AttentionKind::TaskFailed),
        EventKind::ApprovalResolved { approval, .. } => Signal::Retract(*approval),
        // Only the operator revokes; the same name from anyone else is not it.
        EventKind::Custom { name, payload }
            if name == DEVICE_REVOKED_EVENT && matches!(event.actor, Actor::User { .. }) =>
        {
            revoked_device(payload).map_or(Signal::Nothing, Signal::Revoked)
        }
        _ => Signal::Nothing,
    }
}

fn revoked_device(payload: &serde_json::Value) -> Option<DeviceId> {
    payload.get("device")?.as_str()?.parse().ok()
}

/// The notification for a candidate event, when the fleet agrees that it
/// waits on the operator: the trigger table of docs/remote.md.
pub fn compose(
    event: &Event,
    fleet: &dyn Fleet,
    instance_id: &str,
    previews: bool,
) -> Result<Option<Notice>, PushError> {
    assert!(!instance_id.is_empty(), "a daemon has an instance id");
    let (kind, approval, content) = match &event.kind {
        EventKind::ApprovalRequested {
            approval,
            description,
            ..
        } => (
            AttentionKind::Approval,
            Some(*approval),
            Some(description.as_str()),
        ),
        EventKind::QuestionAsked { question } => {
            (AttentionKind::Question, None, Some(question.as_str()))
        }
        EventKind::AgentFailed { error } => {
            (AttentionKind::AgentFailed, None, Some(error.as_str()))
        }
        EventKind::AgentBlocked { reason, .. } => {
            (AttentionKind::AgentBlocked, None, Some(reason.as_str()))
        }
        EventKind::TaskCompleted { .. } => (AttentionKind::TaskCompleted, None, None),
        EventKind::TaskFailed { error, .. } => {
            (AttentionKind::TaskFailed, None, Some(error.as_str()))
        }
        _ => return Ok(None),
    };
    let agent = event.agent_id;
    let facts = match agent {
        Some(agent) => fleet.agent(agent)?,
        None => None,
    };
    if !waits_on_operator(kind, approval, facts.as_ref(), fleet)? {
        return Ok(None);
    }
    let subject = Subject {
        kind,
        agent,
        facts,
        approval,
        content,
    };
    Ok(Some(notice(event, subject, instance_id, previews)))
}

fn waits_on_operator(
    kind: AttentionKind,
    approval: Option<ApprovalId>,
    facts: Option<&AgentFacts>,
    fleet: &dyn Fleet,
) -> Result<bool, PushError> {
    match kind {
        // A remembered rule answers in the same commit; nothing waits then.
        AttentionKind::Approval => match approval {
            Some(approval) => fleet.approval_pending(approval),
            None => Ok(false),
        },
        AttentionKind::AgentBlocked => Ok(matches!(
            facts.and_then(|facts| facts.block.as_ref()),
            Some(OperatorBlock::Inspection | OperatorBlock::Exhausted { first: true, .. })
        )),
        AttentionKind::TaskCompleted | AttentionKind::TaskFailed => {
            Ok(facts.is_some_and(|facts| facts.root))
        }
        AttentionKind::Question | AttentionKind::AgentFailed | AttentionKind::Test => Ok(true),
    }
}

fn notice(event: &Event, subject: Subject<'_>, instance_id: &str, previews: bool) -> Notice {
    let kind = subject.kind;
    let question_seq = (kind == AttentionKind::Question).then_some(event.seq);
    let topic = topic(instance_id, &scope(&subject));
    let payload = PushPayload {
        v: PAYLOAD_VERSION,
        kind,
        instance_id: instance_id.to_owned(),
        seq: event.seq,
        at: event.at,
        title: title(kind).to_owned(),
        url: route(kind, subject.agent, subject.approval, question_seq),
        agent: subject.agent,
        agent_name: subject
            .facts
            .as_ref()
            .map(|facts| clip(&facts.name, MAX_AGENT_NAME_CHARS)),
        task: event.task_id,
        approval: subject.approval,
        question_seq,
        preview: subject.content.filter(|_| previews).and_then(preview),
    };
    Notice {
        payload,
        urgency: Urgency::of(kind),
        topic,
        origin: acting_device(&event.actor),
    }
}

/// The notification `test_push` sends: no event, so `seq` is 0.
pub fn test_notice(instance_id: &str, at: aether_core::time::Timestamp) -> Notice {
    assert!(!instance_id.is_empty(), "a daemon has an instance id");
    let kind = AttentionKind::Test;
    let payload = PushPayload {
        v: PAYLOAD_VERSION,
        kind,
        instance_id: instance_id.to_owned(),
        seq: 0,
        at,
        title: title(kind).to_owned(),
        url: route(kind, None, None, None),
        agent: None,
        agent_name: None,
        task: None,
        approval: None,
        question_seq: None,
        preview: None,
    };
    Notice {
        payload,
        urgency: Urgency::of(kind),
        topic: topic(instance_id, "test"),
        origin: None,
    }
}

/// The device behind an operator's action, when a paired device took it.
fn acting_device(actor: &Actor) -> Option<DeviceId> {
    match actor {
        Actor::User {
            origin: Some(OperatorOrigin::Device { device, .. }),
            ..
        } => Some(*device),
        _ => None,
    }
}

/// Where a tap lands in the web app. The grammar is part of docs/remote.md.
fn route(
    kind: AttentionKind,
    agent: Option<AgentId>,
    approval: Option<ApprovalId>,
    question_seq: Option<u64>,
) -> String {
    match (kind, approval, question_seq, agent) {
        (AttentionKind::Approval, Some(approval), _, _) => format!("#attention?item={approval}"),
        (AttentionKind::Question, _, Some(seq), _) => format!("#attention?item=question-{seq}"),
        (_, _, _, Some(agent)) => format!("#agent/{agent}"),
        _ => "#activity".to_owned(),
    }
}

/// What notices coalesce under: things waiting on an answer and status
/// changes are kept apart per agent, so neither displaces the other.
fn scope(subject: &Subject<'_>) -> String {
    let block = subject
        .facts
        .as_ref()
        .and_then(|facts| facts.block.as_ref());
    if subject.kind == AttentionKind::AgentBlocked
        && let Some(OperatorBlock::Exhausted { provider, .. }) = block
    {
        return format!("provider:{provider}");
    }
    let class = match Urgency::of(subject.kind) {
        Urgency::High => "ask",
        Urgency::Normal => "status",
    };
    match subject.agent {
        Some(agent) => format!("agent:{agent}:{class}"),
        None => format!("fleet:{class}"),
    }
}

/// An opaque topic: the push service learns no agent or provider name.
fn topic(instance_id: &str, scope: &str) -> String {
    let mut hash = sha2::Sha256::new();
    hash.update(instance_id.as_bytes());
    hash.update([0]);
    hash.update(scope.as_bytes());
    let topic = URL_SAFE_NO_PAD.encode(&hash.finalize()[..TOPIC_BYTES]);
    assert_eq!(topic.len(), 32, "a topic is 32 base64url characters");
    topic
}

/// At most [`MAX_PREVIEW_CHARS`] of `content` on one line, or nothing.
fn preview(content: &str) -> Option<String> {
    let clipped = clip(content, MAX_PREVIEW_CHARS);
    (!clipped.is_empty()).then_some(clipped)
}

/// `text` on one line, cut to `max` characters with an ellipsis when longer.
fn clip(text: &str, max: usize) -> String {
    assert!(max >= 2, "room for a character and the ellipsis");
    let mut line = String::new();
    let mut kept = 0usize;
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            gap = kept > 0;
            continue;
        }
        let needed = 1 + usize::from(gap);
        if kept + needed > max {
            if kept == max {
                line.pop();
            }
            line.push('…');
            break;
        }
        if gap {
            line.push(' ');
        }
        line.push(c);
        kept += needed;
        gap = false;
    }
    assert!(
        line.chars().count() <= max,
        "a clipped line respects its limit"
    );
    line
}

#[cfg(test)]
mod tests;
