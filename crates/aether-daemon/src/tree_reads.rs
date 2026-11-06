//! The reads behind `get_tree`, shared like `status`'s: one read per request
//! shape at a time, and past the patience the last tree read, marked stale.

use std::collections::HashMap;
use std::sync::Arc;

use aether_core::id::{AgentId, RunId};
use aether_protocol::{AgentTree, ErrorReply, StatusStale};
use parking_lot::Mutex;

use crate::status_reads::{Answer, PATIENCE, StatusReads};

/// What makes two `get_tree` requests the same read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TreeKey {
    pub(crate) run: Option<RunId>,
    pub(crate) include_archived: bool,
    pub(crate) role_prefix: Option<String>,
    pub(crate) root: Option<AgentId>,
}

/// How many request shapes keep their last tree; past it the idle ones go.
const KEPT: usize = 64;

#[derive(Debug, Default)]
pub(crate) struct TreeReads {
    by_request: Mutex<HashMap<TreeKey, Arc<StatusReads<AgentTree>>>>,
}

impl TreeReads {
    /// The tree `read` builds, if one started after this call finishes within
    /// the patience; else the last one for `key`, marked stale.
    pub(crate) async fn answer(
        &self,
        key: TreeKey,
        read: impl Fn() -> Result<AgentTree, ErrorReply> + Send + Sync + 'static,
    ) -> Result<AgentTree, ErrorReply> {
        let reads = self.reads_for(key);
        let answer = reads.answer(PATIENCE, read).await?;
        Ok(match answer {
            Answer::Fresh(tree) => {
                debug_assert!(tree.stale.is_none(), "a tree read is fresh when built");
                (*tree).clone()
            }
            Answer::Stale(last) => stale_tree(last),
        })
    }

    fn reads_for(&self, key: TreeKey) -> Arc<StatusReads<AgentTree>> {
        let mut by_request = self.by_request.lock();
        if !by_request.contains_key(&key) && by_request.len() >= KEPT {
            // Only this map holds an idle one: no request is waiting on it.
            by_request.retain(|_, reads| Arc::strong_count(reads) > 1);
        }
        let reads = by_request.entry(key).or_default().clone();
        assert!(
            Arc::strong_count(&reads) >= 2,
            "the map and this request hold it"
        );
        reads
    }
}

/// The last tree read, or an empty one when none finished, marked stale.
fn stale_tree(last: Option<(aether_core::time::Timestamp, Arc<AgentTree>)>) -> AgentTree {
    let read_at = last.as_ref().map(|(at, _)| *at);
    let mut tree = last.map(|(_, tree)| (*tree).clone()).unwrap_or_default();
    assert!(tree.stale.is_none(), "only a finished read is kept");
    tree.stale = Some(StatusStale {
        read_at,
        waited_ms: PATIENCE.as_millis() as u64,
    });
    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(root: Option<AgentId>) -> TreeKey {
        TreeKey {
            run: None,
            include_archived: false,
            role_prefix: None,
            root,
        }
    }

    #[test]
    fn a_stale_tree_with_nothing_read_is_empty_and_says_so() {
        let tree = stale_tree(None);
        assert!(tree.nodes.is_empty() && tree.roots.is_empty());
        let stale = tree.stale.expect("marked stale");
        assert_eq!(stale.read_at, None, "nothing was read");
        assert_eq!(stale.waited_ms, PATIENCE.as_millis() as u64);
    }

    #[test]
    fn one_request_shape_shares_one_read_and_idle_shapes_make_room() {
        let reads = TreeReads::default();
        let first = reads.reads_for(key(None));
        assert!(Arc::ptr_eq(&first, &reads.reads_for(key(None))));
        for _ in 1..KEPT {
            drop(reads.reads_for(key(Some(AgentId::new()))));
        }
        assert_eq!(reads.by_request.lock().len(), KEPT);
        drop(reads.reads_for(key(Some(AgentId::new()))));
        let kept = reads.by_request.lock();
        assert!(
            kept.contains_key(&key(None)),
            "a shape still asked for stays"
        );
        assert_eq!(kept.len(), 2, "the idle shapes went for the new one");
    }
}
