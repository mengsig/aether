//! Async client for the Aether daemon protocol.
//!
//! One connection multiplexes any number of concurrent requests: each frame
//! carries an id, a background reader routes replies back to the waiting
//! caller and pushes subscription frames to their [`Subscription`]. The TUI,
//! the CLI and orchestration applications all sit on this crate.

pub mod endpoint;
pub mod images;
pub mod mcp;
pub mod serve;
pub mod sink;
pub mod start;
pub mod transport;

use aether_protocol::{
    ClientFrame, Command, ErrorCode, ErrorReply, Hello, Reply, ServerFrame, Welcome,
};
use futures::StreamExt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc, oneshot};

mod agents;
mod delivery;
mod outbox;
mod resolution;
pub use delivery::key_command;
pub use outbox::{Unreadable, Unresolved};
pub use resolution::{Settlement, Undecided};
mod memory_feedback;
mod negotiation;
mod operations;
mod records;
mod ssh;
mod subscription;
mod workspace_review;
pub use subscription::{Subscription, SubscriptionFrame};

pub use endpoint::{Endpoint, SshRemote};
pub use sink::{CommandSink, DaemonSink, DaemonUnavailable, ReconnectingSink};
pub use start::{StartingDaemon, ensure_daemon, watch_startup};
pub use transport::{MemoryPeer, Transport};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("daemon refused: {0}")]
    Protocol(#[from] ErrorReply),
    #[error("codec: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("SSH connection failed: {0}")]
    Ssh(String),
    #[error("delivery unconfirmed: {0}")]
    DeliveryUnconfirmed(String),
    #[error("disconnected from daemon")]
    Disconnected,
    #[error("incompatible daemon: {0}")]
    Incompatible(String),
    #[error("the daemon refused this token: {0}")]
    Unauthorized(String),
    #[error("unexpected reply: expected {expected}, got {got}")]
    Unexpected { expected: &'static str, got: String },
    #[error("malformed payload: {0}")]
    Malformed(String),
    #[error("invalid client usage: {0}")]
    InvalidUsage(String),
    #[error("the daemon did not answer `{command}` within {}s", timeout.as_secs())]
    TimedOut { command: String, timeout: Duration },
    /// A daemon this client started died before it listened. Carries what
    /// that daemon said, which is the only account of it there is.
    #[error("the daemon failed to start: {0}")]
    DaemonStartup(String),
}

impl ClientError {
    /// True when retrying the connection is the reasonable response.
    pub fn is_connection_failure(&self) -> bool {
        match self {
            Self::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            ),
            Self::Disconnected => true,
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

type PendingReply = oneshot::Sender<std::result::Result<PendingResponse, ErrorReply>>;

struct PendingResponse {
    reply: Reply,
    subscription: Option<UnclaimedSubscription>,
}

struct UnclaimedSubscription {
    id: u64,
    shared: Arc<Shared>,
    sink: mpsc::WeakSender<ClientFrame>,
    runtime: tokio::runtime::Handle,
    claimed: bool,
}

impl Drop for UnclaimedSubscription {
    fn drop(&mut self) {
        if !self.claimed {
            unsubscribe_abandoned(&self.shared, &self.sink, self.id, &self.runtime);
        }
    }
}

struct Shared {
    next_id: AtomicU64,
    closed: AtomicBool,
    pending: Mutex<HashMap<u64, PendingReply>>,
    subscriptions: Mutex<HashMap<u64, mpsc::Sender<SubscriptionFrame>>>,
    /// Receivers created by the reader when a `Subscribed` reply arrives,
    /// claimed by the `subscribe` caller. Created reader-side so no frame
    /// can slip through between the reply and the registration.
    sub_receivers: Mutex<HashMap<u64, mpsc::Receiver<SubscriptionFrame>>>,
    /// The command name of a request that gave up waiting, kept only so a
    /// reply that still arrives can be logged by name instead of vanishing
    /// into "reply for unknown request id": the caller already moved on and
    /// there is nothing left to deliver it to, but it must not be silent.
    late_after_timeout: Mutex<HashMap<u64, String>>,
    /// The last notice the daemon sent on request id 0 - the id reserved for
    /// something the daemon says on its own account, rather than an answer to
    /// a request. It says why a connection is about to close, and without
    /// somewhere to keep it the caller saw only `Disconnected`.
    notice: std::sync::Mutex<Option<ErrorReply>>,
}

/// A handle to the daemon. Cheap to clone; all clones share one connection.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    sink: transport::FrameSink,
    welcome: Option<Welcome>,
    /// `None` waits for ever: the right bound for a request that blocks on a
    /// person by design, like the MCP bridge's engine tool calls, where the
    /// alternative is failing a slow-to-answer approval that the operator did
    /// grant.
    request_timeout: Option<Duration>,
    carrier: Option<Arc<ssh::Carrier>>,
    outbox: Option<Arc<outbox::Outbox>>,
    recovered: Vec<(Command, std::result::Result<Reply, ErrorReply>)>,
}

/// Buffered event/delta frames per subscription, excluding its terminal lag signal.
pub const MAX_QUEUED_SUBSCRIPTION_FRAMES: usize = 1024;

/// Long enough that no healthy command hits it, short enough that a caller
/// that will never be answered finds out.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("welcome", &self.welcome)
            .finish()
    }
}

