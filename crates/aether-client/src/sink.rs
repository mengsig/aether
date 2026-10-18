//! Where a command goes: the daemon in production, a double in tests.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use aether_protocol::{Command, Reply};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::Client;

/// One command in, one reply out. Everything that drives a fleet from
/// outside - the MCP server, `aether serve` - is written against this so it
/// can be tested without a daemon.
#[async_trait]
pub trait CommandSink: Send + Sync {
    async fn call(&self, command: Command) -> anyhow::Result<Reply>;
}

/// The daemon over its own connection.
#[derive(Debug)]
pub struct DaemonSink(pub Client);

#[async_trait]
impl CommandSink for DaemonSink {
    async fn call(&self, command: Command) -> anyhow::Result<Reply> {
        Ok(self.0.request(command).await?)
    }
}

/// The daemon could not be reached; a retry later may succeed. A caller that
/// serves other parties downcasts to this to answer "try again" rather than
/// "no".
#[derive(Debug, thiserror::Error)]
#[error("the daemon is unavailable: {0}")]
pub struct DaemonUnavailable(pub String);

const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Connector =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<Client>> + Send>> + Send + Sync>;

struct Link {
    client: Option<Client>,
    /// Which connection `client` is; bumped on every reconnect.
    generation: u64,
    failures: u32,
    retry_at: Option<Instant>,
}

/// A daemon connection that is re-made, lazily and with bounded backoff, when
/// the daemon goes away. A command that finds no daemon fails with
/// [`DaemonUnavailable`] and is never replayed: the daemon may have acted on
/// it before the connection dropped.
pub struct ReconnectingSink {
    connect: Connector,
    link: Mutex<Link>,
}

impl std::fmt::Debug for ReconnectingSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReconnectingSink").finish_non_exhaustive()
    }
}

impl ReconnectingSink {
    /// `client` is the connection already made; `connect` makes the next one.
    pub fn new<F, Fut>(client: Client, connect: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<Client>> + Send + 'static,
    {
        Self {
            connect: Box::new(move || Box::pin(connect())),
            link: Mutex::new(Link {
                client: Some(client),
                generation: 0,
                failures: 0,
                retry_at: None,
            }),
        }
    }

    async fn current(&self) -> Result<(Client, u64), DaemonUnavailable> {
        let mut link = self.link.lock().await;
        if let Some(client) = link.client.as_ref().filter(|client| client.is_connected()) {
            return Ok((client.clone(), link.generation));
        }
        link.client = None;
        if link.retry_at.is_some_and(|at| Instant::now() < at) {
            return Err(DaemonUnavailable(
                "reconnecting after a failed attempt".into(),
            ));
        }
        let attempt = match tokio::time::timeout(CONNECT_TIMEOUT, (self.connect)()).await {
            Ok(attempt) => attempt,
            Err(_) => Err(anyhow::anyhow!("connecting timed out")),
        };
        match attempt {
            Ok(client) => {
                tracing::info!("reconnected to the daemon");
                link.failures = 0;
                link.retry_at = None;
                link.generation += 1;
                link.client = Some(client.clone());
                Ok((client, link.generation))
            }
            Err(error) => {
                link.failures = link.failures.saturating_add(1);
                let wait = backoff(link.failures);
                link.retry_at = Some(Instant::now() + wait);
                tracing::warn!(%error, retry_in = ?wait, "cannot reach the daemon");
                Err(DaemonUnavailable(format!("{error:#}")))
            }
        }
    }
}

/// Doubles from [`FIRST_BACKOFF`] to [`MAX_BACKOFF`]; `failures` counts from 1.
fn backoff(failures: u32) -> Duration {
    assert!(failures >= 1, "backoff follows a failure");
    let wait = FIRST_BACKOFF.saturating_mul(1 << (failures - 1).min(16));
    wait.min(MAX_BACKOFF)
}

#[async_trait]
impl CommandSink for ReconnectingSink {
    async fn call(&self, command: Command) -> anyhow::Result<Reply> {
        let (client, generation) = self.current().await?;
        match client.request(command).await {
            Ok(reply) => Ok(reply),
            Err(error) if error.is_connection_failure() => {
                tracing::warn!(%error, "the daemon connection dropped");
                // Only the connection that failed: a request that was in
                // flight on an old one must not discard its replacement.
                let mut link = self.link.lock().await;
                if link.generation == generation {
                    link.client = None;
                }
                Err(DaemonUnavailable(error.to_string()).into())
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_is_capped() {
        assert_eq!(backoff(1), FIRST_BACKOFF);
        assert_eq!(backoff(2), FIRST_BACKOFF * 2);
        assert_eq!(backoff(7), MAX_BACKOFF);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }
}
