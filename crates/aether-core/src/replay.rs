//! An agent's turns as they happened: what it saw, what it did, what it cost.
//!
//! Like [`crate::report`], a pure fold of the event log - the inspector, the
//! CLI and any other process step through the same frames because they are
//! derived from the same events, not assembled per surface (§1.3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{Actor, Event, EventKind};
use crate::id::{AgentId, ApprovalId, ArtifactId, InvocationId, SnapshotId, ToolCallId};
use crate::model::{ContentBlock, MessageRole, ModelRef, StopReason, Usage};
use crate::note::{Note, NoteContext, NoteLevel};
use crate::snapshot::ContextLayer;
use crate::time::Timestamp;
use crate::tool::{ApprovalDecision, SideEffectClass};

/// One turn of one agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayFrame {
    pub agent: AgentId,
    pub turn: u32,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Sequence range of the events this frame was folded from, so a caller
    /// can jump the live timeline to the same place.
    pub from_seq: u64,
    pub to_seq: u64,
    /// What it saw: the context assembled for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ReplayContext>,
    /// What was said, in order - what it was shown and what it answered.
    pub messages: Vec<ReplayMessage>,
    /// Notes a client appended after the turn before ended, in order. Notes
    /// written before the first turn open the first frame, or a turn-0 frame
    /// when there is no turn yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ReplayNote>,
    /// What it did.
    pub tools: Vec<ReplayTool>,
    pub approvals: Vec<ReplayApproval>,
    /// What it cost.
    pub usage: Usage,
    pub duration_ms: u64,
    pub outcome: TurnOutcome,
}

/// The shape of what the model was given. The layer texts are here; the
/// whole snapshot is a separate record, named so a caller can fetch it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayContext {
    pub snapshot: SnapshotId,
    pub model: ModelRef,
    pub token_estimate: u32,
    pub layers: Vec<ContextLayer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayMessage {
    pub at: Timestamp,
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationId>,
}

/// A note appended to the transcript; it started no turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayNote {
    pub at: Timestamp,
    pub seq: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(default)]
    pub context: NoteContext,
    #[serde(default, skip_serializing_if = "NoteLevel::is_info")]
    pub level: NoteLevel,
}

impl ReplayNote {
    fn new(event: &Event, note: Note) -> Self {
        Self {
            at: event.at,
            seq: event.seq,
            text: note.text,
            links: note.links,
            context: note.context,
            level: note.level,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayTool {
    pub call: ToolCallId,
    pub name: String,
    pub input: Value,
    pub side_effect: SideEffectClass,
    pub requested_at: Timestamp,
    pub outcome: ToolResult,
}

/// How a tool call in this turn ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ToolResult {
    /// Requested, and the log carries no end for it.
    Pending,
    Succeeded {
        content: String,
        #[serde(default)]
        is_error: bool,
        duration_ms: u64,
        #[serde(default)]
        artifacts: Vec<ArtifactId>,
        #[serde(default)]
        truncated: bool,
    },
    Failed {
        error: String,
        retryable: bool,
        /// `None` when the call never actually ran; see
        /// `EventKind::ToolFailed`. `skip_serializing_if` keeps it absent
        /// on the wire for `None` rather than a literal `null`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Denied {
        reason: String,
    },
    Cancelled,
    /// Answered from a committed outcome; the call did not run again.
    Replayed {
        original: ToolCallId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReplayApproval {
    pub id: ApprovalId,
    pub tool: String,
    pub description: String,
    pub requested_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    /// Who answered, as an actor label (`user`, `rule:rule_…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<Actor>,
}

/// How the turn ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The log ends inside this turn: it is the turn in flight.
    Running,
    Completed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
    },
    Failed {
        error: String,
        retryable: bool,
    },
    Interrupted,
    /// No turn has started: a turn-0 frame holding the notes written so far.
    NotStarted,
}

impl TurnOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Interrupted => "interrupted",
            Self::NotStarted => "not started",
        }
    }
}

