//! In-process fan-out of committed events and transient streaming deltas.
//!
//! The bus is a fast path, not the source of truth. A subscriber that needs
//! durability replays from the store using sequence numbers.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aether_core::event::Event;
use aether_core::id::{AgentId, InvocationId};
use aether_core::model::Usage;
use tokio::sync::broadcast;

/// Transient streaming signal. Never persisted.
#[derive(Debug, Clone, PartialEq)]
pub enum DeltaKind {
    TurnStarted {
        turn: u32,
    },
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolInput {
        name: String,
        partial_json: String,
        /// The tool call's id, when the stream named it.
        tool_use_id: Option<String>,
    },
    /// Absolute in-memory figures, ahead of the next durable accounting event.
    Accounting {
        /// Last committed event published before this reading, for replay ordering.
        after_seq: u64,
        total: Usage,
        turn_usage: Usage,
        context_tokens: Option<u64>,
    },
    TurnFinished,
}

#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "keep streamed accounting snapshots inline without a per-frame allocation"
)]
pub enum BusMessage {
    Event(Arc<Event>),
    Delta {
        agent: AgentId,
        invocation: InvocationId,
        /// 1 for the invocation's first delta, one more for each after it.
        /// Assigned once here, before any subscriber sees or filters it.
        seq: u64,
        delta: DeltaKind,
    },
}

/// Invocations whose delta counters are kept. Ids are time-ordered, so the
/// one dropped past this is the oldest: long finished in any real fleet.
const TRACKED_INVOCATIONS: usize = 65_536;

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<BusMessage>,
    last_seq: Arc<AtomicU64>,
    /// The last delta seq each invocation was given.
    delta_seqs: Arc<Mutex<BTreeMap<InvocationId, u64>>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_capacity(8192)
    }

    /// A bus whose subscribers lag after `capacity` unread messages.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            last_seq: Arc::new(AtomicU64::new(0)),
            delta_seqs: Arc::default(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.tx.subscribe()
    }

    pub fn publish_event(&self, event: Event) {
        self.last_seq.fetch_max(event.seq, Ordering::SeqCst);
        // No receivers is not an error: the store already has the event.
        let _ = self.tx.send(BusMessage::Event(Arc::new(event)));
    }

    pub(crate) fn last_seq(&self) -> u64 {
        self.last_seq.load(Ordering::SeqCst)
    }

    /// Number `delta` within its invocation and fan it out. The lock is held
    /// across the send, so the channel carries each invocation's deltas in
    /// seq order.
    pub fn publish_delta(&self, agent: AgentId, invocation: InvocationId, delta: DeltaKind) {
        // A poisoned counter map is only ever mid-increment: its numbers still hold.
        let mut seqs = self
            .delta_seqs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = {
            let last = seqs.entry(invocation).or_insert(0);
            *last += 1;
            *last
        };
        if seqs.len() > TRACKED_INVOCATIONS {
            seqs.pop_first();
        }
        debug_assert!(seq >= 1 && seqs.len() <= TRACKED_INVOCATIONS);
        // No receivers is not an error: deltas are transient, and the gap a
        // later subscriber sees is how it knows to read the transcript.
        let _ = self.tx.send(BusMessage::Delta {
            agent,
            invocation,
            seq,
            delta,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    fn text(n: usize) -> DeltaKind {
        DeltaKind::Text {
            text: n.to_string(),
        }
    }

    fn seqs_of(rx: &mut broadcast::Receiver<BusMessage>, invocation: InvocationId) -> Vec<u64> {
        let mut seen = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(BusMessage::Delta {
                    invocation: of,
                    seq,
                    ..
                }) if of == invocation => seen.push(seq),
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => return seen,
            }
        }
    }

    #[test]
    fn deltas_are_numbered_gaplessly_per_invocation_across_a_thousand() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let agent = AgentId::new();
        let (first, second) = (InvocationId::new(), InvocationId::new());
        for n in 0..1000 {
            bus.publish_delta(agent, first, text(n));
            if n % 3 == 0 {
                bus.publish_delta(agent, second, text(n));
            }
        }
        assert_eq!(seqs_of(&mut rx, first), (1..=1000).collect::<Vec<_>>());
        let mut rx = bus.subscribe();
        bus.publish_delta(agent, second, DeltaKind::TurnFinished);
        assert_eq!(
            seqs_of(&mut rx, second),
            vec![335],
            "334 numbered before it"
        );
    }

    #[test]
    fn two_subscriptions_see_the_same_numbers_and_a_late_one_is_never_renumbered() {
        let bus = EventBus::new();
        let agent = AgentId::new();
        let invocation = InvocationId::new();
        let mut early = bus.subscribe();
        let mut other = bus.subscribe();
        for n in 0..5 {
            bus.publish_delta(agent, invocation, text(n));
        }
        let mut late = bus.subscribe();
        for n in 5..8 {
            bus.publish_delta(agent, invocation, text(n));
        }
        let all: Vec<u64> = (1..=8).collect();
        assert_eq!(seqs_of(&mut early, invocation), all);
        assert_eq!(seqs_of(&mut other, invocation), all);
        assert_eq!(seqs_of(&mut late, invocation), vec![6, 7, 8]);
    }

    #[test]
    fn a_delta_lost_to_a_lagging_subscriber_shows_as_a_gap() {
        let bus = EventBus::with_capacity(4);
        let mut rx = bus.subscribe();
        let agent = AgentId::new();
        let invocation = InvocationId::new();
        for n in 0..10 {
            bus.publish_delta(agent, invocation, text(n));
        }
        // The channel kept the newest four; the six before them are lost.
        let seen = seqs_of(&mut rx, invocation);
        assert_eq!(seen, vec![7, 8, 9, 10]);
        assert!(seen[0] > 1, "the first one seen says what was missed");
    }

    #[test]
    fn the_counters_kept_are_bounded_and_the_oldest_goes_first() {
        let bus = EventBus::new();
        let agent = AgentId::new();
        let oldest = InvocationId::new();
        bus.publish_delta(agent, oldest, text(0));
        // Ids within one millisecond are unordered; the rest must be newer.
        std::thread::sleep(std::time::Duration::from_millis(2));
        for _ in 0..TRACKED_INVOCATIONS {
            bus.publish_delta(agent, InvocationId::new(), text(0));
        }
        let seqs = bus.delta_seqs.lock().unwrap();
        assert_eq!(seqs.len(), TRACKED_INVOCATIONS);
        assert!(!seqs.contains_key(&oldest));
    }
}