impl Client {
    /// Connect to the daemon socket at the newest protocol both sides speak.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Client> {
        let path = path.as_ref();
        negotiation::newest_common(|version| async move {
            let stream = UnixStream::connect(path).await?;
            Self::open_at(Transport::Unix(stream), None, version).await
        })
        .await
    }

    /// Connect to a daemon's TCP listener, which authorizes on the token
    /// alone, at the newest protocol both sides speak. The connection is kept
    /// alive under the daemon's idle timeout.
    pub async fn connect_tcp(addr: SocketAddr, token: impl Into<String>) -> Result<Client> {
        let token = token.into();
        negotiation::newest_common(|version| {
            let token = token.clone();
            async move {
                let stream = tokio::net::TcpStream::connect(addr).await?;
                let _ = stream.set_nodelay(true);
                Self::open_at(Transport::Tcp(stream), Some(token), version).await
            }
        })
        .await
    }

    /// Wrap a transport and perform the handshake at this build's protocol.
    /// A transport is one connection, so there is no older version to retry.
    pub async fn open(transport: Transport) -> Result<Client> {
        Self::open_at(transport, None, aether_protocol::PROTOCOL_VERSION).await
    }

    async fn open_at(transport: Transport, token: Option<String>, version: u32) -> Result<Client> {
        let mut client = Self::from_transport(transport);
        let welcome = client.handshake_version(token, version).await?;
        client.keep_alive(welcome.idle_timeout_secs);
        client.welcome = Some(welcome);
        Ok(client)
    }

    /// Hold a quiet connection open under the daemon's idle timeout. The task
    /// holds only a weak sink, so it ends with the last `Client` clone rather
    /// than keeping the connection alive on its own.
    fn keep_alive(&self, idle_timeout_secs: Option<u64>) {
        let Some(secs) = idle_timeout_secs.filter(|secs| *secs >= 3) else {
            return;
        };
        let every = Duration::from_secs(secs / 3);
        let sink = self.sink.downgrade();
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(sink) = sink.upgrade() else { break };
                if shared.closed.load(Ordering::SeqCst) {
                    break;
                }
                let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
                // Registered like any other request even though nothing waits
                // on the answer: the reader would otherwise log every pong for
                // the life of the connection as a reply to nothing. A daemon
                // that stops answering grows this by one per interval, bounded
                // by the idle timeout that then closes the connection.
                let (tx, _drop_the_pong) = oneshot::channel();
                {
                    let mut pending = shared.pending.lock().await;
                    if shared.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    pending.insert(id, tx);
                }
                let frame = ClientFrame {
                    id,
                    command: Command::Ping,
                };
                if sink.send(frame).await.is_err() {
                    shared.pending.lock().await.remove(&id);
                    break;
                }
            }
        });
    }

    /// Wrap a transport without handshaking. Callers must send `Hello`
    /// themselves before any other command.
    pub fn from_transport(transport: Transport) -> Client {
        let (stream, sink) = transport.split();
        let shared = Arc::new(Shared {
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            pending: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            sub_receivers: Mutex::new(HashMap::new()),
            late_after_timeout: Mutex::new(HashMap::new()),
            notice: std::sync::Mutex::new(None),
        });
        tokio::spawn(reader_loop(stream, Arc::clone(&shared), sink.downgrade()));
        Client {
            shared,
            sink,
            welcome: None,
            request_timeout: Some(DEFAULT_REQUEST_TIMEOUT),
            carrier: None,
            outbox: None,
            recovered: Vec::new(),
        }
    }

    /// The daemon's handshake reply, when this client performed one.
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcome.as_ref()
    }

    pub fn recovered_commands(&self) -> &[(Command, std::result::Result<Reply, ErrorReply>)] {
        &self.recovered
    }

    pub fn is_connected(&self) -> bool {
        !self.shared.closed.load(Ordering::SeqCst)
    }

    /// A subscription replay ended because retained history is no longer available.
    pub fn history_expired(&self) -> bool {
        self.disconnect_reason()
            .is_some_and(|reason| reason.code == ErrorCode::NotFound)
    }

    /// Why the daemon ended this connection, when it said. A tokened
    /// transport that drops a quiet client explains itself first, and a
    /// caller that only reports `Disconnected` throws that away.
    pub fn disconnect_reason(&self) -> Option<ErrorReply> {
        self.shared
            .notice
            .lock()
            .ok()
            .and_then(|held| held.clone())
            .or_else(|| {
                self.carrier.as_ref().map(|carrier| ErrorReply {
                    code: ErrorCode::Unavailable,
                    message: carrier.diagnostics(),
                })
            })
    }

    async fn handshake_version(&self, token: Option<String>, version: u32) -> Result<Welcome> {
        let mut hello = Hello::new("aether", env!("CARGO_PKG_VERSION"));
        hello.protocol_version = version;
        let hello = match token {
            Some(token) => hello.with_token(token),
            None => hello,
        };
        debug_assert!(
            (aether_protocol::compat::MIN_PROTOCOL_VERSION..=aether_protocol::PROTOCOL_VERSION)
                .contains(&version),
            "a hello only asks for a version this build speaks, got {version}"
        );
        match self.request(Command::Hello(hello)).await {
            Ok(Reply::Welcome(w)) if w.protocol_version == version => Ok(w),
            Ok(Reply::Welcome(w)) => Err(ClientError::Malformed(format!(
                "asked for protocol {version} and the daemon answered with {}",
                w.protocol_version
            ))),
            Ok(other) => Err(unexpected("welcome", &other)),
            Err(ClientError::Protocol(e)) if e.code == ErrorCode::Incompatible => {
                Err(ClientError::Incompatible(version_skew(&e.message)))
            }
            Err(ClientError::Protocol(e)) if e.code == ErrorCode::Unauthorized => {
                Err(ClientError::Unauthorized(e.message))
            }
            Err(e) => Err(e),
        }
    }

    /// Send a command and wait for its reply, or give up.
    ///
    /// Subscribe commands must use [`Self::subscribe`] to return a stream.
    /// Replies are correlated strictly by id, so anything that loses one -
    /// a daemon from an older build that cannot decode the frame, a reply
    /// that never arrives - would otherwise leave the caller waiting for
    /// ever. `aether memory forget` hanging against a daemon left running
    /// from a previous build is the common shape of that.
    pub async fn request(&self, mut command: Command) -> Result<Reply> {
        if matches!(command, Command::Subscribe { .. }) {
            return Err(ClientError::InvalidUsage(
                "use Client::subscribe to receive subscription frames".into(),
            ));
        }
        self.check_introduced(&command)?;
        self.check_images_supported(&command)?;
        let keyed = self.prepare_remote_command(&mut command)?;
        let mutating = self.carrier.is_some() && delivery::is_mutating(&command);
        let name = command.name();
        let recorded = match (&self.outbox, keyed) {
            (Some(outbox), true) => Some(outbox.record(&command, self.horizon()).await?),
            _ => None,
        };
        let result = self.request_inner(command).await;
        if matches!(result, Ok(_) | Err(ClientError::Protocol(_)))
            && let (Some(outbox), Some(claim)) = (&self.outbox, &recorded)
        {
            outbox.acknowledge(claim).await.map_err(|error| {
                ClientError::DeliveryUnconfirmed(format!(
                    "{name} answered but its outbox entry could not be retired: {error}"
                ))
            })?;
        }
        match result {
            Ok(response) => Ok(response.reply),
            Err(error @ (ClientError::Disconnected | ClientError::TimedOut { .. }))
                if self.carrier.is_some() =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let diagnostics = self
                    .disconnect_reason()
                    .map(|reason| reason.message)
                    .unwrap_or_else(|| "SSH has reported no diagnostic".into());
                let message =
                    format!("{name}: {error}; {diagnostics}; refresh host state before retrying");
                if mutating {
                    Err(ClientError::DeliveryUnconfirmed(message))
                } else {
                    Err(ClientError::Ssh(message))
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn request_inner(&self, command: Command) -> Result<PendingResponse> {
        if !self.is_connected() {
            return Err(ClientError::Disconnected);
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let name = command.name();
        let (tx, mut rx) = oneshot::channel();
        {
            let mut pending = self.shared.pending.lock().await;
            if self.shared.closed.load(Ordering::SeqCst) {
                return Err(ClientError::Disconnected);
            }
            pending.insert(id, tx);
        }
        tracing::debug!(id, "sending request");
        if self.sink.send(ClientFrame { id, command }).await.is_err() {
            self.shared.pending.lock().await.remove(&id);
            return Err(ClientError::Disconnected);
        }
        let outcome = match self.request_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, &mut rx).await {
                Ok(inner) => inner,
                Err(_) => {
                    rx.close();
                    // A reply queued at the deadline still belongs to this
                    // caller; closing prevents any later delivery succeeding.
                    if let Ok(result) = rx.try_recv() {
                        return result.map_err(ClientError::Protocol);
                    }
                    let mut pending = self.shared.pending.lock().await;
                    if pending.remove(&id).is_some() {
                        // Publish the name before the reader can claim this
                        // id; it may already own the reply being abandoned.
                        self.shared
                            .late_after_timeout
                            .lock()
                            .await
                            .insert(id, name.clone());
                    }
                    drop(pending);
                    return Err(ClientError::TimedOut {
                        command: name,
                        timeout,
                    });
                }
            },
            None => rx.await,
        };
        match outcome {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(err)) => Err(ClientError::Protocol(err)),
            Err(_) => Err(ClientError::Disconnected),
        }
    }

    /// How long [`Self::request`] waits for a reply. Generous by default:
    /// this is the "something is wrong" bound, not a latency target - an MCP
    /// refresh or a repository sync legitimately takes seconds.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Wait for ever instead of failing at [`DEFAULT_REQUEST_TIMEOUT`]. For a
    /// request that blocks on a person by design - the MCP bridge's engine
    /// tool calls wait on the operator's approval verdict - failing at a
    /// fixed bound would turn a late but real answer into a dropped one.
    pub fn without_request_timeout(mut self) -> Self {
        self.request_timeout = None;
        self
    }
}