/// Fold one agent's events into its turns, oldest first.
///
/// A turn opens at `agent.started`. Anything the agent was shown before its
/// first turn is that turn's prologue, so the first frame says what it
/// actually saw rather than starting mid-conversation. An agent that has
/// taken no turn yet folds to no frames, or to one turn-0 frame of its notes.
pub fn build(agent: AgentId, events: &[Event]) -> Vec<ReplayFrame> {
    let mut frames: Vec<ReplayFrame> = Vec::new();
    let mut prologue: Vec<ReplayMessage> = Vec::new();
    let mut prologue_notes: Vec<ReplayNote> = Vec::new();
    // Which frame asked. A tool outcome or an approval answer that lands
    // after the next turn has opened belongs to the turn that requested it,
    // not to whichever frame happens to be last - written there it would be
    // dropped, and a silent gap is worse than a visible one.
    let mut asked_in: BTreeMap<AskId, usize> = BTreeMap::new();
    for event in events.iter().filter(|e| e.agent_id == Some(agent)) {
        if let EventKind::AgentStarted { turn } = &event.kind {
            let mut frame = ReplayFrame::open(agent, *turn, event.at, event.seq);
            frame.messages = std::mem::take(&mut prologue);
            frame.notes = std::mem::take(&mut prologue_notes);
            frames.push(frame);
            continue;
        }
        if frames.is_empty() {
            if let Some(note) = Note::of(&event.kind) {
                prologue_notes.push(ReplayNote::new(event, note));
            } else if let EventKind::AgentMessage {
                role,
                content,
                invocation,
            } = &event.kind
            {
                prologue.push(ReplayMessage {
                    at: event.at,
                    role: *role,
                    content: content.clone(),
                    invocation: *invocation,
                });
            }
            continue;
        }
        let open = frames.len() - 1;
        match ask_of(&event.kind) {
            // Opens a question: remember where it was asked.
            Some((id, true)) => {
                asked_in.insert(id, open);
                take(&mut frames[open], event);
            }
            // Answers one: fold it into the frame that asked.
            Some((id, false)) => {
                let target = asked_in.get(&id).copied().unwrap_or(open);
                take(&mut frames[target], event);
            }
            None => take(&mut frames[open], event),
        }
    }
    if let Some(frame) = before_any_turn(agent, prologue, prologue_notes) {
        debug_assert!(frames.is_empty(), "a started turn takes the prologue");
        frames.push(frame);
    }
    frames
}

impl ReplayFrame {
    /// An empty frame of a running turn, opened at event `seq`.
    fn open(agent: AgentId, turn: u32, at: Timestamp, seq: u64) -> Self {
        Self {
            agent,
            turn,
            started_at: at,
            ended_at: None,
            from_seq: seq,
            to_seq: seq,
            context: None,
            messages: Vec::new(),
            notes: Vec::new(),
            tools: Vec::new(),
            approvals: Vec::new(),
            usage: Usage::default(),
            duration_ms: 0,
            outcome: TurnOutcome::Running,
        }
    }
}

/// The notes of an agent that has taken no turn, so they are not lost from
/// its replay: turn 0, spanning its first event to its last note.
fn before_any_turn(
    agent: AgentId,
    messages: Vec<ReplayMessage>,
    notes: Vec<ReplayNote>,
) -> Option<ReplayFrame> {
    let (first, last) = (notes.first()?, notes.last()?);
    debug_assert!(first.seq <= last.seq, "notes are in log order");
    let mut frame = ReplayFrame::open(agent, 0, first.at, first.seq);
    frame.to_seq = last.seq;
    frame.outcome = TurnOutcome::NotStarted;
    frame.messages = messages;
    frame.notes = notes;
    Some(frame)
}

/// A tool call or an approval this event opens (`true`) or answers
/// (`false`), when it does either.
fn ask_of(kind: &EventKind) -> Option<(AskId, bool)> {
    Some(match kind {
        EventKind::ToolRequested { call, .. } => (AskId::Tool(call.id), true),
        EventKind::ToolSucceeded { call, .. }
        | EventKind::ToolFailed { call, .. }
        | EventKind::ToolDenied { call, .. }
        | EventKind::ToolCancelled { call }
        | EventKind::ToolReplayed { call, .. } => (AskId::Tool(*call), false),
        EventKind::ApprovalRequested { approval, .. } => (AskId::Approval(*approval), true),
        EventKind::ApprovalResolved { approval, .. } => (AskId::Approval(*approval), false),
        _ => return None,
    })
}

/// A question a later event answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AskId {
    Tool(ToolCallId),
    Approval(ApprovalId),
}

