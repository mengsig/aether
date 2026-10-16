//! One client connection: handshake, request dispatch, subscriptions.

use crate::Daemon;
use crate::authz::{self, AuthContext};
use crate::listen::ConnectionPolicy;
use aether_core::event::Actor;
use aether_protocol::{
    ClientFrame, Command, ErrorCode, ErrorReply, Reply, ServerFrame, Welcome, compat, features,
};
use aether_remote::Ended;
use aether_runtime::RuntimeError;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::sync::CancellationToken;

use futures::StreamExt as _;

mod dispatch;
mod host_services;
mod messages;
mod providers;
mod push;
mod remote_admin;
mod resolution;
mod subscription;
mod writer;
#[cfg(test)]
use dispatch::dispatch;
use dispatch::dispatch_version;
use subscription::{SubscriptionControl, spawn_subscription};

const MAX_SUBSCRIPTIONS: usize = 16;

const MAX_LINE: usize = 64 * 1024 * 1024;
const WRITER_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

fn map_err(e: RuntimeError) -> ErrorReply {
    match e {
        RuntimeError::NotFound(what) => {
            ErrorReply::new(ErrorCode::NotFound, format!("{what} not found"))
        }
        RuntimeError::Invalid(msg)
        | RuntimeError::Memory(aether_memory::MemoryError::Invalid(msg)) => {
            ErrorReply::new(ErrorCode::InvalidArgument, msg)
        }
        RuntimeError::Memory(aether_memory::MemoryError::NotFound(id)) => ErrorReply::not_found(id),
        RuntimeError::Transition(t) => ErrorReply::new(ErrorCode::InvalidTransition, t.to_string()),
        RuntimeError::Models(m) => ErrorReply::new(ErrorCode::Unavailable, m.to_string()),
        RuntimeError::Model(m) => ErrorReply::new(ErrorCode::Unavailable, m.to_string()),
        RuntimeError::Store(e) => store_err(e),
        other => ErrorReply::internal(other),
    }
}

fn store_err(e: aether_store::StoreError) -> ErrorReply {
    match e {
        aether_store::StoreError::HistoryExpired { .. } => {
            ErrorReply::new(ErrorCode::NotFound, e.to_string())
        }
        other => ErrorReply::internal(other),
    }
}

/// Longest line accepted before the hello has been authorized. A hello is a
/// few hundred bytes; the full cap would let every unauthenticated peer ask
/// the daemon for 64 MB of buffer before the codec refused the line.
const PRE_AUTH_MAX_LINE: usize = 64 * 1024;

/// The slots a TCP connection passes through: a handshake slot from the
/// accept until it greets, then an authenticated slot for the rest of its
/// life. A peer that never authorizes can therefore only occupy a handshake
/// slot, which the handshake deadline bounds - it cannot sit on one of the
/// connections the operator's own client is waiting for.
pub(crate) struct Gate {
    handshake: Option<tokio::sync::OwnedSemaphorePermit>,
    authenticated: Arc<tokio::sync::Semaphore>,
    admitted: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl Gate {
    pub(crate) fn new(
        handshake: tokio::sync::OwnedSemaphorePermit,
        authenticated: Arc<tokio::sync::Semaphore>,
    ) -> Gate {
        Gate {
            handshake: Some(handshake),
            authenticated,
            admitted: None,
        }
    }

    /// Trade the handshake slot for an authenticated one. Waits when the
    /// authenticated cap is full: back-pressure, not a refusal - but bounded
    /// by `deadline` and by `shutdown`, so a peer waiting here cannot hold
    /// the handshake slot past either. Releases the handshake slot either
    /// way: a peer that does not get admitted has authorized nothing and is
    /// owed no more of it. Returns whether it was admitted.
    async fn admit(&mut self, shutdown: &CancellationToken, deadline: Duration) -> bool {
        let authenticated = Arc::clone(&self.authenticated);
        let admitted = tokio::select! {
            permit = authenticated.acquire_owned() => permit.ok(),
            _ = shutdown.cancelled() => None,
            _ = tokio::time::sleep(deadline) => None,
        };
        self.handshake = None;
        let ok = admitted.is_some();
        self.admitted = admitted;
        ok
    }
}

pub(crate) async fn serve<S>(
    daemon: Arc<Daemon>,
    stream: S,
    policy: ConnectionPolicy,
    peer: Option<std::net::SocketAddr>,
    mut gate: Option<Gate>,
    shutdown: CancellationToken,
) -> anyhow::Result<Ended>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    // Read and write are framed separately so the read side's line cap can be
    // raised once the peer is authorized, without disturbing the writer.
    let (read, write) = tokio::io::split(stream);
    let mut lines = FramedRead::new(read, LinesCodec::new_with_max_length(PRE_AUTH_MAX_LINE));
    let (out_tx, out_rx) = mpsc::channel::<ServerFrame>(1024);
    let (replay_failure_tx, mut replay_failure_rx) = mpsc::channel::<ErrorReply>(1);