fn expect_ok(reply: Reply) -> Result<()> {
    match reply {
        Reply::Ok => Ok(()),
        other => Err(unexpected("ok", &other)),
    }
}

/// A reply's wire tag, for a message about one that was not expected.
fn reply_name(reply: &Reply) -> String {
    serde_json::to_value(reply)
        .ok()
        .and_then(|v| v.get("reply").and_then(|r| r.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

fn unexpected(expected: &'static str, got: &Reply) -> ClientError {
    ClientError::Unexpected {
        expected,
        got: reply_name(got),
    }
}

/// A hello refused for its protocol version, with both sides of the skew.
///
/// The daemon names the window it serves but not which side is stale, and the
/// usual cause is a daemon left running from the build before an update.
/// The refusing daemon may be older than this crate, so its half
/// of the message can never be improved from here; this half is added by the
/// client that was refused. What to do about it belongs to whoever is driving
/// this client - `aether`, an orchestration application - not to the library.
fn version_skew(daemon_said: &str) -> String {
    format!(
        "{daemon_said}. This build speaks protocol {} to {}, so the daemon answering that \
         socket is from a different build",
        aether_protocol::compat::MIN_PROTOCOL_VERSION,
        aether_protocol::PROTOCOL_VERSION
    )
}

/// Routes incoming frames to waiting requests and subscriptions until the
/// connection ends, then fails everything still pending.
async fn reader_loop(
    mut stream: transport::FrameStream,
    shared: Arc<Shared>,
    sink: mpsc::WeakSender<ClientFrame>,
) {
    while let Some(frame) = stream.next().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(e) => {
                tracing::debug!("connection error: {e}");
                break;
            }
        };
        match frame {
            ServerFrame::Reply { id, result } => {
                tracing::debug!(id, ok = result.is_ok(), "reply received");
                let sub = match &result {
                    Ok(Reply::Subscribed { id, .. }) => Some(*id),
                    _ => None,
                };
                let waiter = shared.pending.lock().await.remove(&id);
                match waiter {
                    Some(waiter) => {
                        let subscription = if let Some(sub) = sub {
                            if !waiter.is_closed() {
                                let (tx, rx) = mpsc::channel(MAX_QUEUED_SUBSCRIPTION_FRAMES + 1);
                                shared.subscriptions.lock().await.insert(sub, tx);
                                shared.sub_receivers.lock().await.insert(sub, rx);
                            }
                            Some(UnclaimedSubscription {
                                id: sub,
                                shared: Arc::clone(&shared),
                                sink: sink.clone(),
                                runtime: tokio::runtime::Handle::current(),
                                claimed: false,
                            })
                        } else {
                            None
                        };
                        // Cleanup stays with the queued reply until its caller
                        // claims the channel, including cancellation after send.
                        let response = result.map(|reply| PendingResponse {
                            reply,
                            subscription,
                        });
                        if waiter.send(response).is_err() {
                            tracing::debug!(id, "reply receiver was abandoned");
                        }
                    }
                    // Request ids start at 1, so 0 is the daemon speaking on
                    // its own account: kept, so the disconnect that usually
                    // follows can say why.
                    None if id == 0 => match result {
                        Err(notice) => {
                            tracing::info!(code = ?notice.code, "{}", notice.message);
                            if let Ok(mut held) = shared.notice.lock() {
                                *held = Some(notice);
                            }
                        }
                        Ok(reply) => tracing::debug!(?reply, "a notice with no error in it"),
                    },
                    None => {
                        if let Some(sub) = sub {
                            unsubscribe_abandoned(
                                &shared,
                                &sink,
                                sub,
                                &tokio::runtime::Handle::current(),
                            );
                        }
                        match shared.late_after_timeout.lock().await.remove(&id) {
                            Some(command) => tracing::warn!(
                                id,
                                %command,
                                "reply arrived after the request timed out; the caller already gave up"
                            ),
                            None => tracing::debug!("reply for unknown request id {id}"),
                        }
                    }
                }
            }
            ServerFrame::Event { sub, event } => {
                deliver(
                    &shared,
                    &sink,
                    sub,
                    SubscriptionFrame::Event(Box::new(event)),
                )
                .await;
            }
            ServerFrame::Delta {
                sub,
                agent,
                invocation,
                delta,
            } => {
                deliver(
                    &shared,
                    &sink,
                    sub,
                    SubscriptionFrame::Delta {
                        agent,
                        invocation,
                        delta,
                    },
                )
                .await;
            }
            // Only the browser carrier challenges, and this client holds no
            // device key: it has reached something it cannot greet.
            ServerFrame::Challenge { .. } => {
                tracing::warn!("the daemon asked for a device proof this client cannot give");
                break;
            }
        }
    }
    shared.closed.store(true, Ordering::SeqCst);
    // An uncorrelatable request is refused at connection scope. Preserve its
    // validation error for every caller; other closures keep Disconnected.
    let reason = shared
        .notice
        .lock()
        .ok()
        .and_then(|held| held.clone())
        .filter(|reason| reason.code == ErrorCode::InvalidArgument);
    for (_, waiter) in shared.pending.lock().await.drain() {
        if let Some(reason) = &reason {
            let _ = waiter.send(Err(reason.clone()));
        }
    }
    // Unclaimed receivers stay: their senders are gone, so a caller racing
    // this close gets a subscription that ends rather than a missing one.
    shared.subscriptions.lock().await.clear();
    shared.late_after_timeout.lock().await.clear();
}

fn unsubscribe_abandoned(
    shared: &Arc<Shared>,
    sink: &mpsc::WeakSender<ClientFrame>,
    sub: u64,
    runtime: &tokio::runtime::Handle,
) {
    let shared = Arc::clone(shared);
    let sink = sink.clone();
    // Waiting here would block the reader that must route the cleanup reply.
    runtime.spawn(async move {
        shared.subscriptions.lock().await.remove(&sub);
        shared.sub_receivers.lock().await.remove(&sub);
        unsubscribe_remote(shared, sink, sub).await;
    });
}

async fn unsubscribe_remote(shared: Arc<Shared>, sink: mpsc::WeakSender<ClientFrame>, sub: u64) {
    let Some(sink) = sink.upgrade() else { return };
    let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = oneshot::channel();
    {
        let mut pending = shared.pending.lock().await;
        if shared.closed.load(Ordering::SeqCst) {
            return;
        }
        pending.insert(id, tx);
    }
    let cleanup = async {
        sink.send(ClientFrame {
            id,
            command: Command::Unsubscribe { id: sub },
        })
        .await
        .map_err(|_| ClientError::Disconnected)?;
        match rx.await {
            Ok(Ok(response)) => expect_ok(response.reply),
            Ok(Err(error)) => Err(ClientError::Protocol(error)),
            Err(_) => Err(ClientError::Disconnected),
        }
    };
    match tokio::time::timeout(DEFAULT_REQUEST_TIMEOUT, cleanup).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(sub, %error, "failed to release subscription");
        }
        Err(_) => {
            tracing::warn!(sub, "timed out releasing subscription");
        }
    }
    shared.pending.lock().await.remove(&id);
}

async fn deliver(
    shared: &Arc<Shared>,
    sink: &mpsc::WeakSender<ClientFrame>,
    sub: u64,
    frame: SubscriptionFrame,
) {
    let sender = shared.subscriptions.lock().await.get(&sub).cloned();
    let Some(tx) = sender else {
        tracing::debug!("frame for unknown subscription {sub} dropped");
        return;
    };
    // Only the reader produces frames. Reserve the final slot for an
    // observable lag signal rather than blocking replies behind a slow sub.
    let overflow = tx.capacity() <= 1;
    let frame = if overflow {
        SubscriptionFrame::Lagged {
            capacity: MAX_QUEUED_SUBSCRIPTION_FRAMES,
        }
    } else {
        frame
    };
    if tx.try_send(frame).is_err() || overflow {
        shared.subscriptions.lock().await.remove(&sub);
        let shared = Arc::clone(shared);
        let sink = sink.clone();
        tokio::spawn(unsubscribe_remote(shared, sink, sub));
    }
}

#[cfg(test)]
mod tests;
