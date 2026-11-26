//! Entries that arrived while the operator was not interacting: the `N new`
//! divider before the first unseen cell and the `N new ↓` footer chip.
//!
//! The operator "has seen" a transcript up to the newest cell at their last
//! input (key, paste, mouse or send). Cells committed after that and before
//! the next input are unseen; the next input clears them.

use std::collections::HashMap;

use aether_core::id::AgentId;

use super::App;
use super::transcript::{AgentTranscript, CellId, CellModel, UserFrom};

/// The first unseen cell of one transcript and how many cells are unseen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unseen {
    pub first: CellId,
    pub count: usize,
}

#[derive(Debug, Default)]
pub struct Arrivals {
    /// The newest cell id the operator saw, per transcript.
    seen: HashMap<AgentId, CellId>,
    unseen: HashMap<AgentId, Unseen>,
}

impl Arrivals {
    pub fn unseen(&self, agent: AgentId) -> Option<Unseen> {
        self.unseen.get(&agent).copied()
    }

    /// What the operator's own message or a turn boundary adds is no news.
    fn counts(model: &CellModel) -> bool {
        !matches!(
            model,
            CellModel::User {
                from: UserFrom::Operator,
                ..
            } | CellModel::Separator { .. }
        )
    }

    fn newest(transcript: &AgentTranscript) -> Option<CellId> {
        transcript.cells.last().map(|cell| cell.id)
    }

    /// Everything on screen is seen: the divider and the count clear.
    fn rebase(&mut self, agent: AgentId, transcript: &AgentTranscript) {
        self.unseen.remove(&agent);
        match Self::newest(transcript) {
            Some(id) => self.seen.insert(agent, id),
            None => self.seen.remove(&agent),
        };
    }

    fn track(&mut self, agent: AgentId, transcript: &AgentTranscript) {
        let Some(newest) = Self::newest(transcript) else {
            return;
        };
        let Some(&seen) = self.seen.get(&agent) else {
            return self.rebase(agent, transcript);
        };
        // Ids restart after a rebuild: what was seen is then unknowable.
        if seen > newest {
            return self.rebase(agent, transcript);
        }
        let mut fresh = transcript
            .cells
            .iter()
            .rev()
            .take_while(|cell| cell.id > seen)
            .filter(|cell| Self::counts(&cell.model))
            .map(|cell| cell.id)
            .collect::<Vec<_>>();
        match fresh.pop() {
            Some(first) => {
                let count = fresh.len() + 1;
                self.unseen.insert(agent, Unseen { first, count });
            }
            None => {
                self.unseen.remove(&agent);
            }
        }
    }
}

impl App {
    /// After an event: the viewed transcript's new cells are unseen, unless
    /// the event was the operator's own input, which has seen them all.
    pub(super) fn track_arrivals(&mut self, interacted: bool) {
        let Some(agent) = self.selected else {
            return;
        };
        let Some(transcript) = self.transcripts.get(&agent).filter(|t| t.loaded) else {
            return;
        };
        if interacted {
            self.arrivals.rebase(agent, transcript);
        } else {
            self.arrivals.track(agent, transcript);
        }
    }

    /// Mouse input arrives outside the reducer.
    pub(crate) fn note_mouse_input(&mut self) {
        self.track_arrivals(true);
    }
}

#[cfg(test)]
mod tests;
