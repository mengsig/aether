use std::sync::Arc;

use aether_protocol::remote::{DeviceRole, HttpError, PairingState, RemoteSettings};
use aws_lc_rs::signature::KeyPair as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hyper::header::CONTENT_SECURITY_POLICY;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::http::{HEALTH, PAIR_CLAIM, PAIR_STATUS, WEBSOCKET};
use crate::{Tuning, testing};

const PUBLIC: &str = "https://host.tailnet.example";

/// A listening server, and the names a browser on the host reaches it by.
struct Site {
    server: Arc<RemoteServer>,
    host: String,
    origin: String,
    shutdown: CancellationToken,
}

async fn site() -> Site {
    let (server, _devices, shutdown) = testing::server(Tuning::default());
    let settings = RemoteSettings {
        enabled: true,
        pairing: true,
        public_origins: vec![PUBLIC.into()],
        push: false,
    };
    server.apply(settings).await.expect("apply");
    let bound = server.listening().expect("the listener is open");
    Site {
        server,
        host: bound.to_string(),
        origin: format!("http://{bound}"),
        shutdown,
    }
}

impl Site {
    fn get(&self, target: &str) -> hyper::http::request::Builder {
        Request::builder().uri(target).header(HOST, &self.host)
    }

    fn post(&self, target: &str) -> hyper::http::request::Builder {
        self.get(target)
            .method(Method::POST)
            .header(ORIGIN, &self.origin)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
    }

    fn upgrade(&self) -> hyper::http::request::Builder {
        self.get(WEBSOCKET)
            .header(ORIGIN, &self.origin)
            .header(CONNECTION, "Upgrade")
            .header(UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header(SEC_WEBSOCKET_PROTOCOL, "aether")
    }

    async fn send(&self, request: hyper::http::request::Builder, body: &str) -> Answer {
        let request = request
            .body(Body::from(body.to_owned()))
            .expect("a request");
        self.answer(request, &UpgradeSlot::default()).await
    }

    async fn answer(&self, request: Request<Body>, slot: &UpgradeSlot) -> Answer {
        let response = respond(&self.server, request, slot).await;
        let (head, body) = response.into_parts();
        let body = body.collect().await.expect("a body").to_bytes();
        Answer { head, body }
    }

    /// A claim body for the open pairing's `code`, with a fresh key.
    fn claim(code: &str) -> String {
        let key = testing::key_pair();
        let public_key = URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
        serde_json::json!({"code": code, "public_key": public_key, "device_name": "phone"})
            .to_string()
    }
}

/// Set a header, replacing the one a helper already put there.
fn with(
    mut request: hyper::http::request::Builder,
    name: HeaderName,
    value: &str,
) -> hyper::http::request::Builder {
    let headers = request.headers_mut().expect("a well-formed request so far");
    headers.insert(name, value.parse().expect("a header value"));
    request
}

struct Answer {
    head: hyper::http::response::Parts,
    body: Bytes,
}

impl Answer {
    fn header(&self, name: impl hyper::header::AsHeaderName) -> &str {
        let value = self.head.headers.get(name).expect("the header is set");
        value.to_str().expect("a text header")
    }

    /// The refusal's code, checked against the status that goes with it.
    fn refused(&self) -> HttpErrorCode {
        let error: HttpError = serde_json::from_slice(&self.body).expect("an error body");
        assert_eq!(self.head.status, crate::http::status_of(error.error.code));
        assert_eq!(self.header(CONTENT_TYPE), JSON);
        error.error.code
    }

