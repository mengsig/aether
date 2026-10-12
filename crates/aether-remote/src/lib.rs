//! Remote control: the listener, WebSocket carrier, pairing, web app and
//! Web Push for a phone. The contract is docs/remote.md.

pub mod assets;
pub mod auth;
pub mod connections;
pub mod http;
pub mod listener;
mod lobby;
pub mod pairing;
pub mod push;
#[cfg(test)]
pub(crate) mod testing;
pub mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aether_protocol::remote::{DeviceIdentity, DeviceRole, Pairing, RemoteSettings, RemoteStatus};
use tokio_util::sync::CancellationToken;

pub use auth::{Challenge, DeviceAuthenticator, DeviceGate, DeviceSession, DeviceStore, Sessions};
pub use connections::Connections;
use http::OriginPolicy;
use lobby::{Lobby, Seat};
use pairing::{Enrolment, PairingBook, PairingError};

/// The static `[remote]` settings, read once at daemon start. What changes
/// at runtime is [`RemoteSettings`], which lives in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    pub address: SocketAddr,
    /// Bind outside loopback. Off by default: Aether terminates no TLS.
    pub allow_remote: bool,
    pub max_connections: usize,
    pub idle_timeout: Duration,
    /// Let push payloads carry up to [`push::MAX_PREVIEW_CHARS`] of content.
    pub push_previews: bool,
    /// The VAPID `sub` claim; derived from the first public origin if unset.
    pub push_subject: Option<String>,
}

/// The listener's own deadlines. Not configuration: nothing here is worth
/// an operator's attention, and tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    /// How long a client may take to send a request's headers, and how long
    /// a kept-alive connection may sit between requests.
    pub header_timeout: Duration,
    /// How long a request body may take to arrive.
    pub body_timeout: Duration,
    /// How long a peer owed a response may take none of it.
    pub write_timeout: Duration,
    /// How long a connection nobody authenticated keeps its place once a
    /// newcomer needs one: see [`lobby::Lobby`].
    pub lobby_patience: Duration,
    pub ping_interval: Duration,
    /// Silence, pongs included, after which a peer is taken for gone.
    pub pong_timeout: Duration,
    /// How long a peer owed frames may take none before it is dropped.
    pub stall_timeout: Duration,
    /// How long a close may take, the peer's own hang-up included.
    pub close_timeout: Duration,
    /// How long open connections get to end when the listener closes.
    pub drain_timeout: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            lobby_patience: Duration::from_millis(500),
            ping_interval: Duration::from_secs(ws::PING_INTERVAL_SECS),
            pong_timeout: Duration::from_secs(ws::PONG_TIMEOUT_SECS),
            stall_timeout: Duration::from_secs(ws::STALL_TIMEOUT_SECS),
            close_timeout: Duration::from_secs(2),
            drain_timeout: Duration::from_secs(5),
        }
    }
}

/// How the daemon finished serving one carrier connection. The carrier
/// turns it into the close code docs/remote.md names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The peer left, or the stream ended.
    Closed,
    /// The hello was refused.
    Refused,
    /// The device was revoked or its role changed while connected.
    Revoked,
    /// No accepted hello in time, or idle past the timeout.
    TimedOut,
    /// The daemon is stopping, or the listener was disabled.
    ShuttingDown,
}

/// What the remote server needs from the daemon hosting it.
#[async_trait::async_trait]
pub trait RemoteHost: Send + Sync + std::fmt::Debug + 'static {
    fn instance_id(&self) -> &str;

    /// Serve the newline-delimited protocol on one carrier connection: send
    /// the challenge, check the hello against `gate`, and say how it ended.
    async fn serve(
        &self,
        stream: tokio::io::DuplexStream,
        gate: DeviceGate,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Ended>;
}

/// What every request is answered by: the settings in force and the
/// allowlist they imply. Replaced whole, so a request sees one consistent set.
#[derive(Debug, Default)]
pub(crate) struct Live {
    pub settings: RemoteSettings,
    pub policy: OriginPolicy,
    /// The address actually bound; `None` while the listener is closed.
    pub bound: Option<SocketAddr>,
    /// Why the listener is closed although enabled.
    pub error: Option<String>,
}

