//! In-process fan-out of committed events and transient streaming deltas.
//!
//! The bus is a fast path, not the source of truth. A subscriber that needs
//! durability replays from the store using sequence numbers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aether_core::event::Event;
use aether_core::id::{AgentId, InvocationId};
use aether_core::model::Usage;
use tokio::sync::broadcast;

use crate::noesis_seal::StreamSeal;

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
        /// Assigned once by its producer, before any subscriber sees it.
        seq: u64,
        delta: DeltaKind,
    },
}

/// One invocation's deltas, numbered by the one producer that streams them.
/// The producer owns the count, so it lives exactly as long as the stream can.
#[derive(Debug)]
pub struct DeltaStream {
    invocation: InvocationId,
    last: u64,
    /// Which deltas this stream withholds, decided once when it is made.
    seal: StreamSeal,
}

impl DeltaStream {
    pub fn new(invocation: InvocationId, seal: StreamSeal) -> Self {
        Self {
            invocation,
            last: 0,
            seal,
        }
    }

    /// A fresh stream for the next invocation of the same agent, sealed alike.
    pub fn next_invocation(&self) -> Self {
        Self::new(InvocationId::new(), self.seal)
    }

    pub fn invocation(&self) -> InvocationId {
        self.invocation
    }

    /// The seq the next delta takes: 1 for the first, one more each after.
    fn advance(&mut self) -> u64 {
        assert!(
            self.last < u64::MAX,
            "an invocation streams fewer than u64::MAX deltas"
        );
        let seq = self.last + 1;
        debug_assert!(seq >= 1, "the first delta is numbered 1");
        self.last = seq;
        seq
    }
}

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<BusMessage>,
    last_seq: Arc<AtomicU64>,
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

    /// Number `delta` within its stream and fan it out. One producer owns a
    /// stream, so the channel carries its deltas in seq order. A delta the
    /// stream's seal withholds is dropped before it takes a number.
    pub fn publish_delta(&self, agent: AgentId, stream: &mut DeltaStream, delta: DeltaKind) {
        if !stream.seal.admits(&delta) {
            return;
        }
        let seq = stream.advance();
        // No receivers is not an error: deltas are transient, and the gap a
        // later subscriber sees is how it knows to read the transcript.
        let _ = self.tx.send(BusMessage::Delta {
            agent,
            invocation: stream.invocation,
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
        let mut first = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
        let mut second = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
        for n in 0..1000 {
            bus.publish_delta(agent, &mut first, text(n));
            if n % 3 == 0 {
                bus.publish_delta(agent, &mut second, text(n));
            }
        }
        assert_eq!(
            seqs_of(&mut rx, first.invocation()),
            (1..=1000).collect::<Vec<_>>()
        );
        let mut rx = bus.subscribe();
        bus.publish_delta(agent, &mut second, DeltaKind::TurnFinished);
        assert_eq!(
            seqs_of(&mut rx, second.invocation()),
            vec![335],
            "334 numbered before it"
        );
    }

    #[test]
    fn two_subscriptions_see_the_same_numbers_and_a_late_one_is_never_renumbered() {
        let bus = EventBus::new();
        let agent = AgentId::new();
        let mut stream = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
        let mut early = bus.subscribe();
        let mut other = bus.subscribe();
        for n in 0..5 {
            bus.publish_delta(agent, &mut stream, text(n));
        }
        let mut late = bus.subscribe();
        for n in 5..8 {
            bus.publish_delta(agent, &mut stream, text(n));
        }
        let all: Vec<u64> = (1..=8).collect();
        let invocation = stream.invocation();
        assert_eq!(seqs_of(&mut early, invocation), all);
        assert_eq!(seqs_of(&mut other, invocation), all);
        assert_eq!(seqs_of(&mut late, invocation), vec![6, 7, 8]);
    }

    #[test]
    fn a_delta_lost_to_a_lagging_subscriber_shows_as_a_gap() {
        let bus = EventBus::with_capacity(4);
        let mut rx = bus.subscribe();
        let agent = AgentId::new();
        let mut stream = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
        for n in 0..10 {
            bus.publish_delta(agent, &mut stream, text(n));
        }
        // The channel kept the newest four; the six before them are lost.
        let seen = seqs_of(&mut rx, stream.invocation());
        assert_eq!(seen, vec![7, 8, 9, 10]);
        assert!(seen[0] > 1, "the first one seen says what was missed");
    }

    #[test]
    fn a_long_silent_stream_keeps_its_count_however_many_others_ran() {
        let bus = EventBus::new();
        let agent = AgentId::new();
        let mut silent = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
        bus.publish_delta(agent, &mut silent, text(0));
        for n in 0..100_000 {
            let mut other = DeltaStream::new(InvocationId::new(), StreamSeal::Ordinary);
            bus.publish_delta(agent, &mut other, text(n));
        }
        let mut rx = bus.subscribe();
        bus.publish_delta(agent, &mut silent, DeltaKind::TurnFinished);
        assert_eq!(seqs_of(&mut rx, silent.invocation()), vec![2]);
    }
}