    // The version this connection speaks. The writer starts before the
    // handshake picks it, so it is shared; hello stores it before the
    // welcome is queued, and nothing before the welcome needs translating.
    let version = Arc::new(AtomicU32::new(aether_protocol::PROTOCOL_VERSION));

    let (auth_tx, auth_rx) = tokio::sync::watch::channel::<Option<AuthContext>>(None);
    let writer = writer::run(write, out_rx, version.clone(), auth_rx);
    tokio::pin!(writer);

    let connection_cancel = shutdown.child_token();
    let _cancel_on_drop = connection_cancel.clone().drop_guard();
    let mut subscriptions: HashMap<u64, (CancellationToken, tokio::task::JoinHandle<()>)> =
        HashMap::new();
    let mut next_sub: u64 = 1;
    // Set when the hello is accepted: who this connection is from then on.
    let mut standing: Option<(AuthContext, Actor)> = None;
    let mut ended = Ended::Closed;
    let connection = ulid::Ulid::generate().to_string();

    let writer_finished = {
        let reading = async {
            // A carrier connection is challenged before it may greet.
            if let Some(challenge) = challenge_frame(&daemon, &policy)
                && !send_frame(&out_tx, challenge, &connection_cancel).await
            {
                return;
            }
            loop {
                // An un-greeted peer is on the short handshake deadline, whatever the
                // idle timeout is: until it presents a token it is holding a slot on
                // no authority at all.
                let limit = if standing.is_some() {
                    policy.idle_timeout
                } else {
                    Some(policy.handshake_timeout)
                };
                let next = async {
                    match limit {
                        Some(limit) => tokio::time::timeout(limit, lines.next()).await,
                        None => Ok(lines.next().await),
                    }
                };
                let line = tokio::select! {
                    biased;
                    _ = revoked(standing.as_ref()) => {
                        ended = Ended::Revoked;
                        break
                    },
                    _ = connection_cancel.cancelled() => {
                        if shutdown.is_cancelled() {
                            ended = Ended::ShuttingDown;
                        }
                        break
                    },
                    Some(error) = replay_failure_rx.recv() => {
                        let _ = send_frame(&out_tx, ServerFrame::Reply { id: 0, result: Err(error) }, &connection_cancel).await;
                        break;
                    },
                    l = next => match l {
                        Ok(Some(Ok(l))) => l,
                        Ok(Some(Err(e))) => { tracing::debug!(error = %e, "read failed"); break }
                        Ok(None) => break,
                        Err(_) => {
                            ended = Ended::TimedOut;
                            // A peer that never greeted is owed no explanation, and
                            // giving one would be answering a request it never made.
                            if standing.is_none() {
                                tracing::debug!("closed a connection that did not greet in time");
                                break;
                            }
                            // Say why, rather than dropping a silent client's socket
                            // with no explanation on it.
                            let _ = send_frame(&out_tx, ServerFrame::Reply { id: 0, result: Err(ErrorReply::new(
                                ErrorCode::Unavailable,
                                "closed after the idle timeout; send `ping` to hold a quiet connection open",
                            ))}, &connection_cancel).await;
                            break
                        }
                    },
                };
                let (id, decoded) = match parse_frame(&line) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let _ = send_frame(
                            &out_tx,
                            ServerFrame::Reply {
                                id: 0,
                                result: Err(error),
                            },
                            &connection_cancel,
                        )
                        .await;
                        break;
                    }
                };
                let frame = match decoded {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = send_frame(
                            &out_tx,
                            ServerFrame::Reply {
                                id,
                                result: Err(ErrorReply::invalid(format!("bad frame: {e}"))),
                            },
                            &connection_cancel,
                        )
                        .await;
                        // Before the hello, a line that does not decode is not a
                        // client making a mistake: it is a peer holding a slot and
                        // refreshing the deadline without ever authorizing.
                        if standing.is_none() {
                            break;
                        }
                        continue;
                    }
                };
                let Some((auth, operator)) = standing.as_ref() else {
                    let result = match &frame.command {
                        Command::Hello(hello) => match compat::negotiate(hello) {
                            Err(refusal) => Err(refusal),
                            // The version is stored before the token is checked, so a
                            // refusal is written in the shape the client asked for.
                            // Deliberate, and it means an unauthorized peer can learn
                            // the daemon's protocol window: a version oracle, not a
                            // token one, and the alternative is a refusal a client
                            // cannot decode.
                            Ok(speaking) => {
                                version.store(speaking, Ordering::SeqCst);
                                match policy.admit(hello) {
                                    Err(refusal) => {
                                        tokio::time::sleep(policy.refusal_delay()).await;
                                        Err(refusal)
                                    }
                                    Ok(context) => {
                                        // Authorized: the full frame size, and the
                                        // slot this connection holds from here on.
                                        // `LinesCodec` fixes its cap at
                                        // construction, so the reader is rebuilt -
                                        // carrying whatever the peer already
                                        // pipelined behind the hello.
                                        let pipelined = std::mem::take(lines.read_buffer_mut());
                                        lines = FramedRead::new(
                                            lines.into_inner(),
                                            LinesCodec::new_with_max_length(MAX_LINE),
                                        );
                                        lines.read_buffer_mut().extend_from_slice(&pipelined);
                                        let admitted = match gate.as_mut() {
                                            Some(gate) => {
                                                gate.admit(&shutdown, policy.handshake_timeout)
                                                    .await
                                            }
                                            None => true,
                                        };
                                        if !admitted {
                                            Err(ErrorReply::new(
                                                ErrorCode::Unavailable,
                                                "no authenticated connection slot became available in \
                                         time",
                                            ))
                                        } else {
                                            let device = context.device().cloned();
                                            let actor = context.actor(peer, connection.clone());
                                            auth_tx.send_replace(Some(context.clone()));
                                            standing = Some((context, actor));
                                            Ok(Reply::Welcome(Welcome {
                                                protocol_version: speaking,
                                                daemon_version: daemon.version.clone(),
                                                instance_id: daemon.instance_id.clone(),
                                                schema_version: aether_store::SCHEMA_VERSION,
                                                pid: std::process::id(),
                                                features: vec![
                                                    features::DELTAS.into(),
                                                    features::ARTIFACT_BYTES.into(),
                                                    features::PUBLISH_ARTIFACT.into(),
                                                    features::HOST_CONTEXT.into(),
                                                    features::OPERATOR_MEMORY_ROLES.into(),
                                                    features::MEMORY_RECORDED_BY.into(),
                                                    features::REPOSITORY_ROOT.into(),
                                                    features::REPOSITORY_ALIASES.into(),
                                                    features::MEMORY_PROPOSALS.into(),
                                                    features::MEMORY_CONTEXT.into(),
                                                    features::MEMORY_FEEDBACK.into(),
                                                    features::STORAGE.into(),
                                                    features::PRUNE_BUILD_OUTPUT.into(),
                                                    features::BUILD_CACHE.into(),
                                                ],
                                                daemon_protocol_version: Some(
                                                    aether_protocol::PROTOCOL_VERSION,
                                                ),
                                                min_protocol_version: Some(
                                                    compat::MIN_PROTOCOL_VERSION,
                                                ),
                                                idle_timeout_secs: policy
                                                    .idle_timeout
                                                    .map(|limit| limit.as_secs()),
                                                device,
                                                submission_horizon: resolution::greeting_horizon(
                                                    &daemon, speaking,
                                                ),
                                            }))
                                        }
                                    }
                                }
                            }
                        },
                        _ => Err(ErrorReply::new(
                            ErrorCode::Incompatible,
                            "first frame must be hello",
                        )),
                    };
                    let ok = result.is_ok();
                    if matches!(&result, Err(refusal) if refusal.code == ErrorCode::Unauthorized) {
                        ended = Ended::Refused;
                    }
                    let _ = send_frame(
                        &out_tx,
                        ServerFrame::Reply { id, result },
                        &connection_cancel,
                    )
                    .await;
                    if !ok {
                        break;
                    }
                    continue;
                };
                // Before any branch below: subscriptions and shutdown never
                // reach dispatch, so a check there would not cover them.
                let allowed = auth
                    .authorize(&frame.command)
                    .and_then(|()| authz::guard(&daemon, auth, &frame.command));
                if let Err(refusal) = allowed {
                    let refused = ServerFrame::Reply {
                        id,
                        result: Err(refusal),
                    };
                    if !send_frame(&out_tx, refused, &connection_cancel).await {
                        break;
                    }
                    continue;
                }
                if let Err(error) = reap_subscriptions(&mut subscriptions).await {
                    tracing::error!(%error, "subscription task failed; closing its connection");
                    break;
                }
                let result = match frame.command {
                    Command::Subscribe { ref query, .. } if query.before.is_some() => {
                        Err(ErrorReply::invalid("before is only supported by GetEvents"))
                    }
                    Command::Subscribe { .. } if subscriptions.len() >= MAX_SUBSCRIPTIONS => {
                        Err(ErrorReply::new(
                            ErrorCode::Unavailable,
                            "connection subscription limit reached (16); unsubscribe before opening another",
                        ))
                    }
                    Command::Subscribe {
                        after,
                        query,
                        deltas,
                    } => {
                        let sub = next_sub;
                        next_sub += 1;
                        let cancel = connection_cancel.child_token();
                        let (last_seq, task) = match spawn_subscription(
                            daemon.clone(),
                            out_tx.clone(),
                            sub,
                            after,
                            query.into(),
                            deltas,
                            SubscriptionControl {
                                protocol_version: version.load(Ordering::SeqCst),
                                cancel: cancel.clone(),
                                failure: replay_failure_tx.clone(),
                                auth: auth.clone(),
                            },
                        ) {
                            Ok(seq) => seq,
                            Err(e) => {
                                let _ = send_frame(
                                    &out_tx,
                                    ServerFrame::Reply {
                                        id,
                                        result: Err(store_err(e)),
                                    },
                                    &connection_cancel,
                                )
                                .await;
                                continue;
                            }
                        };
                        assert!(
                            subscriptions.insert(sub, (cancel, task)).is_none(),
                            "subscription ids are unique"
                        );
                        debug_assert!(subscriptions.len() <= MAX_SUBSCRIPTIONS);
                        Ok(Reply::Subscribed { id: sub, last_seq })
                    }
                    Command::Unsubscribe { id: sub } => {
                        if let Some((cancel, task)) = subscriptions.remove(&sub) {
                            cancel.cancel();
                            if let Err(error) = task.await {
                                tracing::error!(%error, "unsubscribed task failed");
                                break;
                            }
                        }
                        Ok(Reply::Ok)
                    }
                    Command::Shutdown => {
                        let _ = send_frame(
                            &out_tx,
                            ServerFrame::Reply {
                                id,
                                result: Ok(Reply::Ok),
                            },
                            &connection_cancel,
                        )
                        .await;
                        shutdown.cancel();
                        break;
                    }
                    other => dispatch_version(&daemon, other, operator.clone(), auth).await,
                };
                if !send_frame(
                    &out_tx,
                    ServerFrame::Reply { id, result },
                    &connection_cancel,
                )
                .await
                {
                    break;
                }
            }
        };
        tokio::pin!(reading);
        tokio::select! {
            _ = &mut reading => false,
            _ = &mut writer => {
                connection_cancel.cancel();
                // Finish an accepted mutation before releasing this connection.
                reading.await;
                true
            },
        }
    };
    for (_, (cancel, task)) in subscriptions {
        cancel.cancel();
        if let Err(error) = task.await {
            tracing::error!(%error, "subscription cleanup failed");
        }
    }
    drop(out_tx);
    if !writer_finished
        && tokio::time::timeout(WRITER_DRAIN_TIMEOUT, &mut writer)
            .await
            .is_err()
    {
        tracing::debug!("closed a connection whose writer did not drain in time");
    }
    Ok(ended)
}

