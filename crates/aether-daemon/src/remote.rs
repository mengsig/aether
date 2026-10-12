//! The daemon's side of remote control: serves the protocol on the carrier's streams behind a
//! device gate, and keeps what remote access needs to survive a restart.

mod admin;
mod devices;

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use aether_model::config::{ConfigError, RemoteSection};
use aether_protocol::remote::RemoteSettings;
use aether_remote::push::Notifier;
use aether_remote::{DeviceGate, Ended, RemoteConfig, RemoteHost, RemoteServer};
use aether_runtime::Runtime;
use tokio::io::DuplexStream;
use tokio_util::sync::CancellationToken;

use crate::listen::{ConnectionPolicy, HANDSHAKE_TIMEOUT};
use crate::{Daemon, connection};

pub use admin::AdminError;

/// The `[remote]` stanza as the listener takes it. Refused here, at start,
/// rather than when the operator first runs `aether remote enable`.
pub fn configured(section: &RemoteSection) -> anyhow::Result<RemoteConfig> {
    let bad = |reason: String| ConfigError {
        path: "config.toml".into(),
        reason,
    };
    let config = RemoteConfig {
        address: section.address().map_err(bad)?,
        allow_remote: section.allow_remote,
        max_connections: section.max_connections,
        idle_timeout: Duration::from_secs(section.idle_timeout_secs),
        push_previews: section.push_previews,
        push_subject: section.push_subject().map_err(bad)?.map(str::to_owned),
    };
    aether_remote::listener::check(&config)?;
    Ok(config)
}

/// Serves carrier connections as device connections of one daemon.
pub struct DeviceConnections {
    /// Weak: the daemon owns the listener that owns this.
    daemon: Weak<Daemon>,
    instance_id: String,
    config: RemoteConfig,
}

/// Manual: the runtime behind the daemon prints far more than a log line wants.
impl std::fmt::Debug for DeviceConnections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceConnections")
            .field("instance_id", &self.instance_id)
            .field("config", &self.config)
            .finish()
    }
}

impl DeviceConnections {
    pub fn new(daemon: &Arc<Daemon>, config: RemoteConfig) -> Self {
        Self {
            daemon: Arc::downgrade(daemon),
            instance_id: daemon.instance_id.clone(),
            config,
        }
    }
}

#[async_trait::async_trait]
impl RemoteHost for DeviceConnections {
    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn serve(
        &self,
        stream: DuplexStream,
        gate: DeviceGate,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Ended> {
        anyhow::ensure!(
            gate.challenge.instance_id == self.instance_id,
            "the challenge was issued for another daemon"
        );
        let Some(daemon) = self.daemon.upgrade() else {
            return Ok(Ended::ShuttingDown);
        };
        // No token and no peer address: the device proof is the only way in,
        // and the carrier, not this stream, knows where the peer is.
        let policy = ConnectionPolicy {
            token: None,
            idle_timeout: Some(self.config.idle_timeout),
            handshake_timeout: HANDSHAKE_TIMEOUT,
            device: Some(gate),
        };
        connection::serve(daemon, stream, policy, None, None, shutdown).await
    }
}

/// Remote control of one daemon: the listener, and the administration of
/// its settings, pairings and devices.
pub struct RemoteControl {
    server: Arc<RemoteServer>,
    runtime: Arc<Runtime>,
    /// One administrative change at a time: read, commit, then act on it.
    admin: tokio::sync::Mutex<()>,
    /// Web Push, which follows the settings and forgets revoked devices.
    /// `None` only in a daemon assembled without it.
    push: Option<Notifier>,
}

/// Manual: the runtime prints far more than a log line wants.
impl std::fmt::Debug for RemoteControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteControl")
            .field("config", self.server.config())
            .field("listening", &self.server.listening())
            .finish()
    }
}

/// The daemon's remote control, absent only in a daemon built without one.
#[derive(Debug, Default)]
pub struct RemoteSlot(OnceLock<Arc<RemoteControl>>);

impl RemoteSlot {
    pub fn get(&self) -> Option<&Arc<RemoteControl>> {
        self.0.get()
    }
}

/// Give `daemon` its remote control in the state the stored settings ask for. A listener or push
/// key that cannot be restored leaves the daemon running; `aether remote status` says why.
pub async fn attach(
    daemon: &Arc<Daemon>,
    config: RemoteConfig,
    shutdown: CancellationToken,
) -> anyhow::Result<Arc<RemoteControl>> {
    anyhow::ensure!(
        daemon.remote.get().is_none(),
        "remote control is already attached to this daemon"
    );
    let push = daemon.push.as_ref().map(|push| push.notifier().clone());
    let control = assemble(daemon, config, push, shutdown)?;
    anyhow::ensure!(
        daemon.remote.0.set(control.clone()).is_ok(),
        "remote control is already attached to this daemon"
    );
    if let Err(error) = control.restore().await {
        tracing::error!(?error, "could not restore remote access as it was");
    }
    Ok(control)
}

/// A remote control over `daemon` that forgets revoked devices through `push`.
fn assemble(
    daemon: &Arc<Daemon>,
    config: RemoteConfig,
    push: Option<Notifier>,
    shutdown: CancellationToken,
) -> anyhow::Result<Arc<RemoteControl>> {
    let host = Arc::new(DeviceConnections::new(daemon, config.clone()));
    let stored = Arc::new(devices::RuntimeDevices::new(daemon.runtime.clone()));
    if let Some(notifier) = &push {
        // From here on nothing is pushed to a device this table revoked.
        notifier.guard_devices(stored.clone());
    }
    let server = RemoteServer::new(config, host, stored, shutdown)?;
    Ok(Arc::new(RemoteControl {
        server: Arc::new(server),
        runtime: daemon.runtime.clone(),
        admin: tokio::sync::Mutex::new(()),
        push,
    }))
}

impl RemoteControl {
    /// Keep the listener until `shutdown`, then close it.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        shutdown.cancelled().await;
        self.server.stop().await;
    }

    /// The settings in force.
    pub fn settings(&self) -> RemoteSettings {
        self.server.settings()
    }

    pub fn server(&self) -> &Arc<RemoteServer> {
        &self.server
    }
}
