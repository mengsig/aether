//! An agent's conversation as people read it: one entry per thing said or
//! done, each flagged `important` where it is produced. Every surface filters
//! on that flag, so the TUI's `/focus` and a phone show the same entries.

use serde::{Deserialize, Serialize};

use crate::event::{Actor, Event, EventKind};
use crate::id::InvocationId;
use crate::inbox::InboxKind;
use crate::model::{ContentBlock, MessageRole, ModelErrorClass, ToolResultBlock};
use crate::note::Note;
use crate::time::Timestamp;

/// The most text a tool call, tool result or trigger entry carries, in bytes.
pub const MAX_DETAIL_BYTES: usize = 2048;

/// The event types an agent's conversation is folded from.
pub const EVENT_TYPES: [&str; 6] = [
    "task.created",
    "inbox.delivered",
    "agent.message",
    "invocation.failed",
    "question.asked",
    "custom",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ConversationEntryKind")]
pub enum EntryKind {
    /// What the operator sent: a message, or a task they created.
    Operator,
    /// The agent's own text, including text beside its tool calls.
    Assistant,
    /// A readable post a host appended (`append_note`).
    Post,
    /// The agent asking the operator (`ask_user`).
    Question,
    /// A message or task from another agent.
    Message,
    Thinking,
    ToolCall,
    ToolResult,
    /// A raw external trigger payload.
    Trigger,
}

impl EntryKind {
    /// The importance an entry of this kind always has; a post carries its own.
    fn important(self) -> bool {
        matches!(self, Self::Operator | Self::Assistant | Self::Question)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ConversationEntry")]
pub struct Entry {
    /// The event this entry came from; with `part`, the entry's id.
    pub seq: u64,
    /// Its place among the entries of the same event.
    pub part: u32,
    pub at: Timestamp,
    pub kind: EntryKind,
    pub important: bool,
    /// Markdown for operator, assistant, post and question entries.
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

/// The conversation entries one event adds, in order; most events add none.
pub fn entries(event: &Event) -> Vec<Entry> {
    let mut parts: Vec<(EntryKind, String, Vec<String>, bool)> = Vec::new();
    let mut add = |kind: EntryKind, text: String| {
        if !text.trim().is_empty() {
            parts.push((kind, text.trim().to_owned(), Vec::new(), kind.important()));
        }
    };
    match &event.kind {
        EventKind::TaskCreated { objective, .. } => add(sender(&event.actor), objective.clone()),
        EventKind::InboxDelivered { item } => match &item.kind {
            InboxKind::Message { from, content, .. } => add(sender(from), message_text(content)),
            InboxKind::ExternalTrigger {
                source, payload, ..
            } => add(EntryKind::Trigger, bounded(&format!("{source}: {payload}"))),
            _ => {}
        },
        EventKind::AgentMessage { role, content, .. } => {
            for (kind, text) in message_parts(*role, content) {
                add(kind, text);
            }
        }
        EventKind::InvocationFailed { partial, .. } if keeps_partial(&event.kind) => {
            let said = message_parts(MessageRole::Assistant, partial);
            for (kind, text) in said.into_iter().filter(|(kind, _)| !is_tool(*kind)) {
                add(kind, text);
            }
        }
        EventKind::QuestionAsked { question } => add(EntryKind::Question, question.clone()),
        EventKind::Custom { .. } => {
            if let Some(note) = Note::of(&event.kind) {
                let text = note.text.trim().to_owned();
                parts.push((EntryKind::Post, text, note.links, note.important));
            }
        }
        _ => {}
    }
    let entries: Vec<Entry> = parts
        .into_iter()
        .enumerate()
        .map(|(part, (kind, text, links, important))| Entry {
            seq: event.seq,
            part: u32::try_from(part).unwrap_or(u32::MAX),
            at: event.at,
            kind,
            important,
            text,
            links,
        })
        .collect();
    debug_assert!(
        entries.iter().all(|entry| entry.kind == EntryKind::Post
            || entry.important == entry.kind.important()),
        "only a post carries an importance of its own"
    );
    entries
}

/// The invocation of a cancelled turn, whose kept text the `agent.message`
/// just before it may already hold; that message's entries stand for it.
pub fn cancelled_invocation(event: &Event) -> Option<InvocationId> {
    match &event.kind {
        EventKind::InvocationFailed {
            invocation,
            cancelled: true,
            ..
        } => Some(*invocation),
        _ => None,
    }
}

/// Whether a failed turn's partial reads as said: a stop, or a failure that
/// is not retried (a usage limit is not retried soon either). The TUI agrees.
pub fn keeps_partial(kind: &EventKind) -> bool {
    let EventKind::InvocationFailed {
        cancelled,
        retryable,
        class,
        ..
    } = kind
    else {
        return false;
    };
    let limited = matches!(
        class,
        Some(ModelErrorClass::UsageLimit | ModelErrorClass::QuotaExhausted)
    );
    *cancelled || !*retryable || limited
}

fn is_tool(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::ToolCall | EntryKind::ToolResult)
}

/// Who sent a message or task: the operator, or another agent or service.
fn sender(actor: &Actor) -> EntryKind {
    match actor {
        Actor::User { .. } => EntryKind::Operator,
        _ => EntryKind::Message,
    }
}

/// A model message as entries: reasoning and text once each, then one entry
/// per tool call or result. A user message's own text repeats its inbox items.
fn message_parts(role: MessageRole, content: &[ContentBlock]) -> Vec<(EntryKind, String)> {
    let mut thinking = String::new();
    let mut text = String::new();
    let mut tools = Vec::new();
    for block in content {
        match block {
            ContentBlock::Thinking { text: t, .. } => thinking.push_str(t),
            ContentBlock::Text { text: t } if role == MessageRole::Assistant => text.push_str(t),
            ContentBlock::ToolUse { name, input, .. } => {
                tools.push((EntryKind::ToolCall, bounded(&format!("{name} {input}"))));
            }
            ContentBlock::ToolResult { content, .. } => {
                tools.push((EntryKind::ToolResult, bounded(&tool_result_text(content))));
            }
            _ => {}
        }
    }
    let mut parts = vec![
        (EntryKind::Thinking, thinking),
        (EntryKind::Assistant, text),
    ];
    parts.extend(tools);
    parts
}

fn tool_result_text(content: &[ToolResultBlock]) -> String {
    content
        .iter()
        .map(|block| match block {
            ToolResultBlock::Text { text } => text.as_str(),
            ToolResultBlock::Image { .. } => "[image]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A delivered message's text, then a line per image it carries.
fn message_text(content: &[ContentBlock]) -> String {
    let mut lines = vec![crate::model::text_of(content)];
    lines.extend(content.iter().filter_map(|block| match block {
        ContentBlock::ImageRef(image) => Some(image.label()),
        _ => None,
    }));
    lines.retain(|line| !line.trim().is_empty());
    lines.join("\n")
}

/// `text` cut to [`MAX_DETAIL_BYTES`] on a character boundary.
fn bounded(text: &str) -> String {
    if text.len() <= MAX_DETAIL_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_DETAIL_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests;