async fn reap_subscriptions(
    subscriptions: &mut HashMap<u64, (CancellationToken, tokio::task::JoinHandle<()>)>,
) -> Result<(), tokio::task::JoinError> {
    debug_assert!(subscriptions.len() <= MAX_SUBSCRIPTIONS);
    let finished: Vec<_> = subscriptions
        .iter()
        .filter_map(|(&id, (_, task))| task.is_finished().then_some(id))
        .collect();
    for id in finished {
        let Some((_, task)) = subscriptions.remove(&id) else {
            unreachable!("completed subscription ids came from this map");
        };
        assert!(
            task.is_finished(),
            "only completed tasks release their accounting"
        );
        task.await?;
    }
    Ok(())
}

/// The frame a carrier connection is sent before it may greet.
fn challenge_frame(daemon: &Daemon, policy: &ConnectionPolicy) -> Option<ServerFrame> {
    let challenge = &policy.device.as_ref()?.challenge;
    debug_assert_eq!(
        challenge.instance_id, daemon.instance_id,
        "a challenge names the daemon that issued it"
    );
    debug_assert!(!challenge.nonce.is_empty(), "a challenge carries a nonce");
    Some(ServerFrame::Challenge {
        nonce: challenge.nonce.clone(),
        instance_id: challenge.instance_id.clone(),
    })
}