/// The open listener: how to stop it, and the task to wait for.
#[derive(Debug)]
struct Running {
    bound: SocketAddr,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

/// The remote edge of one daemon: listener, pairing and live sessions.
#[derive(Debug)]
pub struct RemoteServer {
    config: RemoteConfig,
    tuning: Tuning,
    host: Arc<dyn RemoteHost>,
    devices: Arc<dyn DeviceStore>,
    sessions: Arc<Sessions>,
    authenticator: Arc<dyn DeviceAuthenticator>,
    connections: Arc<Connections>,
    /// HTTP connections being served: none of them has proved anything.
    http_lobby: Arc<Lobby>,
    /// WebSockets whose hello has not been accepted yet.
    greeting_lobby: Arc<Lobby>,
    /// WebSockets whose hello was accepted. Theirs alone, so no number of
    /// peers without a key can take a device's place.
    session_slots: Arc<tokio::sync::Semaphore>,
    pairing: parking_lot::Mutex<PairingBook>,
    live: parking_lot::RwLock<Arc<Live>>,
    /// Held across a whole settings change, so two cannot interleave.
    running: tokio::sync::Mutex<Option<Running>>,
    /// The daemon's own shutdown; the listener's stop is a child of it.
    shutdown: CancellationToken,
}

impl RemoteServer {
    pub fn new(
        config: RemoteConfig,
        host: Arc<dyn RemoteHost>,
        devices: Arc<dyn DeviceStore>,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Self> {
        Self::with_tuning(config, Tuning::default(), host, devices, shutdown)
    }

    pub fn with_tuning(
        config: RemoteConfig,
        tuning: Tuning,
        host: Arc<dyn RemoteHost>,
        devices: Arc<dyn DeviceStore>,
        shutdown: CancellationToken,
    ) -> anyhow::Result<Self> {
        listener::check(&config)?;
        let sessions = Arc::new(Sessions::default());
        let authenticator = Arc::new(auth::StoredDevices {
            devices: devices.clone(),
            sessions: sessions.clone(),
        });
        let ungreeted = config.max_connections.min(listener::MAX_UNGREETED_SOCKETS);
        Ok(Self {
            http_lobby: Lobby::new(config.max_connections, tuning.lobby_patience),
            greeting_lobby: Lobby::new(ungreeted, tuning.lobby_patience),
            session_slots: Arc::new(tokio::sync::Semaphore::new(config.max_connections)),
            config,
            tuning,
            host,
            devices,
            sessions,
            authenticator,
            connections: Arc::new(Connections::default()),
            pairing: parking_lot::Mutex::new(PairingBook::default()),
            live: parking_lot::RwLock::new(Arc::new(Live::default())),
            running: tokio::sync::Mutex::new(None),
            shutdown,
        })
    }

    pub fn config(&self) -> &RemoteConfig {
        &self.config
    }

    pub fn host(&self) -> &Arc<dyn RemoteHost> {
        &self.host
    }

    pub fn devices(&self) -> &Arc<dyn DeviceStore> {
        &self.devices
    }

    /// Live device sessions; revoking a device closes them through this.
    pub fn sessions(&self) -> &Arc<Sessions> {
        &self.sessions
    }

    /// The WebSockets open right now, by origin and device.
    pub fn connections(&self) -> &Arc<Connections> {
        &self.connections
    }

    /// The settings in force: the last ones [`RemoteServer::apply`] took.
    pub fn settings(&self) -> RemoteSettings {
        self.live.read().settings.clone()
    }

    /// The address actually bound; `None` while the listener is closed.
    pub fn listening(&self) -> Option<SocketAddr> {
        self.live.read().bound
    }

    pub(crate) fn live(&self) -> Arc<Live> {
        self.live.read().clone()
    }

    pub(crate) fn tuning(&self) -> &Tuning {
        &self.tuning
    }

    pub(crate) fn pairing(&self) -> &parking_lot::Mutex<PairingBook> {
        &self.pairing
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    pub(crate) fn http_lobby(&self) -> &Arc<Lobby> {
        &self.http_lobby
    }

    /// A place for a WebSocket until its hello is accepted; `None` when
    /// every one is held by a connection that has only just arrived.
    pub(crate) fn greeting_seat(&self) -> Option<Seat> {
        self.greeting_lobby.try_enter().ok()
    }

    /// What a freshly upgraded connection is carried with. `stop` is the
    /// listener's; the connection gets a child it can be ended by alone.
    pub(crate) fn link(&self, origin: String, seat: Seat, stop: &CancellationToken) -> ws::Link {
        let stop = stop.child_token();
        ws::Link {
            host: self.host.clone(),
            authenticator: self.authenticator.clone(),
            registered: self.connections.register(&origin, stop.clone()),
            seat,
            session_slots: self.session_slots.clone(),
            origin,
            stop,
            daemon_stopping: self.shutdown.clone(),
            tuning: self.tuning,
        }
    }

    /// Open, close or re-police the listener to match `settings`. A listener that cannot bind is
    /// not an error: the settings stand and [`RemoteServer::status`] says why it is closed.
    pub async fn apply(self: &Arc<Self>, settings: RemoteSettings) -> anyhow::Result<()> {
        // Checked before anything changes, so bad origins change nothing.
        http::public_origins(&settings.public_origins).map_err(anyhow::Error::msg)?;
        let mut running = self.running.lock().await;
        if !settings.enabled || self.is_stopping() {
            self.close(&mut running).await;
            self.replace_live(settings, None, None)?;
            self.pairing.lock().cancel_live(aether_core::time::now());
            return Ok(());
        }
        let bound = match running.as_ref() {
            Some(open) => open.bound,
            None => match listener::bind(&self.config).await {
                Ok((socket, bound)) => {
                    // The allowlist is in force before the first accept.
                    self.replace_live(settings.clone(), Some(bound), None)?;
                    *running = Some(self.open(socket, bound));
                    bound
                }
                Err(error) => {
                    let why = format!("could not listen on {}: {error:#}", self.config.address);
                    tracing::error!(error = %why, "the remote listener is closed");
                    self.pairing.lock().cancel_live(aether_core::time::now());
                    return self.replace_live(settings, None, Some(why));
                }
            },
        };
        if !settings.pairing {
            self.pairing.lock().cancel_live(aether_core::time::now());
        }
        self.replace_live(settings, Some(bound), None)?;
        let policy = self.live().policy.clone();
        let closed = self
            .connections
            .close_unless(|origin| policy.allows_origin(origin));
        if closed > 0 {
            tracing::info!(closed, "closed device connections from removed origins");
        }
        Ok(())
    }

    fn open(self: &Arc<Self>, socket: tokio::net::TcpListener, bound: SocketAddr) -> Running {
        let stop = self.shutdown.child_token();
        let serving = listener::serve(socket, self.clone(), stop.clone());
        let task = tokio::spawn(async move {
            if let Err(error) = serving.await {
                tracing::error!(error = %format!("{error:#}"), "the remote listener stopped");
            }
        });
        tracing::info!(%bound, "the remote listener is open");
        Running { bound, stop, task }
    }

    /// Stop the listener and wait for its connections to end.
    async fn close(&self, running: &mut Option<Running>) {
        let Some(open) = running.take() else {
            return;
        };
        open.stop.cancel();
        if let Err(error) = open.task.await {
            tracing::error!(%error, "the remote listener task failed");
        }
        tracing::info!(bound = %open.bound, "the remote listener is closed");
    }

    fn replace_live(
        &self,
        settings: RemoteSettings,
        bound: Option<SocketAddr>,
        error: Option<String>,
    ) -> anyhow::Result<()> {
        let policy = match bound {
            None => OriginPolicy::default(),
            Some(bound) => {
                OriginPolicy::new(&settings.public_origins, bound).map_err(anyhow::Error::msg)?
            }
        };
        *self.live.write() = Arc::new(Live {
            settings,
            policy,
            bound,
            error,
        });
        Ok(())
    }

    /// Close the listener for good; the daemon is stopping.
    pub async fn stop(&self) {
        debug_assert!(self.is_stopping(), "stop follows the daemon's shutdown");
        let mut running = self.running.lock().await;
        self.close(&mut running).await;
    }

    /// The reply to `get_remote` and `configure_remote`, at `revision`.
    pub fn status(&self, revision: u64) -> Result<RemoteStatus, auth::AuthError> {
        let live = self.live();
        let devices = self.devices.devices(false)?.len();
        Ok(RemoteStatus {
            revision,
            settings: live.settings.clone(),
            address: self.config.address.to_string(),
            listening: live.bound.map(|bound| bound.to_string()),
            allow_remote: self.config.allow_remote,
            devices: u32::try_from(devices).unwrap_or(u32::MAX),
            connections: self.connections.authenticated(),
            push_previews: self.config.push_previews,
            error: live.error.clone(),
        })
    }

    /// Open a pairing for `role`. The reply alone carries the code.
    pub fn start_pairing(&self, role: DeviceRole) -> Result<Pairing, PairingError> {
        let live = self.live();
        if !live.settings.enabled {
            return Err(PairingError::Disabled(
                "remote access is disabled; run `aether remote enable` first",
            ));
        }
        if !live.settings.pairing {
            return Err(PairingError::Disabled(
                "pairing is disabled; run `aether remote enable` without `--no-pairing`",
            ));
        }
        let origin = live.policy.primary().ok_or(PairingError::Disabled(
            "the remote listener is not open; see `aether remote status`",
        ))?;
        let now = aether_core::time::now();
        self.pairing.lock().start(role, origin, now)
    }

    pub fn get_pairing(&self, pairing: &str) -> Result<Pairing, PairingError> {
        self.pairing.lock().get(pairing, aether_core::time::now())
    }

    /// The host's decision. `enrol` commits the device and runs under the
    /// pairing lock, so a claim can be confirmed exactly once.
    pub fn confirm_pairing(
        &self,
        pairing: &str,
        accept: bool,
        enrol: impl FnOnce(Enrolment<'_>) -> Result<DeviceIdentity, String>,
    ) -> Result<Pairing, PairingError> {
        let now = aether_core::time::now();
        self.pairing.lock().confirm(pairing, accept, now, enrol)
    }

    pub fn cancel_pairing(&self, pairing: &str) -> Result<Pairing, PairingError> {
        self.pairing
            .lock()
            .cancel(pairing, aether_core::time::now())
    }
}
