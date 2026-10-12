//! One HTTP request to one response: the checks in the order docs/remote.md
//! gives them, then the endpoint.

use std::time::Duration;

use aether_protocol::remote::{
    Health, HttpErrorCode, PairClaimRequest, PairClaimResponse, PairStatusResponse,
};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::Body as HttpBody;
use hyper::header::{
    ALLOW, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HOST, HeaderMap, HeaderName, HeaderValue, IF_NONE_MATCH, ORIGIN, RETRY_AFTER,
    SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_PROTOCOL, UPGRADE,
};
use hyper::upgrade::OnUpgrade;
use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{MAX_BODY_BYTES, Refusal, Route, SECURITY_HEADERS, allowed_methods, error_body, route};
use crate::lobby::Seat;
use crate::{Live, RemoteServer, assets, pairing, ws};

pub(crate) type Body = Full<Bytes>;

const JSON: &str = "application/json";
/// Endpoint answers are about this moment; nothing may keep one.
const NO_STORE: &str = "no-store";
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// A connection whose last response was `101`: what carries it from there.
pub(crate) struct Pending {
    pub on_upgrade: OnUpgrade,
    /// The allowlisted `Origin` the upgrade arrived with.
    pub origin: String,
    /// Where the socket waits until its hello is accepted.
    pub seat: Seat,
}

/// Where a request handler leaves an accepted upgrade for its connection.
#[derive(Default)]
pub(crate) struct UpgradeSlot(parking_lot::Mutex<Option<Pending>>);

impl UpgradeSlot {
    fn put(&self, pending: Pending) {
        let displaced = self.0.lock().replace(pending);
        debug_assert!(displaced.is_none(), "a connection upgrades at most once");
    }

    pub(crate) fn take(&self) -> Option<Pending> {
        self.0.lock().take()
    }
}

/// Answer one request. Never fails: every refusal is a response too.
pub(crate) async fn respond<B>(
    server: &RemoteServer,
    request: Request<B>,
    upgrade: &UpgradeSlot,
) -> Response<Body>
where
    B: HttpBody,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let live = server.live();
    let response = match answer(server, &live, request, upgrade).await {
        Ok(response) => response,
        Err(refusal) => refuse(&refusal),
    };
    harden(response)
}

async fn answer<B>(
    server: &RemoteServer,
    live: &Live,
    mut request: Request<B>,
    upgrade: &UpgradeSlot,
) -> Result<Response<Body>, Refusal>
where
    B: HttpBody,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    check_host(live, &request)?;
    if server.is_stopping() || live.bound.is_none() {
        return Err(Refusal::new(
            HttpErrorCode::Unavailable,
            "the daemon is shutting down",
        ));
    }
    let route = find_route(request.method(), request.uri().path())?;
    let origin = match route.needs_origin() {
        true => check_origin(live, request.headers())?,
        false => String::new(),
    };
    match route {
        Route::Health => Ok(json(StatusCode::OK, &health(server))),
        Route::PairClaim => {
            let claim: ClaimBody = read_json(request, server.tuning().body_timeout).await?;
            claim_pairing(server, claim)
                .await
                .map(|r| json(StatusCode::OK, &r))
        }
        Route::PairStatus => {
            let asked: StatusBody = read_json(request, server.tuning().body_timeout).await?;
            pairing_status(server, &asked.pairing).map(|r| json(StatusCode::OK, &r))
        }
        Route::WebSocket => switch_protocols(server, &mut request, origin, upgrade),
        Route::Asset(path) => asset(&path, &request),
    }
}

/// `Host` must name this listener, whatever else the request says.
fn check_host<B>(live: &Live, request: &Request<B>) -> Result<(), Refusal> {
    let refused = || Refusal::new(HttpErrorCode::ForbiddenHost, "this host is not served here");
    let host = single(request.headers(), &HOST).ok_or_else(refused)?;
    if !live.policy.allows_host(host) {
        return Err(refused());
    }
    // An absolute-form target carries a host of its own; it must agree.
    match request.uri().authority() {
        Some(authority) if !live.policy.allows_host(authority.as_str()) => Err(refused()),
        _ => Ok(()),
    }
}