/// Resolves when the device behind this connection is revoked or re-roled.
/// Never resolves for the host, or before the hello.
async fn revoked(standing: Option<&(AuthContext, Actor)>) {
    match standing.and_then(|(auth, _)| auth.closed()) {
        Some(closed) => closed.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn send_frame(
    out: &mpsc::Sender<ServerFrame>,
    frame: ServerFrame,
    cancel: &CancellationToken,
) -> bool {
    tokio::select! {
        result = out.send(frame) => result.is_ok(),
        _ = cancel.cancelled() => false,
    }
}

/// Capture `req` while Serde decodes the typed frame. Keeping MapAccess intact
/// preserves duplicate-field validation in commands and nested specifications.
fn parse_frame(line: &str) -> Result<(u64, Result<ClientFrame, serde_json::Error>), ErrorReply> {
    use serde::de::{DeserializeSeed, IntoDeserializer, MapAccess, Visitor};
    use serde::{Deserialize, Deserializer};

    struct CorrelatedMap<'a, A> {
        map: A,
        id: &'a mut Option<u64>,
        reading_req: bool,
    }
    impl<'de, A: MapAccess<'de>> MapAccess<'de> for CorrelatedMap<'_, A> {
        type Error = A::Error;
        fn next_key_seed<K: DeserializeSeed<'de>>(
            &mut self,
            seed: K,
        ) -> Result<Option<K::Value>, A::Error> {
            let Some(key) = self.map.next_key::<String>()? else {
                return Ok(None);
            };
            self.reading_req = key == "req";
            if self.reading_req && self.id.is_some() {
                *self.id = None;
                return Err(serde::de::Error::custom("duplicate field `req`"));
            }
            seed.deserialize(key.into_deserializer()).map(Some)
        }
        fn next_value_seed<V: DeserializeSeed<'de>>(
            &mut self,
            seed: V,
        ) -> Result<V::Value, A::Error> {
            if self.reading_req {
                let id = self.map.next_value::<u64>()?;
                if id == 0 {
                    return Err(serde::de::Error::custom("`req` must be nonzero"));
                }
                *self.id = Some(id);
                seed.deserialize(id.into_deserializer())
            } else {
                self.map.next_value_seed(seed)
            }
        }
    }
    struct FrameVisitor<'a>(&'a mut Option<u64>);
    impl<'de> Visitor<'de> for FrameVisitor<'_> {
        type Value = ClientFrame;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with a nonzero `req`")
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<ClientFrame, A::Error> {
            ClientFrame::deserialize(serde::de::value::MapAccessDeserializer::new(
                CorrelatedMap {
                    map,
                    id: self.0,
                    reading_req: false,
                },
            ))
        }
    }
    let mut id = None;
    let mut deserializer = serde_json::Deserializer::from_str(line);
    let decoded = (&mut deserializer).deserialize_map(FrameVisitor(&mut id));
    // Syntax/framing errors are connection errors even if `req` preceded them.
    if let Err(error) = &decoded
        && !error.is_data()
    {
        return Err(ErrorReply::invalid(format!(
            "bad frame: invalid `req`: {error}"
        )));
    }
    if let Err(error) = deserializer.end() {
        return Err(ErrorReply::invalid(format!(
            "bad frame: invalid `req`: {error}"
        )));
    }
    let id =
        id.ok_or_else(|| ErrorReply::invalid("bad frame: missing or ambiguous nonzero `req`"))?;
    Ok((id, decoded))
}

#[cfg(test)]
mod tests;
