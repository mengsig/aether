use crate::{Client, ClientError, Result, expect_ok, unexpected};
use aether_core::event::Event;
use aether_core::id::{AgentId, InvocationId};
use aether_protocol::{Command, Delta, EventQuery, Reply};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// Frames delivered to a subscription.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "keep streamed accounting snapshots inline without a per-frame allocation"
)]
pub enum SubscriptionFrame {
    Event(Box<Event>),
    /// Terminal overflow. Resume after the last consumed event sequence, or
    /// [`Subscription::last_seq`] if none. Live deltas cannot be replayed.
    Lagged {
        capacity: usize,
    },
    Delta {
        agent: AgentId,
        invocation: InvocationId,
        delta: Delta,
    },
}

impl Client {
    /// Open a subscription. A full queue ends with [`SubscriptionFrame::Lagged`];
    /// resume after the last event consumed. Live deltas cannot be replayed.
    pub async fn subscribe(
        &self,
        after: Option<u64>,
        query: EventQuery,
        deltas: bool,
    ) -> Result<Subscription> {
        let mut response = self
            .request_inner(Command::Subscribe {
                after,
                query,
                deltas,
            })
            .await?;
        match response.reply {
            Reply::Subscribed { id, last_seq } => {
                let rx = self
                    .shared
                    .sub_receivers
                    .lock()
                    .await
                    .remove(&id)
                    .ok_or_else(|| ClientError::Unexpected {
                        expected: "subscription channel",
                        got: format!("no channel for subscription {id}"),
                    })?;
                if let Some(subscription) = &mut response.subscription {
                    subscription.claimed = true;
                }
                Ok(Subscription {
                    id,
                    last_seq,
                    rx,
                    client: self.clone(),
                    released: false,
                    runtime: tokio::runtime::Handle::current(),
                })
            }
            other => Err(unexpected("subscribed", &other)),
        }
    }

    pub async fn unsubscribe(&self, id: u64) -> Result<()> {
        self.shared.subscriptions.lock().await.remove(&id);
        expect_ok(self.request(Command::Unsubscribe { id }).await?)
    }
}

/// A live subscription. Ends on disconnect or cancellation. Overflow yields
/// [`SubscriptionFrame::Lagged`] after buffered frames, then ends.
pub struct Subscription {
    id: u64,
    last_seq: u64,
    rx: mpsc::Receiver<SubscriptionFrame>,
    client: Client,
    released: bool,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .finish()
    }
}

impl Subscription {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Initial replay cursor: requested `after`, or the daemon's latest sequence.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub async fn next(&mut self) -> Option<SubscriptionFrame> {
        self.rx.recv().await
    }

    /// Tell the daemon to stop; frames already in flight are discarded.
    pub async fn unsubscribe(mut self) -> Result<()> {
        let result = self.client.unsubscribe(self.id).await;
        self.released = result.is_ok();
        result
    }
}

impl Stream for Subscription {
    type Item = SubscriptionFrame;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if !self.released {
            super::unsubscribe_abandoned(
                &self.client.shared,
                &self.client.sink.downgrade(),
                self.id,
                &self.runtime,
            );
        }
    }
}
