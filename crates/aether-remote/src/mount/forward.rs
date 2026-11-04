//! One mounted request: read within the listener's limits, sent to the loopback target under a
//! lease ticket, and answered with what the target said. Nothing is added but `Host`.

use std::time::Duration;

use aether_protocol::remote::HttpErrorCode;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::Body as HttpBody;
use hyper::header::{
    CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderValue, SEC_WEBSOCKET_EXTENSIONS, UPGRADE,
};
use hyper::upgrade::OnUpgrade;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::leased::Leased;
use super::{Mounted, Ticket, Tunnel, end_to_end};
use crate::RemoteServer;
use crate::http::{MAX_BODY_BYTES, Refusal};

/// How long the target may take to accept a connection. It is on loopback.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the target may take to answer once it has the request.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest answer passed back from a target.
pub(crate) const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;

/// What a mounted request came to.
pub(crate) enum Forwarded {
    /// The target's answer, to pass on as it is.
    Answer(Response<Full<Bytes>>),
    /// The target accepted a WebSocket upgrade: the `101` to send, and what carries it after.
    Upgraded(Response<Full<Bytes>>, Tunnel),
}

/// Why a mounted request got no answer from its target.
#[derive(Debug)]
pub(crate) enum Failure {
    /// Refused here, before anything reached the target.
    Refused(Refusal),
    /// The target could not be reached or did not answer properly.
    Gateway(StatusCode, &'static str),
}

/// Forward `request` to the target `mounted` names, while the lease allows it and the mounts
/// it was routed by, whose `remounted` token it holds, are still in force.
pub(crate) async fn forward<B>(
    server: &RemoteServer,
    mounted: Mounted,
    remounted: CancellationToken,
    mut request: Request<B>,
) -> Result<Forwarded, Failure>
where
    B: HttpBody,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    assert!(
        mounted.target.ip().is_loopback(),
        "mount targets are loopback"
    );
    let upgrade = is_websocket_upgrade(request.headers());
    let slot = match upgrade {
        true => Some(server.mount_slot().ok_or_else(|| {
            refused(
                HttpErrorCode::Unavailable,
                "too many mounted connections are open",
            )
        })?),
        false => None,
    };
    let ticket = server
        .lease()
        .admit()
        .await
        .map_err(|why| refused(HttpErrorCode::Unavailable, why.message()))?;
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();
    let body = read_body(&parts.headers, body, server.tuning().body_timeout).await?;
    let outbound = outbound(&parts, &mounted, body, upgrade)?;
    // Held to the end: the answer's body and an upgrade both arrive through the connection.
    let (answered, _connection) = tokio::select! {
        biased;
        _ = ticket.ended() => return Err(lease_ended()),
        _ = remounted.cancelled() => return Err(mount_changed()),
        answered = exchange(&mounted, outbound, &ticket) => answered?,
    };
    if remounted.is_cancelled() {
        return Err(mount_changed());
    }
    match (client_upgrade, slot) {
        (Some(client), Some(slot)) if answered.status() == StatusCode::SWITCHING_PROTOCOLS => {
            switched(answered, client, ticket, slot, remounted).await
        }
        _ => passed_on(answered, &ticket).await,
    }
}

fn refused(code: HttpErrorCode, message: &'static str) -> Failure {
    Failure::Refused(Refusal::new(code, message))
}

fn mount_changed() -> Failure {
    refused(
        HttpErrorCode::Unavailable,
        "the mount this request was routed by was changed or removed",
    )
}

fn lease_ended() -> Failure {
    refused(
        HttpErrorCode::Unavailable,
        "mounted traffic stopped: the listener's publication status could not be confirmed",
    )
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let has = |name, wanted: &str| {
        headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|token| token.trim().eq_ignore_ascii_case(wanted))
    };
    has(CONNECTION, "upgrade") && has(UPGRADE, "websocket")
}

/// The request body, whole, within the listener's size cap and in time.
async fn read_body<B>(headers: &HeaderMap, body: B, patience: Duration) -> Result<Bytes, Failure>
where
    B: HttpBody,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let too_large = || {
        refused(
            HttpErrorCode::PayloadTooLarge,
            "the request body is too large",
        )
    };
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY_BYTES as u64) {
        return Err(too_large());
    }
    let reading = Limited::new(body, MAX_BODY_BYTES).collect();
    let body = match tokio::time::timeout(patience, reading).await {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(error)) if error.is::<LengthLimitError>() => return Err(too_large()),
        Ok(Err(_)) => {
            return Err(refused(
                HttpErrorCode::BadRequest,
                "the request body could not be read",
            ));
        }
        Err(_) => {
            return Err(refused(
                HttpErrorCode::BadRequest,
                "the request body did not arrive in time",
            ));
        }
    };
    assert!(body.len() <= MAX_BODY_BYTES, "the limit holds");
    Ok(body)
}

