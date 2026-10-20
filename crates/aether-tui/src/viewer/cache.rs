use super::references::{Reference, parse};
use crate::{
    app::transcript::{Cell, CellId, CellRevision, TranscriptEpoch},
    ui::cells::ChatCell,
};
use aether_core::id::AgentId;
use std::{collections::HashMap, sync::Arc};

type Key = (AgentId, TranscriptEpoch, CellId, CellRevision);
const BYTE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ReferenceCache {
    entries: HashMap<Key, Arc<[Reference]>>,
    bytes: usize,
}

impl ReferenceCache {
    pub fn get(&mut self, agent: AgentId, epoch: TranscriptEpoch, cell: &Cell) -> Arc<[Reference]> {
        let key = (agent, epoch, cell.id, cell.revision);
        if let Some(found) = self.entries.get(&key) {
            return found.clone();
        }
        let refs: Arc<[Reference]> = cell
            .model
            .copy_source()
            .map(parse)
            .unwrap_or_default()
            .into();
        let bytes: usize = refs
            .iter()
            .map(|r| r.target.len() + r.label.len() + size_of::<Reference>())
            .sum();
        if bytes > BYTE_LIMIT {
            return refs;
        }
        if self.bytes + bytes > BYTE_LIMIT || self.entries.len() >= 64 {
            self.entries.clear();
            self.bytes = 0;
        }
        self.bytes += bytes;
        self.entries.insert(key, refs.clone());
        debug_assert!(self.bytes <= BYTE_LIMIT);
        debug_assert!(self.entries.len() <= 64);
        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::transcript::{CellModel, CellRevision};

    #[test]
    fn stable_references_are_shared_and_revisions_invalidate() {
        let agent = AgentId::new();
        let epoch = TranscriptEpoch::default();
        let mut cell = Cell {
            id: CellId(1),
            revision: CellRevision(0),
            at: aether_core::time::now(),
            model: CellModel::Assistant {
                text: "first.png".into(),
            },
        };
        let mut cache = ReferenceCache::default();
        let first = cache.get(agent, epoch, &cell);
        assert!(Arc::ptr_eq(&first, &cache.get(agent, epoch, &cell)));
        cell.revision = CellRevision(1);
        cell.model = CellModel::Assistant {
            text: "second.png".into(),
        };
        assert_eq!(cache.get(agent, epoch, &cell)[0].target, "second.png");
        assert!(!Arc::ptr_eq(
            &first,
            &cache.get(agent, TranscriptEpoch::default(), &cell)
        ));
    }
}
