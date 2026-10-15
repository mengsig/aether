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
        delta: DeltaKind,
    },
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
        let (tx, _) = broadcast::channel(8192);
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

    pub fn publish_delta(&self, agent: AgentId, invocation: InvocationId, delta: DeltaKind) {
        let _ = self.tx.send(BusMessage::Delta {
            agent,
            invocation,
            delta,
        });
    }
}