/// The request as the target gets it: method, stripped path, end-to-end headers and body as
/// they came, `Host` naming the target. An upgrade keeps its own two hop headers.
fn outbound(
    parts: &hyper::http::request::Parts,
    mounted: &Mounted,
    body: Bytes,
    upgrade: bool,
) -> Result<Request<Full<Bytes>>, Failure> {
    let internal = || refused(HttpErrorCode::Internal, "could not forward the request");
    let mut headers = end_to_end(&parts.headers);
    let host = HeaderValue::from_str(&mounted.target.to_string()).map_err(|_| internal())?;
    headers.insert(HOST, host);
    if upgrade {
        // Frames are relayed one by one, so no extension may change their bytes.
        headers.remove(SEC_WEBSOCKET_EXTENSIONS);
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    }
    let mut request = Request::builder()
        .method(parts.method.clone())
        .uri(mounted.path_and_query.as_str())
        .body(Full::new(body))
        .map_err(|_| internal())?;
    *request.headers_mut() = headers;
    debug_assert_eq!(request.headers().get_all(HOST).iter().count(), 1);
    Ok(request)
}

/// Aborts the target connection's task when the request is done with it.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Send `request` on a fresh connection to the target and wait for the head of its answer.
async fn exchange(
    mounted: &Mounted,
    request: Request<Full<Bytes>>,
    ticket: &Ticket,
) -> Result<(Response<hyper::body::Incoming>, AbortOnDrop), Failure> {
    let down = Failure::Gateway(
        StatusCode::BAD_GATEWAY,
        "the mounted daemon is not answering",
    );
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(mounted.target))
        .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::debug!(%error, target = %mounted.target, "a mount target refused the connection");
            return Err(down);
        }
        Err(_) => return Err(down),
    };
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(crate::listener::MAX_HEAD_BYTES)
        .handshake(TokioIo::new(Leased::new(stream, ticket.standing())))
        .await
        .map_err(|_| {
            Failure::Gateway(
                StatusCode::BAD_GATEWAY,
                "the mounted daemon is not answering",
            )
        })?;
    let connection = AbortOnDrop(tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%error, "a mounted daemon connection ended with an error");
        }
    }));
    if !ticket.live() {
        return Err(lease_ended());
    }
    let failed = match tokio::time::timeout(ANSWER_TIMEOUT, sender.send_request(request)).await {
        Ok(Ok(response)) => return Ok((response, connection)),
        Ok(Err(_)) => Failure::Gateway(
            StatusCode::BAD_GATEWAY,
            "the mounted daemon broke off its answer",
        ),
        Err(_) => Failure::Gateway(
            StatusCode::GATEWAY_TIMEOUT,
            "the mounted daemon did not answer in time",
        ),
    };
    Err(unless_lease_ended(failed, ticket))
}

/// A target socket fails once the lease is over: then that, not the target, is the cause.
fn unless_lease_ended(failed: Failure, ticket: &Ticket) -> Failure {
    match ticket.live() {
        true => failed,
        false => lease_ended(),
    }
}

/// A plain answer, read whole and passed on with its end-to-end headers. The connection
/// closes after it, so no kept-alive connection outlives the lease it was served under.
async fn passed_on(
    answered: Response<hyper::body::Incoming>,
    ticket: &Ticket,
) -> Result<Forwarded, Failure> {
    let (parts, body) = answered.into_parts();
    let reading = Limited::new(body, MAX_ANSWER_BYTES).collect();
    let body = tokio::select! {
        read = tokio::time::timeout(ANSWER_TIMEOUT, reading) => match read {
            Ok(Ok(body)) => body.to_bytes(),
            Ok(Err(_)) => {
                let broken = Failure::Gateway(StatusCode::BAD_GATEWAY, "the mounted daemon's answer was too large or broken");
                return Err(unless_lease_ended(broken, ticket));
            }
            Err(_) => return Err(Failure::Gateway(StatusCode::GATEWAY_TIMEOUT, "the mounted daemon did not answer in time")),
        },
        _ = ticket.ended() => return Err(lease_ended()),
    };
    assert!(body.len() <= MAX_ANSWER_BYTES, "the limit holds");
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = parts.status;
    *response.headers_mut() = end_to_end(&parts.headers);
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    Ok(Forwarded::Answer(response))
}

/// The target switched protocols: take its side of the connection, and answer the client's
/// upgrade with the target's own `101`.
async fn switched(
    mut answered: Response<hyper::body::Incoming>,
    client: OnUpgrade,
    ticket: Ticket,
    slot: tokio::sync::OwnedSemaphorePermit,
    remounted: CancellationToken,
) -> Result<Forwarded, Failure> {
    let target =
        match tokio::time::timeout(CONNECT_TIMEOUT, hyper::upgrade::on(&mut answered)).await {
            Ok(Ok(target)) => target,
            Ok(Err(_)) | Err(_) => {
                let failed = Failure::Gateway(
                    StatusCode::BAD_GATEWAY,
                    "the mounted daemon's upgrade did not complete",
                );
                return Err(unless_lease_ended(failed, &ticket));
            }
        };
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    *headers = end_to_end(answered.headers());
    // None was offered; one accepted anyway would have the client expect bytes no relay keeps.
    headers.remove(SEC_WEBSOCKET_EXTENSIONS);
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    let tunnel = Tunnel {
        client,
        target: TokioIo::new(target),
        ticket,
        remounted,
        slot,
    };
    Ok(Forwarded::Upgraded(response, tunnel))
}
