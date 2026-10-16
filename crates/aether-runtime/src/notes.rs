//! Notes a client appends to an agent's transcript: stored, shown, never a turn.

use std::collections::VecDeque;

use aether_core::event::{Actor, Event, EventKind};
use aether_core::id::AgentId;
use aether_core::note::{Note, NoteContext};
use aether_store::{Commit, EventFilter};

use crate::{Result, Runtime, RuntimeError};

/// The most notes one turn's digest carries; older ones are only counted.
pub const MAX_DIGEST_NOTES: usize = 10;

/// The notes a turn is given, oldest first, and how many older ones it is not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteDigest {
    pub notes: Vec<Note>,
    pub omitted: usize,
}

impl Runtime {
    /// Record `note` on `agent`'s transcript. Nothing is queued in its inbox,
    /// so the scheduler has no reason to start a turn for it.
    pub fn append_note(&self, agent: AgentId, note: &Note, by: Actor) -> Result<u64> {
        note.check().map_err(RuntimeError::Invalid)?;
        let hold = self.hold();
        let target = self
            .store
            .agent(agent)?
            .ok_or_else(|| RuntimeError::NotFound(agent.to_string()))?;
        let draft = note
            .draft(target.id, target.run, by)
            .map_err(RuntimeError::Invalid)?;
        let events = self.apply_held(hold, Commit::new().event(draft))?;
        let [event] = events.as_slice() else {
            return Err(RuntimeError::Internal(format!(
                "a note committed {} events, not one",
                events.len()
            )));
        };
        debug_assert_eq!(event.agent_id, Some(agent));
        Ok(event.seq)
    }
}

/// The `digest` notes appended to `agent` between its previous turn's start
/// and the current one's, so each reaches exactly one turn.
pub(crate) fn digest(runtime: &Runtime, agent: AgentId) -> Result<NoteDigest> {
    const PAGE: usize = 256;
    let started = runtime.store.latest_events(
        2,
        &EventFilter {
            types: vec!["agent.started".into()],
            ..EventFilter::own_events(agent)
        },
    )?;
    // The turn being assembled has already committed its own start.
    let Some(current) = started.first() else {
        return Ok(NoteDigest::default());
    };
    let previous = started.get(1).map_or(0, |event| event.seq);
    debug_assert!(previous < current.seq, "newest first");
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::own_events(agent)
    };
    let mut kept: VecDeque<Note> = VecDeque::with_capacity(MAX_DIGEST_NOTES);
    let mut omitted = 0;
    let mut after = previous;
    loop {
        let page = runtime.store.events(after, PAGE, &filter)?;
        let full = page.len() == PAGE;
        for event in page.iter().take_while(|event| event.seq < current.seq) {
            let Some(note) = digest_note(event) else {
                continue;
            };
            if kept.len() == MAX_DIGEST_NOTES {
                kept.pop_front();
                omitted += 1;
            }
            kept.push_back(note);
        }
        match page.last() {
            Some(last) if full && last.seq < current.seq => after = last.seq,
            _ => break,
        }
    }
    debug_assert!(kept.len() <= MAX_DIGEST_NOTES);
    Ok(NoteDigest {
        notes: kept.into(),
        omitted,
    })
}

fn digest_note(event: &Event) -> Option<Note> {
    debug_assert!(matches!(event.kind, EventKind::Custom { .. }));
    Note::of(&event.kind).filter(|note| note.context == NoteContext::Digest)
}