    fn json<T: DeserializeOwned>(&self) -> T {
        assert_eq!(self.head.status, StatusCode::OK, "{:?}", self.body);
        serde_json::from_slice(&self.body).expect("the documented body")
    }
}

#[tokio::test]
async fn every_response_carries_the_csp_and_the_security_headers() {
    let site = site().await;
    let answers = [
        site.send(site.get(HEALTH), "").await,
        site.send(site.get("/"), "").await,
        site.send(site.get("/api/nope"), "").await,
        site.send(with(site.get(HEALTH), HOST, "evil.example"), "")
            .await,
        site.send(site.upgrade(), "").await,
    ];
    for answer in &answers {
        let csp = answer.header(CONTENT_SECURITY_POLICY);
        assert_eq!(csp, crate::http::CONTENT_SECURITY_POLICY);
        assert!(csp.contains("connect-src 'self';"), "{csp}");
        // No host is a source: `ws://[::1]:<port>` is one no browser accepts.
        assert!(!csp.contains('[') && !csp.contains("ws"), "{csp}");
        for (name, value) in SECURITY_HEADERS {
            assert_eq!(answer.header(*name), *value, "{name}");
        }
    }
    assert_eq!(answers[0].header(CACHE_CONTROL), NO_STORE);
    assert_eq!(answers[2].header(CACHE_CONTROL), NO_STORE);
}

#[tokio::test]
async fn a_host_this_listener_is_not_known_by_is_refused_first() {
    let site = site().await;
    let nameless = Request::builder().uri(HEALTH);
    let twice = site.get(HEALTH).header(HOST, &site.host);
    let absolute = site.get("http://evil.example/api/health");
    for (request, why) in [
        (
            with(site.get(HEALTH), HOST, "evil.example"),
            "a rebound name",
        ),
        (with(site.get(HEALTH), HOST, "127.0.0.1:1"), "another port"),
        (nameless, "no host at all"),
        (twice, "two hosts"),
        (absolute, "a target naming another host"),
        // Checked before the route: an unknown host learns nothing else.
        (
            with(site.get("/api/nope"), HOST, "evil.example"),
            "an unknown path",
        ),
    ] {
        let answer = site.send(request, "").await;
        assert_eq!(answer.refused(), HttpErrorCode::ForbiddenHost, "{why}");
    }
    let public = Request::builder()
        .uri(HEALTH)
        .header(HOST, "HOST.tailnet.example");
    assert_eq!(site.send(public, "").await.head.status, StatusCode::OK);
    let localhost = format!(
        "localhost:{}",
        site.server.listening().expect("open").port()
    );
    let local = Request::builder().uri(HEALTH).header(HOST, localhost);
    assert_eq!(site.send(local, "").await.head.status, StatusCode::OK);
}

#[tokio::test]
async fn paths_and_methods_are_refused_before_origins() {
    let site = site().await;
    let unknown = site.send(site.get("/api/nope"), "").await;
    assert_eq!(unknown.refused(), HttpErrorCode::NotFound);
    let traversal = site.send(site.get("/ui/../../etc/passwd"), "").await;
    assert_eq!(traversal.refused(), HttpErrorCode::BadRequest);
    let missing = site.send(site.get("/missing.js"), "").await;
    assert_eq!(missing.refused(), HttpErrorCode::NotFound);

    let wrong = site.send(site.get(PAIR_CLAIM), "").await;
    assert_eq!(wrong.refused(), HttpErrorCode::MethodNotAllowed);
    assert_eq!(wrong.header(ALLOW), "POST");
    let written = site
        .send(site.get("/index.html").method(Method::PUT), "")
        .await;
    assert_eq!(written.refused(), HttpErrorCode::MethodNotAllowed);
    assert_eq!(written.header(ALLOW), "GET, HEAD");
}

#[tokio::test]
async fn posts_and_upgrades_need_an_allowlisted_same_origin_request() {
    let site = site().await;
    let json = "application/json";
    let no_origin = site
        .get(PAIR_STATUS)
        .method(Method::POST)
        .header(CONTENT_TYPE, json);
    let foreign = with(site.post(PAIR_STATUS), ORIGIN, "https://evil.example");
    let null = with(site.post(PAIR_CLAIM), ORIGIN, "null");
    let cross_site = site.post(PAIR_CLAIM).header("sec-fetch-site", "cross-site");
    let same_site = site.upgrade().header("sec-fetch-site", "same-site");
    let hijack = with(site.upgrade(), ORIGIN, "https://evil.example");
    let trailing = with(site.upgrade(), ORIGIN, &format!("{}/", site.origin));
    let two_origins = site.upgrade().header(ORIGIN, &site.origin);
    for (request, why) in [
        (no_origin, "no origin"),
        (foreign, "a foreign origin"),
        (null, "an opaque origin"),
        (cross_site, "a cross-site fetch"),
        (same_site, "a same-site sibling"),
        (hijack, "a cross-site WebSocket"),
        (trailing, "an origin that is not byte-exact"),
        (two_origins, "two origins"),
    ] {
        let answer = site.send(request, "{}").await;
        assert_eq!(answer.refused(), HttpErrorCode::ForbiddenOrigin, "{why}");
    }
    let public = with(site.post(PAIR_STATUS), HOST, "host.tailnet.example");
    let public = with(public, ORIGIN, PUBLIC).header("sec-fetch-site", "same-origin");
    let answer = site.send(public, r#"{"pairing":"unknown"}"#).await;
    assert_eq!(
        answer.refused(),
        HttpErrorCode::NotFound,
        "allowed, then looked up"
    );
}

#[tokio::test]
async fn health_reports_versions_and_whether_a_pairing_is_open() {
    let site = site().await;
    let before: Health = site.send(site.get(HEALTH), "").await.json();
    assert_eq!(before.status, "ok");
    assert_eq!(before.protocol_version, aether_protocol::PROTOCOL_VERSION);
    assert!(before.min_protocol_version <= before.protocol_version);
    assert!(!before.pairing_open);
    site.server
        .start_pairing(DeviceRole::Operator)
        .expect("pairing is on");
    let during: Health = site.send(site.get(HEALTH), "").await.json();
    assert!(during.pairing_open);
}

#[tokio::test]
async fn a_request_body_must_be_small_json_with_known_fields() {
    let site = site().await;
    let pairing = site
        .server
        .start_pairing(DeviceRole::Operator)
        .expect("open");
    let good = Site::claim(pairing.code.as_deref().expect("the code"));

    let form = with(site.post(PAIR_CLAIM), CONTENT_TYPE, "text/plain");
    let typed = site.send(form, &good).await;
    assert_eq!(typed.refused(), HttpErrorCode::UnsupportedMediaType);
    let untyped = site
        .get(PAIR_CLAIM)
        .method(Method::POST)
        .header(ORIGIN, &site.origin);
    let untyped = site.send(untyped, &good).await;
    assert_eq!(untyped.refused(), HttpErrorCode::UnsupportedMediaType);

    let padded = format!("{}{}", good, " ".repeat(MAX_BODY_BYTES));
    let large = site.send(site.post(PAIR_CLAIM), &padded).await;
    assert_eq!(large.refused(), HttpErrorCode::PayloadTooLarge);
    let declared = site.post(PAIR_CLAIM).header(CONTENT_LENGTH, "1000000");
    let declared = site.send(declared, "{}").await;
    assert_eq!(declared.refused(), HttpErrorCode::PayloadTooLarge);

    let extra = good.replacen('{', r#"{"role":"operator","#, 1);
    for (body, why) in [
        (extra.as_str(), "an unknown field"),
        (r#"{"code":"AAAA-AAAA"}"#, "missing fields"),
        ("{not json", "malformed JSON"),
        ("", "no body"),
    ] {
        let answer = site.send(site.post(PAIR_CLAIM), body).await;
        assert_eq!(answer.refused(), HttpErrorCode::BadRequest, "{why}");
    }
    // None of that spent the code.
    let claimed: PairClaimResponse = site.send(site.post(PAIR_CLAIM), &good).await.json();
    assert_eq!(claimed.pairing, pairing.id);
}

#[tokio::test(start_paused = true)]
async fn a_claim_is_answered_by_the_state_of_the_pairing() {
    let site = site().await;
    let closed = site
        .send(site.post(PAIR_CLAIM), &Site::claim("AAAA-AAAA"))
        .await;
    assert_eq!(closed.refused(), HttpErrorCode::PairingClosed);

    let pairing = site
        .server
        .start_pairing(DeviceRole::Observer)
        .expect("open");
    let code = pairing.code.clone().expect("the code");
    let asked = tokio::time::Instant::now();
    let wrong = site
        .send(site.post(PAIR_CLAIM), &Site::claim("AAAA-AAAA"))
        .await;
    assert_eq!(wrong.refused(), HttpErrorCode::InvalidCode);
    assert!(
        asked.elapsed() >= pairing::WRONG_CODE_DELAY,
        "a wrong guess is slow"
    );

    let claimed: PairClaimResponse = site
        .send(site.post(PAIR_CLAIM), &Site::claim(&code))
        .await
        .json();
    assert_eq!(claimed.fingerprint.len(), 64);
    assert_eq!(claimed.phrase.split('-').count(), 3, "{}", claimed.phrase);
    let again = site.send(site.post(PAIR_CLAIM), &Site::claim(&code)).await;
    assert_eq!(
        again.refused(),
        HttpErrorCode::InvalidCode,
        "a code works once"
    );

    let status = format!(r#"{{"pairing":"{}"}}"#, claimed.pairing);
    let waiting: PairStatusResponse = site.send(site.post(PAIR_STATUS), &status).await.json();
    assert_eq!(
        (waiting.state, waiting.device),
        (PairingState::Claimed, None)
    );
    site.server
        .confirm_pairing(&pairing.id, false, |_| Err("never asked".into()))
        .expect("a claimed pairing can be rejected");
    let rejected: PairStatusResponse = site.send(site.post(PAIR_STATUS), &status).await.json();
    assert_eq!(rejected.state, PairingState::Rejected);
}

#[tokio::test(start_paused = true)]
async fn the_eleventh_guess_in_a_second_is_rate_limited() {
    let site = site().await;
    for attempt in 0..=pairing::MAX_GUESSES_PER_SECOND {
        let answer = site
            .send(site.post(PAIR_CLAIM), &Site::claim("AAAA-AAAA"))
            .await;
        assert_eq!(
            answer.refused(),
            HttpErrorCode::PairingClosed,
            "claim {attempt} of nothing is not a guess"
        );
    }
    site.server
        .start_pairing(DeviceRole::Observer)
        .expect("open");
    // The clock is paused, so the delayed answers take no real time.
    for attempt in 0..pairing::MAX_GUESSES_PER_SECOND {
        let answer = site
            .send(site.post(PAIR_CLAIM), &Site::claim("AAAA-AAAA"))
            .await;
        assert_eq!(
            answer.refused(),
            HttpErrorCode::InvalidCode,
            "guess {attempt}"
        );
    }
    let limited = site
        .send(site.post(PAIR_CLAIM), &Site::claim("AAAA-AAAA"))
        .await;
    assert_eq!(limited.refused(), HttpErrorCode::RateLimited);
    assert_eq!(limited.header(RETRY_AFTER), "1");
}

#[tokio::test]
async fn the_shell_is_served_with_a_validator_and_revalidated() {
    let site = site().await;
    let shell = site.send(site.get("/"), "").await;
    assert_eq!(shell.head.status, StatusCode::OK);
    assert_eq!(shell.header(CONTENT_TYPE), "text/html; charset=utf-8");
    assert_eq!(shell.header(CACHE_CONTROL), assets::CACHE_CONTROL);
    assert_eq!(shell.header(CONTENT_LENGTH), shell.body.len().to_string());
    let etag = shell.header(ETAG).to_owned();
    // A client-side route is the same document.
    let route = site.send(site.get("/pair?code=ignored"), "").await;
    assert_eq!(
        (route.header(ETAG), &route.body),
        (etag.as_str(), &shell.body)
    );

    let head = site.send(site.get("/").method(Method::HEAD), "").await;
    assert_eq!(head.head.status, StatusCode::OK);
    assert_eq!(head.header(CONTENT_LENGTH), shell.body.len().to_string());
    assert!(head.body.is_empty());

    let fresh = site
        .send(site.get("/").header(IF_NONE_MATCH, &etag), "")
        .await;
    assert_eq!(fresh.head.status, StatusCode::NOT_MODIFIED);
    assert_eq!(fresh.header(ETAG), etag);
    assert!(fresh.body.is_empty());
    let stale = site
        .send(site.get("/").header(IF_NONE_MATCH, "\"old\""), "")
        .await;
    assert_eq!(stale.head.status, StatusCode::OK);

    let manifest = site.send(site.get(assets::MANIFEST), "").await;
    assert_eq!(manifest.header(CONTENT_TYPE), "application/json");
    assert_eq!(manifest.header(CACHE_CONTROL), assets::CACHE_CONTROL);
}

#[tokio::test]
async fn an_upgrade_is_answered_with_101_until_the_connection_cap() {
    let site = site().await;
    let plain = site
        .send(site.get(WEBSOCKET).header(ORIGIN, &site.origin), "")
        .await;
    assert_eq!(plain.refused(), HttpErrorCode::BadRequest);
    let unnamed = with(site.upgrade(), SEC_WEBSOCKET_PROTOCOL, "chat");
    assert_eq!(
        site.send(unnamed, "").await.refused(),
        HttpErrorCode::BadRequest
    );

    let cap = site.server.config().max_connections;
    let mut held = Vec::new();
    for _ in 0..cap {
        let slot = UpgradeSlot::default();
        let request = site.upgrade().body(Body::default()).expect("a request");
        let switched = site.answer(request, &slot).await;
        assert_eq!(switched.head.status, StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            switched.header(SEC_WEBSOCKET_ACCEPT),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(switched.header(SEC_WEBSOCKET_PROTOCOL), ws::SUBPROTOCOL);
        assert_eq!(switched.header(UPGRADE), "websocket");
        let pending = slot.take().expect("the upgrade is left for the connection");
        assert_eq!(pending.origin, site.origin);
        held.push(pending);
    }
    let full = site.send(site.upgrade(), "").await;
    assert_eq!(full.refused(), HttpErrorCode::Unavailable);
    held.pop();
    let freed = site.send(site.upgrade(), "").await;
    assert_eq!(freed.head.status, StatusCode::SWITCHING_PROTOCOLS);
}

#[tokio::test]
async fn a_stopping_daemon_answers_unavailable() {
    let site = site().await;
    site.shutdown.cancel();
    let answer = site.send(site.get(HEALTH), "").await;
    assert_eq!(answer.refused(), HttpErrorCode::Unavailable);
    assert!(answer.head.headers.contains_key(CONTENT_SECURITY_POLICY));
}
