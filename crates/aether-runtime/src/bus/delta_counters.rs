//! The last delta number each invocation was given, kept bounded without
//! ever forgetting an invocation that may still stream.

use std::collections::{BTreeMap, HashMap, VecDeque};

use aether_core::id::InvocationId;

#[derive(Debug)]
pub(super) struct DeltaCounters {
    capacity: usize,
    /// Each invocation's last seq, and the tick of its last delta.
    last: HashMap<InvocationId, (u64, u64)>,
    /// Invocations by the tick of their last delta, least recent first.
    by_tick: BTreeMap<u64, InvocationId>,
    /// Invocations whose completion or failure was published, oldest first.
    finished: VecDeque<InvocationId>,
    tick: u64,
}

impl DeltaCounters {
    pub(super) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "at least one counter is kept");
        Self {
            capacity,
            last: HashMap::new(),
            by_tick: BTreeMap::new(),
            finished: VecDeque::new(),
            tick: 0,
        }
    }

    /// The seq of `invocation`'s next delta: 1, then one more each time.
    pub(super) fn next(&mut self, invocation: InvocationId) -> u64 {
        self.tick += 1;
        let tick = self.tick;
        let (seq, previous) = match self.last.get(&invocation) {
            Some(&(seq, previous)) => (seq + 1, Some(previous)),
            None => (1, None),
        };
        if let Some(previous) = previous {
            let moved = self.by_tick.remove(&previous);
            debug_assert_eq!(moved, Some(invocation), "one tick per invocation");
        }
        self.last.insert(invocation, (seq, tick));
        self.by_tick.insert(tick, invocation);
        if self.last.len() > self.capacity {
            self.evict();
        }
        assert_eq!(self.last.len(), self.by_tick.len(), "the two indexes agree");
        seq
    }

    /// `invocation` ended: its counter may go before any that may still stream.
    pub(super) fn finish(&mut self, invocation: InvocationId) {
        if !self.last.contains_key(&invocation) {
            return;
        }
        self.finished.push_back(invocation);
        if self.finished.len() > self.capacity {
            self.finished.pop_front();
        }
    }

    /// Forget one counter: the oldest finished, else the one idle longest.
    fn evict(&mut self) {
        while let Some(done) = self.finished.pop_front() {
            if self.forget(done) {
                return;
            }
        }
        let idle = self.by_tick.first_key_value().map(|(_, id)| *id);
        let forgot = idle.is_some_and(|id| self.forget(id));
        assert!(forgot, "a full set of counters has an idle one");
    }

    fn forget(&mut self, invocation: InvocationId) -> bool {
        let Some((_, tick)) = self.last.remove(&invocation) else {
            return false;
        };
        let removed = self.by_tick.remove(&tick);
        debug_assert_eq!(removed, Some(invocation));
        true
    }

    #[cfg(test)]
    pub(super) fn tracks(&self, invocation: InvocationId) -> bool {
        self.last.contains_key(&invocation)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.last.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_counter_goes_before_one_still_streaming() {
        let mut counters = DeltaCounters::new(2);
        let (streaming, done, newest) = (
            InvocationId::new(),
            InvocationId::new(),
            InvocationId::new(),
        );
        assert_eq!(counters.next(streaming), 1);
        assert_eq!(counters.next(done), 1);
        counters.finish(done);
        assert_eq!(counters.next(streaming), 2);
        assert_eq!(counters.next(newest), 1);
        assert!(counters.tracks(streaming) && !counters.tracks(done));
        assert_eq!(counters.next(streaming), 3, "never renumbered");
    }

    #[test]
    fn with_none_finished_the_one_idle_longest_goes() {
        let mut counters = DeltaCounters::new(2);
        let (idle, busy, newest) = (
            InvocationId::new(),
            InvocationId::new(),
            InvocationId::new(),
        );
        counters.next(idle);
        counters.next(busy);
        counters.next(busy);
        counters.next(newest);
        assert_eq!(counters.len(), 2);
        assert!(!counters.tracks(idle));
        assert_eq!(counters.next(busy), 3);
    }
}
