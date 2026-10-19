//! The notification center: things a poster (an agent or a service such as
//! Archon) wants the operator to see or answer, kept per daemon.
//!
//! Every change is recorded as a `custom` event named `notification.*` whose
//! payload is the whole record after the change, so a client that predates
//! notifications still decodes the log and a newer one can follow it live.

use serde::{Deserialize, Serialize};

use crate::event::{Actor, EventDraft, EventKind};
use crate::id::{AgentId, NotificationId, RunId};
use crate::time::Timestamp;

pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_BODY_BYTES: usize = 4096;
pub const MAX_KEY_BYTES: usize = 200;
pub const MAX_LINK_BYTES: usize = 512;
pub const MAX_ACTIONS: usize = 8;
pub const MAX_ACTION_ID_BYTES: usize = 64;
pub const MAX_ACTION_LABEL_CHARS: usize = 64;
pub const MAX_ANSWER_BYTES: usize = 4096;
pub const MAX_SOURCE_BYTES: usize = 64;
/// The inbox trigger source an answer reaches a posting agent under.
pub const ANSWER_TRIGGER_SOURCE: &str = "notification";

/// What a notification asks of the operator. Questions and blockers wait on
/// them; the rest inform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Question,
    Blocker,
    Decision,
    Outcome,
    Info,
}

impl NotificationKind {
    /// Whether work waits on the operator, so the indicator makes it stand out.
    pub fn is_urgent(self) -> bool {
        matches!(self, Self::Question | Self::Blocker)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Blocker => "blocker",
            Self::Decision => "decision",
            Self::Outcome => "outcome",
            Self::Info => "info",
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NotificationState {
    Unread,
    Read,
    Resolved,
}

impl NotificationState {
    pub const ALL: [Self; 3] = [Self::Unread, Self::Read, Self::Resolved];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Read => "read",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == raw)
    }
}

/// A choice the operator can make; its `id` is what the poster receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NotificationAction {
    pub id: String,
    pub label: String,
}

/// What a poster sends. Posting the same `key` again from the same source
/// updates that notification instead of adding another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NotificationPost {
    pub kind: NotificationKind,
    /// One line, at most [`MAX_TITLE_CHARS`] characters.
    pub title: String,
    /// Short Markdown, at most [`MAX_BODY_BYTES`] bytes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// The poster's own id for the thing, such as a question id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A URL, or an agent id the operator can jump to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<NotificationAction>,
    /// Whether the operator may answer in their own words.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub free_text: bool,
}

/// The operator's answer: a chosen action, their own words, or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NotificationAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub by: Actor,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Notification {
    pub id: NotificationId,
    /// The posting agent's id, or a service name such as `archon`.
    pub source: String,
    pub kind: NotificationKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<NotificationAction>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub free_text: bool,
    pub state: NotificationState,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<NotificationAnswer>,
}

/// Which change an event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationChange {
    Posted,
    Updated,
    Read,
    Answered,
    Resolved,
}

impl NotificationChange {
    pub const ALL: [Self; 5] = [
        Self::Posted,
        Self::Updated,
        Self::Read,
        Self::Answered,
        Self::Resolved,
    ];

    /// The `custom` event name this change is recorded under.
    pub fn event_name(self) -> &'static str {
        match self {
            Self::Posted => "notification.posted",
            Self::Updated => "notification.updated",
            Self::Read => "notification.read",
            Self::Answered => "notification.answered",
            Self::Resolved => "notification.resolved",
        }
    }

    fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.event_name() == name)
    }
}

/// Why a service name cannot be a notification source.
pub fn check_service_source(source: &str) -> Result<(), String> {
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c);
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES || !source.chars().all(allowed) {
        return Err(format!(
            "a notification source is 1 to {MAX_SOURCE_BYTES} lowercase letters, digits, \
             `-`, `_` or `.`, and `{source}` is not"
        ));
    }
    if source.starts_with(&format!("{}_", AgentId::PREFIX)) {
        return Err("an agent posts with the notify tool, not under its id as a service".into());
    }
    Ok(())
}

impl NotificationPost {
    /// Why this post cannot be stored, naming the limit it breaks.
    pub fn check(&self) -> Result<(), String> {
        let title = self.title.trim();
        if title.is_empty() || title.contains(['\n', '\r']) {
            return Err("a notification title is one line of text".into());
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(format!(
                "a notification title is limited to {MAX_TITLE_CHARS} characters"
            ));
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(format!(
                "a notification body is limited to {MAX_BODY_BYTES} bytes and this one has {}",
                self.body.len()
            ));
        }
        check_reference("key", self.key.as_deref(), MAX_KEY_BYTES)?;
        check_reference("link", self.link.as_deref(), MAX_LINK_BYTES)?;
        self.check_actions()
    }

    fn check_actions(&self) -> Result<(), String> {
        if self.actions.len() > MAX_ACTIONS {
            return Err(format!("a notification has at most {MAX_ACTIONS} actions"));
        }
        let id_ok = |id: &str| {
            !id.is_empty()
                && id.len() <= MAX_ACTION_ID_BYTES
                && id.chars().all(|c| c.is_ascii_graphic())
        };
        let label_ok = |label: &str| {
            !label.trim().is_empty()
                && label.chars().count() <= MAX_ACTION_LABEL_CHARS
                && !label.chars().any(char::is_control)
        };
        for (i, action) in self.actions.iter().enumerate() {
            if !id_ok(&action.id) || !label_ok(&action.label) {
                return Err(format!(
                    "action {} needs an id of at most {MAX_ACTION_ID_BYTES} visible ASCII bytes \
                     and a one-line label of at most {MAX_ACTION_LABEL_CHARS} characters",
                    i + 1
                ));
            }
            if self.actions[..i].iter().any(|a| a.id == action.id) {
                return Err(format!("action id `{}` is used twice", action.id));
            }
        }
        Ok(())
    }
}

