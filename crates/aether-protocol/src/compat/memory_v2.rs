//! What a client that did not list `features::MEMORY_V2` must not see: a
//! memory reply reaches it without `standing`, byte for byte as before.

use crate::{Reply, ServerFrame};

/// Drop `standing` from every record in `frame`. Whether anything was dropped.
pub fn withhold_memory_standing(frame: &mut ServerFrame) -> bool {
    let ServerFrame::Reply {
        result: Ok(reply), ..
    } = frame
    else {
        return false;
    };
    let records = match reply {
        Reply::Memories(records) => records.as_mut_slice(),
        Reply::Memory(record) => std::slice::from_mut(record),
        _ => return false,
    };
    let changed = records.iter().any(|record| record.standing.is_some());
    for record in records.iter_mut() {
        record.standing = None;
    }
    debug_assert!(records.iter().all(|record| record.standing.is_none()));
    changed
}