fn find_route(method: &Method, path: &str) -> Result<Route, Refusal> {
    route(method, path).map_err(|code| {
        let message = match code {
            HttpErrorCode::NotFound => "no such endpoint",
            HttpErrorCode::MethodNotAllowed => "this path does not take that method",
            _ => "the request path is not one this listener serves",
        };
        let mut refusal = Refusal::new(code, message);
        if code == HttpErrorCode::MethodNotAllowed {
            refusal.allow = allowed_methods(path);
        }
        refusal
    })
}

/// `Origin` must be allowlisted, and a browser that says where the request
/// came from must say it came from the page itself.
fn check_origin(live: &Live, headers: &HeaderMap) -> Result<String, Refusal> {
    let refused = || Refusal::new(HttpErrorCode::ForbiddenOrigin, "this origin is not allowed");
    let origin = single(headers, &ORIGIN).ok_or_else(refused)?;
    if !live.policy.allows_origin(origin) {
        return Err(refused());
    }
    let cross_site = headers
        .get_all(SEC_FETCH_SITE)
        .into_iter()
        .any(|site| site.as_bytes() != b"same-origin");
    if cross_site {
        return Err(refused());
    }
    Ok(origin.to_owned())
}

/// The value of a header sent exactly once, as text.
fn single<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a str> {
    let mut values = headers.get_all(name).into_iter();
    match (values.next(), values.next()) {
        (Some(only), None) => only.to_str().ok(),
        _ => None,
    }
}

fn health(server: &RemoteServer) -> Health {
    Health {
        status: "ok".into(),
        protocol_version: aether_protocol::PROTOCOL_VERSION,
        min_protocol_version: aether_protocol::compat::MIN_PROTOCOL_VERSION,
        pairing_open: server.pairing().lock().is_open(aether_core::time::now()),
    }
}

/// `POST /api/pair/claim`. No `Debug`: the code is a credential.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimBody {
    code: String,
    public_key: String,
    device_name: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusBody {
    pairing: String,
}

async fn claim_pairing(
    server: &RemoteServer,
    claim: ClaimBody,
) -> Result<PairClaimResponse, Refusal> {
    let request = PairClaimRequest {
        code: claim.code,
        public_key: claim.public_key,
        device_name: claim.device_name,
    };
    let claimed = server
        .pairing()
        .lock()
        .claim(&request, aether_core::time::now());
    let refused = match claimed {
        Ok(response) => return Ok(response),
        Err(refused) => refused,
    };
    if refused.delayed {
        tokio::time::sleep(pairing::WRONG_CODE_DELAY).await;
    }
    Err(refused.refusal)
}

fn pairing_status(server: &RemoteServer, pairing: &str) -> Result<PairStatusResponse, Refusal> {
    server
        .pairing()
        .lock()
        .status(pairing, aether_core::time::now())
        .map_err(|code| Refusal::new(code, "no such pairing; it may have been forgotten"))
}

/// Read a JSON body: the right media type, within the size cap, in time,
/// and with no field the endpoint does not take.
async fn read_json<B, T>(request: Request<B>, patience: Duration) -> Result<T, Refusal>
where
    B: HttpBody,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T: DeserializeOwned,
{
    let too_large = || {
        Refusal::new(
            HttpErrorCode::PayloadTooLarge,
            format!("the request body may be at most {MAX_BODY_BYTES} bytes"),
        )
    };
    if !is_json(request.headers()) {
        return Err(Refusal::new(
            HttpErrorCode::UnsupportedMediaType,
            "the request body must be application/json",
        ));
    }
    let declared = single(request.headers(), &CONTENT_LENGTH).and_then(|n| n.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY_BYTES as u64) {
        return Err(too_large());
    }
    let reading = Limited::new(request.into_body(), MAX_BODY_BYTES).collect();
    let body = match tokio::time::timeout(patience, reading).await {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(error)) if error.is::<LengthLimitError>() => return Err(too_large()),
        Ok(Err(_)) => return Err(bad_request("the request body could not be read")),
        Err(_) => return Err(bad_request("the request body did not arrive in time")),
    };
    debug_assert!(body.len() <= MAX_BODY_BYTES, "the limit holds");
    serde_json::from_slice(&body)
        .map_err(|error| bad_request(format!("invalid request body: {error}")))
}