fn check_reference(what: &str, value: Option<&str>, max: usize) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(format!(
            "a notification {what} is limited to {max} bytes, is not blank and has no control characters"
        ));
    }
    Ok(())
}

impl Notification {
    /// A fresh, unread notification from a checked post.
    pub fn new(source: String, post: NotificationPost, at: Timestamp) -> Self {
        debug_assert!(
            post.check().is_ok(),
            "a post is checked before it is stored"
        );
        Self {
            id: NotificationId::new(),
            source,
            kind: post.kind,
            title: post.title.trim().to_owned(),
            body: post.body,
            key: post.key,
            link: post.link,
            actions: post.actions,
            free_text: post.free_text,
            state: NotificationState::Unread,
            created_at: at,
            updated_at: at,
            answer: None,
        }
    }

    /// The agent that posted this, when an agent did.
    pub fn poster_agent(&self) -> Option<AgentId> {
        self.source.parse().ok()
    }

    /// Whether the operator can still answer it.
    pub fn is_answerable(&self) -> bool {
        self.state != NotificationState::Resolved
            && self.answer.is_none()
            && (self.free_text || !self.actions.is_empty())
    }

    /// Apply a repost under the same key. Returns whether anything changed;
    /// a change makes a read notification unread, and a resolved one stays so.
    pub fn repost(&mut self, post: NotificationPost, at: Timestamp) -> bool {
        debug_assert!(
            post.check().is_ok(),
            "a post is checked before it is stored"
        );
        debug_assert_eq!(post.key, self.key, "a repost keeps its key");
        let before = self.clone();
        self.kind = post.kind;
        self.title = post.title.trim().to_owned();
        self.body = post.body;
        self.link = post.link;
        self.actions = post.actions;
        self.free_text = post.free_text;
        if *self == before {
            return false;
        }
        if self.state == NotificationState::Read {
            self.state = NotificationState::Unread;
        }
        self.updated_at = at;
        true
    }

    /// Mark it seen. Only an unread notification changes.
    pub fn mark_read(&mut self, at: Timestamp) -> bool {
        if self.state != NotificationState::Unread {
            return false;
        }
        self.state = NotificationState::Read;
        self.updated_at = at;
        true
    }

    /// Settle it. Resolving twice changes nothing.
    pub fn resolve(&mut self, at: Timestamp) -> bool {
        if self.state == NotificationState::Resolved {
            return false;
        }
        self.state = NotificationState::Resolved;
        self.updated_at = at;
        true
    }

    /// Record the operator's answer, which resolves the notification. Refused
    /// when it cannot be answered, or the answer is not one it offers.
    pub fn answer(
        &mut self,
        action: Option<String>,
        text: Option<String>,
        by: Actor,
        at: Timestamp,
    ) -> Result<(), String> {
        if self.answer.is_some() {
            return Err(format!("{} was already answered", self.id));
        }
        if self.state == NotificationState::Resolved {
            return Err(format!(
                "{} is resolved and no longer takes an answer",
                self.id
            ));
        }
        let text = text.filter(|t| !t.trim().is_empty());
        self.check_choice(action.as_deref(), text.as_deref())?;
        self.answer = Some(NotificationAnswer {
            action,
            text,
            by,
            at,
        });
        self.state = NotificationState::Resolved;
        self.updated_at = at;
        Ok(())
    }

    fn check_choice(&self, action: Option<&str>, text: Option<&str>) -> Result<(), String> {
        if action.is_none() && text.is_none() {
            return Err("an answer chooses an action or gives text".into());
        }
        if let Some(action) = action
            && !self.actions.iter().any(|a| a.id == action)
        {
            let offered: Vec<&str> = self.actions.iter().map(|a| a.id.as_str()).collect();
            return Err(format!(
                "`{action}` is not an action of {}; it offers: {}",
                self.id,
                if offered.is_empty() {
                    "none".into()
                } else {
                    offered.join(", ")
                }
            ));
        }
        let Some(text) = text else {
            return Ok(());
        };
        if !self.free_text {
            return Err(format!("{} takes a choice of action, not text", self.id));
        }
        if text.len() > MAX_ANSWER_BYTES {
            return Err(format!("an answer is limited to {MAX_ANSWER_BYTES} bytes"));
        }
        Ok(())
    }

    /// The event recording `change`, carrying the record as it now stands.
    /// An agent's notification is filed under that agent and its run.
    pub fn draft(
        &self,
        change: NotificationChange,
        by: Actor,
        poster_run: Option<RunId>,
    ) -> Result<EventDraft, String> {
        let payload = serde_json::to_value(self).map_err(|error| error.to_string())?;
        debug_assert!(payload.is_object(), "a notification encodes as an object");
        let kind = EventKind::Custom {
            name: change.event_name().to_owned(),
            payload,
        };
        let mut draft = EventDraft::new(by, kind);
        if let (Some(agent), Some(run)) = (self.poster_agent(), poster_run) {
            draft = draft.agent(agent).run(run);
        }
        Ok(draft)
    }

    /// The change and record `kind` holds, when it is a notification event.
    pub fn of(kind: &EventKind) -> Option<(NotificationChange, Self)> {
        let EventKind::Custom { name, payload } = kind else {
            return None;
        };
        let change = NotificationChange::of_name(name)?;
        serde_json::from_value(payload.clone())
            .ok()
            .map(|record| (change, record))
    }
}

#[cfg(test)]
mod tests;
