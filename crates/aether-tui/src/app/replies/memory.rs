//! Replies about operator memories, compaction and the ignored remainder.
use crate::app::{App, Effect, Reply, Then};
use aether_core::memory::MemoryRecord;

impl App {
    pub(super) fn apply_memory_reply(
        &mut self,
        then: Then,
        reply: Reply,
        effects: &mut Vec<Effect>,
    ) {
        match (then, reply) {
            (Then::Memories, Reply::Memories(records)) => self.memories_arrived(records),
            (Then::MemoryCreated { .. }, Reply::Memory(record)) => {
                self.memory_created(&record);
                effects.push(self.memories_request());
            }
            (Then::MemoryCreated { .. }, _) => self.operator_reply_unexpected(),
            (Then::MemoryMutated(action), Reply::Memory(record)) => {
                self.flash(format!(
                    "{action} {}",
                    crate::ui::views::short_id(&self.theme, &record.id)
                ));
                effects.push(self.memories_request());
            }
            (Then::Ignore, _) => {}
            (Then::CompactionAccepted(agent), Reply::Ok) => {
                self.flash(aether_core::event::compaction_request_notice(&format!(
                    "aether events --agent {agent} --follow"
                )));
            }
            (Then::CompactionAccepted(_), _) => {
                self.flash_error(
                    "compaction request returned an unexpected reply; check the transcript",
                );
            }
            (then, reply) => {
                tracing::debug!("unhandled reply {:?} for {:?}", reply, then);
            }
        }
    }

    fn memories_arrived(&mut self, mut records: Vec<MemoryRecord>) {
        // Pinned first, then current, then superseded/forgotten;
        // newest within each group.
        records.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.is_current().cmp(&a.is_current()))
                .then(b.created_at.cmp(&a.created_at))
        });
        self.memories = records;
    }

    fn memory_created(&mut self, record: &MemoryRecord) {
        let replaced = record
            .supersedes
            .map(|id| {
                format!(
                    "; replaced {}",
                    crate::ui::views::short_id(&self.theme, &id)
                )
            })
            .unwrap_or_default();
        self.flash(format!(
            "operator memory {} admitted for {}{replaced}; /memory shows it (not agent-authored)",
            crate::ui::views::short_id(&self.theme, &record.id),
            crate::ui::views::memory::scope_label(&record.scope, &self.theme)
        ));
    }
}
