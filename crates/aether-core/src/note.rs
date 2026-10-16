//! A note a client appends to an agent's transcript without starting a turn.
//!
//! Recorded as a `custom` event named [`NOTE_EVENT`], so a client that
//! predates notes still decodes the log; [`Note::of`] reads one back.

use serde::{Deserialize, Serialize};

use crate::event::{Actor, EventDraft, EventKind};
use crate::id::{AgentId, RunId};

/// The `custom` event name a note is recorded under.
pub const NOTE_EVENT: &str = "note.appended";
/// The most text one note may carry, in bytes.
pub const MAX_NOTE_BYTES: usize = 2048;
pub const MAX_NOTE_LINKS: usize = 8;
pub const MAX_NOTE_LINK_BYTES: usize = 512;

/// Whether the agent's model is shown the note as well as people.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NoteContext {
    /// Only people see it: the model is never given it.
    #[default]
    None,
    /// The agent's next turn is also given it, in a compact digest.
    Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Note {
    /// At most [`MAX_NOTE_BYTES`] bytes.
    pub text: String,
    /// References to show with it (URLs, ids), at most [`MAX_NOTE_LINKS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    #[serde(default)]
    pub context: NoteContext,
}

impl Note {
    /// Why this note cannot be stored, naming the limit it breaks.
    pub fn check(&self) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("a note needs text".into());
        }
        if self.text.len() > MAX_NOTE_BYTES {
            return Err(format!(
                "a note is limited to {MAX_NOTE_BYTES} bytes of text and this one has {}",
                self.text.len()
            ));
        }
        if self.links.len() > MAX_NOTE_LINKS {
            return Err(format!(
                "a note is limited to {MAX_NOTE_LINKS} links and this one has {}",
                self.links.len()
            ));
        }
        let malformed = |link: &String| {
            link.trim().is_empty()
                || link.len() > MAX_NOTE_LINK_BYTES
                || link.chars().any(char::is_control)
        };
        if self.links.iter().any(malformed) {
            return Err(format!(
                "each note link is limited to {MAX_NOTE_LINK_BYTES} bytes, \
                 is not blank and has no control characters"
            ));
        }
        Ok(())
    }

    /// The event appending this note to `agent`'s transcript.
    pub fn draft(&self, agent: AgentId, run: RunId, by: Actor) -> Result<EventDraft, String> {
        self.check()?;
        let payload = serde_json::to_value(self).map_err(|error| error.to_string())?;
        debug_assert!(payload.is_object(), "a note encodes as an object");
        let kind = EventKind::Custom {
            name: NOTE_EVENT.to_owned(),
            payload,
        };
        Ok(EventDraft::new(by, kind).agent(agent).run(run))
    }

    /// The note `kind` records, when it is one. A malformed payload is no note.
    pub fn of(kind: &EventKind) -> Option<Self> {
        let EventKind::Custom { name, payload } = kind else {
            return None;
        };
        if name != NOTE_EVENT {
            return None;
        }
        serde_json::from_value(payload.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str) -> Note {
        Note {
            text: text.to_owned(),
            links: Vec::new(),
            context: NoteContext::None,
        }
    }

    #[test]
    fn a_note_reads_back_from_its_event() {
        let mut written = note("3 items shipped");
        written.links = vec!["https://example.com/pr/1".into()];
        written.context = NoteContext::Digest;
        let draft = written
            .draft(AgentId::new(), RunId::new(), Actor::user())
            .expect("a valid note");
        assert_eq!(Note::of(&draft.kind), Some(written));
        assert!(draft.agent_id.is_some());
    }

    #[test]
    fn context_defaults_to_none_and_links_to_empty() {
        let decoded: Note = serde_json::from_value(serde_json::json!({"text": "hi"})).unwrap();
        assert_eq!(decoded, note("hi"));
    }

    #[test]
    fn an_oversized_note_is_refused_with_the_limit_named() {
        let error = note(&"x".repeat(MAX_NOTE_BYTES + 1)).check().unwrap_err();
        assert!(error.contains(&MAX_NOTE_BYTES.to_string()), "{error}");
        assert!(note(&"x".repeat(MAX_NOTE_BYTES)).check().is_ok());
    }

    #[test]
    fn blank_text_and_bad_links_are_refused() {
        assert!(note("  \n").check().is_err());
        let mut many = note("ok");
        many.links = vec!["a".into(); MAX_NOTE_LINKS + 1];
        assert!(many.check().unwrap_err().contains("links"));
        for bad in ["", "a\nb", &"y".repeat(MAX_NOTE_LINK_BYTES + 1)] {
            let mut linked = note("ok");
            linked.links = vec![bad.to_owned()];
            assert!(linked.check().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn other_custom_events_are_not_notes() {
        let kind = EventKind::Custom {
            name: "memory.feedback".into(),
            payload: serde_json::json!({"text": "hi"}),
        };
        assert_eq!(Note::of(&kind), None);
        let malformed = EventKind::Custom {
            name: NOTE_EVENT.into(),
            payload: serde_json::json!({"links": []}),
        };
        assert_eq!(Note::of(&malformed), None);
    }
}
