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
        let Some(reads) = self.reads_for(key) else {
            tracing::warn!(
                kept = KEPT,
                "get_tree: every kept request shape is still reading"
            );
            // Waited out like any stale answer, so a client asking again cannot spin.
            tokio::time::sleep(PATIENCE).await;
            return Ok(stale_tree(None));
        };
        let answer = reads.answer(PATIENCE, read).await?;
        Ok(match answer {
            Answer::Fresh(tree) => {
                debug_assert!(tree.stale.is_none(), "a tree read is fresh when built");
                (*tree).clone()
            }
            Answer::Stale(last) => stale_tree(last),
        })
    }

    /// The reads shared by `key`'s requests, or `None` when every kept shape
    /// is still reading: a new shape then starts no read, bounding them at [`KEPT`].
    fn reads_for(&self, key: TreeKey) -> Option<Arc<StatusReads<AgentTree>>> {
        let mut by_request = self.by_request.lock();
        if !by_request.contains_key(&key) && by_request.len() >= KEPT {
            // Idle: no request waits on it and no read it started still runs.
            by_request.retain(|_, reads| Arc::strong_count(reads) > 1 || reads.reading());
            if by_request.len() >= KEPT {
                return None;
            }
        }
        let reads = by_request.entry(key).or_default().clone();
        assert!(
            Arc::strong_count(&reads) >= 2,
            "the map and this request hold it"
        );
        assert!(by_request.len() <= KEPT, "at most KEPT shapes are kept");
        Some(reads)
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
        let first = reads.reads_for(key(None)).expect("room");
        assert!(Arc::ptr_eq(
            &first,
            &reads.reads_for(key(None)).expect("kept")
        ));
        for _ in 1..KEPT {
            drop(reads.reads_for(key(Some(AgentId::new()))));
        }
        assert_eq!(reads.by_request.lock().len(), KEPT);
        drop(
            reads
                .reads_for(key(Some(AgentId::new())))
                .expect("idle shapes make room"),
        );
        let kept = reads.by_request.lock();
        assert!(
            kept.contains_key(&key(None)),
            "a shape still asked for stays"
        );
        assert_eq!(kept.len(), 2, "the idle shapes went for the new one");
    }

    /// Starts a read for `key` that runs until `held` is released, and gives up on it.
    async fn give_up_on_a_held_read(reads: &TreeReads, key: TreeKey, held: &Held) {
        let held = held.clone();
        let shared = reads.reads_for(key).expect("room for the shape");
        let read = move || {
            let _ = held.lock().recv();
            Ok(AgentTree::default())
        };
        let answer = shared
            .answer(std::time::Duration::from_millis(1), read)
            .await;
        assert!(
            matches!(answer, Ok(Answer::Stale(None))),
            "the held read is past its patience"
        );
        assert!(shared.reading(), "and still runs");
    }

    type Held = Arc<Mutex<std::sync::mpsc::Receiver<()>>>;

    #[tokio::test]
    async fn a_shape_whose_caller_gave_up_is_kept_while_its_read_runs() {
        let reads = TreeReads::default();
        let (release, held) = std::sync::mpsc::channel::<()>();
        give_up_on_a_held_read(&reads, key(None), &Arc::new(Mutex::new(held))).await;
        for _ in 1..KEPT {
            drop(reads.reads_for(key(Some(AgentId::new()))));
        }
        drop(
            reads
                .reads_for(key(Some(AgentId::new())))
                .expect("idle shapes make room"),
        );
        assert!(
            reads.by_request.lock().contains_key(&key(None)),
            "a shape still reading is kept, so asking again starts no second read"
        );
        drop(release);
    }

    #[tokio::test]
    async fn a_new_shape_starts_no_read_while_every_kept_one_reads() {
        let reads = TreeReads::default();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(held));
        for _ in 0..KEPT {
            give_up_on_a_held_read(&reads, key(Some(AgentId::new())), &held).await;
        }
        assert!(
            reads.reads_for(key(None)).is_none(),
            "no room for another read"
        );
        let tree = reads
            .answer(key(None), || panic!("no read starts"))
            .await
            .expect("answered");
        assert!(
            tree.stale.is_some() && tree.nodes.is_empty(),
            "stale, with nothing read"
        );
        assert_eq!(reads.by_request.lock().len(), KEPT);
        drop(release);
    }
}