/// Fold one event into the turn it belongs to, and stretch that turn's range
/// to reach it. Set here rather than on the open frame: a tool outcome that
/// arrives after the next turn started is folded into the frame that asked
/// for it, and the range has to be the one that frame actually covers.
fn take(frame: &mut ReplayFrame, event: &Event) {
    frame.to_seq = frame.to_seq.max(event.seq);
    match &event.kind {
        EventKind::AgentMessage {
            role,
            content,
            invocation,
        } => frame.messages.push(ReplayMessage {
            at: event.at,
            role: *role,
            content: content.clone(),
            invocation: *invocation,
        }),
        EventKind::SnapshotCreated {
            snapshot,
            model,
            token_estimate,
            layers,
            ..
        } => {
            frame.context = Some(ReplayContext {
                snapshot: *snapshot,
                model: model.clone(),
                token_estimate: *token_estimate,
                layers: layers.clone(),
            })
        }
        EventKind::ToolRequested { call, side_effect } => frame.tools.push(ReplayTool {
            call: call.id,
            name: call.name.clone(),
            input: call.input.clone(),
            side_effect: *side_effect,
            requested_at: call.requested_at,
            outcome: ToolResult::Pending,
        }),
        EventKind::ToolSucceeded { call, outcome } => {
            set_tool(
                frame,
                *call,
                ToolResult::Succeeded {
                    content: outcome.content.clone(),
                    is_error: outcome.is_error,
                    duration_ms: outcome.duration_ms,
                    artifacts: outcome.artifacts.clone(),
                    truncated: outcome.truncated,
                },
            );
        }
        EventKind::ToolFailed {
            call,
            error,
            retryable,
            duration_ms,
        } => set_tool(
            frame,
            *call,
            ToolResult::Failed {
                error: error.clone(),
                retryable: *retryable,
                duration_ms: *duration_ms,
            },
        ),
        EventKind::ToolDenied { call, reason } => set_tool(
            frame,
            *call,
            ToolResult::Denied {
                reason: reason.clone(),
            },
        ),
        EventKind::ToolCancelled { call } => set_tool(frame, *call, ToolResult::Cancelled),
        EventKind::ToolReplayed { call, original, .. } => set_tool(
            frame,
            *call,
            ToolResult::Replayed {
                original: *original,
            },
        ),
        EventKind::ApprovalRequested {
            approval,
            tool,
            description,
            ..
        } => frame.approvals.push(ReplayApproval {
            id: *approval,
            tool: tool.clone(),
            description: description.clone(),
            requested_at: event.at,
            decision: None,
            decided_by: None,
        }),
        EventKind::ApprovalResolved {
            approval,
            decision,
            by,
            ..
        } => {
            if let Some(a) = frame.approvals.iter_mut().find(|a| a.id == *approval) {
                a.decision = Some(*decision);
                a.decided_by = Some(by.original().clone());
            }
        }
        EventKind::Custom { .. } => {
            if let Some(note) = Note::of(&event.kind) {
                frame.notes.push(ReplayNote::new(event, note));
            }
        }
        EventKind::AgentUsageUpdated { turn_usage, .. } => {
            frame.usage = *turn_usage;
        }
        EventKind::InvocationCompleted {
            usage,
            stop_reason,
            duration_ms,
            ..
        } => {
            frame.usage += *usage;
            frame.duration_ms = frame.duration_ms.saturating_add(*duration_ms);
            frame.outcome = TurnOutcome::Completed {
                stop_reason: stop_reason.clone(),
            };
        }
        EventKind::InvocationFailed {
            error,
            retryable,
            cancelled,
            ..
        } => {
            frame.outcome = if *cancelled {
                TurnOutcome::Interrupted
            } else {
                TurnOutcome::Failed {
                    error: error.clone(),
                    retryable: *retryable,
                }
            };
        }
        EventKind::AgentInterrupted { .. } => frame.outcome = TurnOutcome::Interrupted,
        EventKind::AgentTurnCompleted { .. } => frame.ended_at = Some(event.at),
        EventKind::AgentFailed { error } => {
            frame.ended_at = Some(event.at);
            if matches!(frame.outcome, TurnOutcome::Running) {
                frame.outcome = TurnOutcome::Failed {
                    error: error.clone(),
                    retryable: false,
                };
            }
        }
        EventKind::AgentCompleted { .. } | EventKind::AgentCancelled { .. } => {
            frame.ended_at = Some(event.at)
        }
        _ => {}
    }
}

fn set_tool(frame: &mut ReplayFrame, call: ToolCallId, result: ToolResult) {
    if let Some(tool) = frame.tools.iter_mut().find(|t| t.call == call) {
        tool.outcome = result;
    }
}

#[cfg(test)]
mod tests;