fn bad_request(message: impl Into<std::borrow::Cow<'static, str>>) -> Refusal {
    Refusal::new(HttpErrorCode::BadRequest, message)
}

/// Whether `Content-Type` is `application/json`, parameters aside.
fn is_json(headers: &HeaderMap) -> bool {
    single(headers, &CONTENT_TYPE)
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case(JSON))
}

/// Answer `GET /ws` with `101` and leave the connection for the carrier.
fn switch_protocols<B>(
    server: &RemoteServer,
    request: &mut Request<B>,
    origin: String,
    upgrade: &UpgradeSlot,
) -> Result<Response<Body>, Refusal> {
    let accept = ws::accept(request.headers()).map_err(bad_request)?;
    let seat = server.greeting_seat().ok_or_else(|| {
        Refusal::new(
            HttpErrorCode::Unavailable,
            "too many connections are waiting to authenticate; try again shortly",
        )
    })?;
    let accept = HeaderValue::from_str(&accept)
        .map_err(|_| Refusal::new(HttpErrorCode::Internal, "could not answer the upgrade"))?;
    upgrade.put(Pending {
        on_upgrade: hyper::upgrade::on(request),
        origin,
        seat,
    });
    let mut response = Response::new(Body::default());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(ws::SUBPROTOCOL),
    );
    Ok(response)
}

/// An embedded file, or the app shell for a client-side route.
fn asset<B>(path: &str, request: &Request<B>) -> Result<Response<Body>, Refusal> {
    let asset = assets::resolve(path)
        .ok_or_else(|| Refusal::new(HttpErrorCode::NotFound, "no such file"))?;
    let etag = HeaderValue::from_str(&assets::etag(&asset))
        .map_err(|_| Refusal::new(HttpErrorCode::Internal, "could not name the file's version"))?;
    let caching = assets::cache_control(&asset, request.uri().query());
    let fresh = request
        .headers()
        .get_all(IF_NONE_MATCH)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
        .any(|validators| assets::is_fresh(validators, &asset));
    let mut response = Response::new(Body::default());
    let headers = response.headers_mut();
    headers.insert(ETAG, etag);
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(caching));
    if fresh {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        return Ok(response);
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(asset.bytes.len()));
    if request.method() != Method::HEAD {
        *response.body_mut() = Body::new(Bytes::from_static(asset.bytes));
    }
    Ok(response)
}

fn json<T: Serialize>(status: StatusCode, body: &T) -> Response<Body> {
    let (status, bytes) = match serde_json::to_vec(body) {
        Ok(bytes) => (status, bytes),
        Err(error) => {
            tracing::error!(%error, "could not encode a response body");
            let fallback =
                br#"{"error":{"code":"internal","message":"could not encode the response"}}"#;
            (StatusCode::INTERNAL_SERVER_ERROR, fallback.to_vec())
        }
    };
    let mut response = Response::new(Body::new(Bytes::from(bytes)));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(JSON));
    response
}

fn refuse(refusal: &Refusal) -> Response<Body> {
    let body = error_body(refusal.code, refusal.message.as_ref());
    let mut response = json(super::status_of(refusal.code), &body);
    if let Some(seconds) = refusal.retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(seconds));
    }
    if !refusal.allow.is_empty() {
        let allow = refusal.allow.join(", ");
        match HeaderValue::from_str(&allow) {
            Ok(allow) => drop(response.headers_mut().insert(ALLOW, allow)),
            Err(error) => tracing::error!(%error, "could not name the allowed methods"),
        }
    }
    response
}

/// The headers every response carries, whatever it is.
fn harden(mut response: Response<Body>) -> Response<Body> {
    let headers = response.headers_mut();
    let csp = HeaderValue::from_static(super::CONTENT_SECURITY_POLICY);
    headers.insert(CONTENT_SECURITY_POLICY, csp);
    for (name, value) in SECURITY_HEADERS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    if !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    }
    response
}

#[cfg(test)]
mod tests;
