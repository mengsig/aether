//! `aether serve`: the outside world's two doors into a fleet.
//!
//! In: a signed POST on a configured path becomes an external trigger on a
//! named agent's inbox - the same inbox item `aether trigger` writes, so a
//! webhook wakes an agent exactly the way a person does. The signature
//! covers a timestamp and a nonce as well as the body, and both are checked,
//! so a captured POST cannot be delivered a second time.
//!
//! Out: an event subscription posts to a URL, signed the same way, with
//! bounded retries and a dead letter holding whatever never landed.
//!
//! Both doors are written against [`CommandSink`] and a plain event stream,
//! so the tests drive them without a daemon.

mod headers;
mod inbound;
mod outbound;
pub mod replay;
mod request;
pub mod signature;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_core::event::Event;
use anyhow::Context as _;
use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::sink::CommandSink;

pub use inbound::{Webhook, serve_webhooks};
pub use outbound::{Delivery, Stopped, Subscription, deliver};

/// Everything `aether serve` runs.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Where the inbound listener binds. `None` runs the outbound half only.
    pub address: Option<SocketAddr>,
    /// Bind outside loopback. Off by default: a webhook creates work on this
    /// machine, and Aether terminates no TLS.
    pub allow_remote: bool,
    /// Durable replay state; shared by every restart of this door.
    pub replay_path: PathBuf,
    pub webhooks: Vec<Webhook>,
    pub subscriptions: Vec<Subscription>,
}

/// How long an inbound peer may take to send its request head.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an inbound peer may take to deliver its body once the head has
/// arrived. Without it a peer that sends one byte a minute holds a slot.
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest webhook body accepted. A trigger payload is small; a larger one is
/// a mistake or an attack, and reading it would be the vulnerability.
pub const MAX_BODY_BYTES: usize = 1 << 20;

/// Inbound connections served at once.
pub const MAX_CONNECTIONS: usize = 32;

/// Unauthenticated requests worked on at once: the bad-signature delay and
/// the body-read wait happen inside this smaller budget rather than tying up
/// a general connection slot for the whole [`BODY_READ_TIMEOUT`], so a flood
/// of unsigned requests costs far less than [`MAX_CONNECTIONS`] slots' worth
/// of that wait.
pub const PRE_AUTH_SLOTS: usize = 8;

/// Run both halves until `shutdown`. `bound` is called with the inbound
/// address once it is listening, so a caller that asked for port 0 learns it.
pub async fn run<S>(
    sink: Arc<dyn CommandSink>,
    config: ServeConfig,
    events: S,
    shutdown: CancellationToken,
    bound: impl FnOnce(Option<SocketAddr>),
) -> anyhow::Result<()>
where
    S: Stream<Item = Event> + Send + 'static,
{
    if config.webhooks.is_empty() && config.subscriptions.is_empty() {
        anyhow::bail!(
            "[serve] configures no webhook and no subscription, so there is nothing to serve"
        );
    }
    // Raced, not sequenced. Awaiting the halves in order meant a
    // configuration error the inbound half already had in hand waited on an
    // event stream that never ends, and an outbound half that had stopped
    // delivering was never noticed at all.
    //
    // A half that has nothing to do (no listener, no subscription) returns at
    // once, so each waits for shutdown afterwards rather than deciding the
    // race by being trivially finished.
    let posting = async {
        let stopped =
            outbound::deliver(config.subscriptions.clone(), events, shutdown.clone()).await?;
        if stopped == Stopped::StreamEnded {
            anyhow::bail!("the event stream from the daemon ended; nothing is being delivered");
        }
        shutdown.cancelled().await;
        anyhow::Ok(())
    };
    let serving = async {
        inbound::serve_webhooks(sink, &config, shutdown.clone(), bound).await?;
        shutdown.cancelled().await;
        anyhow::Ok(())
    };
    futures::pin_mut!(posting, serving);
    let outcome: anyhow::Result<()> = tokio::select! {
        served = &mut serving => served.context("aether serve: the webhook listener stopped"),
        posted = &mut posting => posted.context("aether serve: event delivery stopped"),
    };
    // The other half has nothing left to serve. Cancelling tells anything it
    // spawned to stop; dropping it here ends the half itself.
    shutdown.cancel();
    outcome
}

/// Where a delivery that never landed is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetter(pub PathBuf);
