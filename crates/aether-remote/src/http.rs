//! HTTP routing for the remote listener: the endpoints that cannot ride the socket, and the
//! headers every response carries. Business logic is behind the WebSocket carrier.

pub(crate) mod respond;

use std::borrow::Cow;
use std::net::SocketAddr;

use aether_protocol::remote::{HttpError, HttpErrorBody, HttpErrorCode};
use hyper::{Method, StatusCode};

pub const HEALTH: &str = "/api/health";
pub const PAIR_CLAIM: &str = "/api/pair/claim";
pub const PAIR_STATUS: &str = "/api/pair/status";
pub const WEBSOCKET: &str = "/ws";
/// Everything under here is an endpoint, never an asset or an app route.
pub const API_PREFIX: &str = "/api/";

/// Largest request body accepted on any endpoint.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Public origins one listener answers for. Far above any real setup, and
/// it bounds the CSP header every response carries.
pub const MAX_PUBLIC_ORIGINS: usize = 16;

/// Names a browser on the host itself reaches the listener by.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// Where one request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Health,
    PairClaim,
    PairStatus,
    WebSocket,
    /// An embedded file, or the app shell for a client-side route.
    Asset(String),
}

impl Route {
    /// Whether the request must carry an allowlisted `Origin`. Asset reads
    /// need only an allowlisted `Host`.
    pub fn needs_origin(&self) -> bool {
        matches!(self, Self::PairClaim | Self::PairStatus | Self::WebSocket)
    }
}

/// The route for a method and a path, with the query already removed.
pub fn route(method: &Method, path: &str) -> Result<Route, HttpErrorCode> {
    let endpoint = matches!(path, HEALTH | WEBSOCKET | PAIR_CLAIM | PAIR_STATUS);
    if !endpoint && path.starts_with(API_PREFIX) {
        return Err(HttpErrorCode::NotFound);
    }
    if !endpoint && !is_asset_path(path) {
        return Err(HttpErrorCode::BadRequest);
    }
    if !allowed_methods(path).contains(&method.as_str()) {
        return Err(HttpErrorCode::MethodNotAllowed);
    }
    Ok(match path {
        HEALTH => Route::Health,
        WEBSOCKET => Route::WebSocket,
        PAIR_CLAIM => Route::PairClaim,
        PAIR_STATUS => Route::PairStatus,
        _ => Route::Asset(path.to_owned()),
    })
}

/// The methods a routable path takes: what a `405` names in `Allow`.
/// `HEAD` reads an asset; an endpoint has no headers worth asking for.
pub fn allowed_methods(path: &str) -> &'static [&'static str] {
    match path {
        HEALTH | WEBSOCKET => &["GET"],
        PAIR_CLAIM | PAIR_STATUS => &["POST"],
        _ => &["GET", "HEAD"],
    }
}

/// A path that can name an embedded file: absolute, no empty, `.` or `..`
/// segment, no backslash or percent-escape.
fn is_asset_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    !path.contains(['\\', '%', '\0'])
        && rest
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// The status each refusal is sent with.
pub fn status_of(code: HttpErrorCode) -> StatusCode {
    match code {
        HttpErrorCode::BadRequest => StatusCode::BAD_REQUEST,
        HttpErrorCode::ForbiddenHost | HttpErrorCode::ForbiddenOrigin => StatusCode::FORBIDDEN,
        HttpErrorCode::PairingClosed => StatusCode::FORBIDDEN,
        HttpErrorCode::InvalidCode => StatusCode::FORBIDDEN,
        HttpErrorCode::NotFound => StatusCode::NOT_FOUND,
        HttpErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        HttpErrorCode::Expired => StatusCode::GONE,
        HttpErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        HttpErrorCode::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        HttpErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        HttpErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        HttpErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// The JSON body of a refusal.
pub fn error_body(code: HttpErrorCode, message: impl Into<String>) -> HttpError {
    HttpError {
        error: HttpErrorBody {
            code,
            message: message.into(),
        },
    }
}

/// Why a request was refused, and the headers that go with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: HttpErrorCode,
    pub message: Cow<'static, str>,
    /// Seconds, on `rate_limited`.
    pub retry_after: Option<u64>,
    /// The methods the path does take, on `method_not_allowed`.
    pub allow: &'static [&'static str],
}

impl Refusal {
    pub fn new(code: HttpErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after: None,
            allow: &[],
        }
    }
}

/// One configured public origin, checked: `https://host[:port]` exactly as
/// a browser serializes it, so it compares byte for byte with `Origin`.
pub fn public_origin(raw: &str) -> Result<String, String> {
    let url = url::Url::parse(raw).map_err(|e| format!("`{raw}` is not a URL: {e}"))?;
    if url.scheme() != "https" {
        return Err(format!(
            "`{raw}` is not an https origin; browsers only give a remote page a signing key over https"
        ));
    }
    let bare = url.path() == "/" && url.query().is_none() && url.fragment().is_none();
    let anonymous = url.username().is_empty() && url.password().is_none();
    if !bare || !anonymous || raw.ends_with('/') {
        return Err(format!(
            "`{raw}` must be an origin alone: https://host[:port] with no path, query, user or trailing slash"
        ));
    }
    let canonical = url.origin().ascii_serialization();
    if canonical != raw {
        return Err(format!(
            "`{raw}` is not how a browser writes that origin; use `{canonical}`"
        ));
    }
    Ok(canonical)
}

/// The public origins of a settings change: each checked, duplicates
/// dropped, order kept (the first is the one pairing URLs use).
pub fn public_origins(raw: &[String]) -> Result<Vec<String>, String> {
    let mut origins: Vec<String> = Vec::with_capacity(raw.len());
    for origin in raw {
        let origin = public_origin(origin)?;
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    if origins.len() > MAX_PUBLIC_ORIGINS {
        return Err(format!(
            "{} public origins; at most {MAX_PUBLIC_ORIGINS} are served",
            origins.len()
        ));
    }
    Ok(origins)
}

/// Which `Host` and `Origin` values reach this listener legitimately: the
/// configured public origins, and loopback on the bound port.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OriginPolicy {
    /// Serialized origins, e.g. `https://host.tailnet.ts.net`. Public first.
    origins: Vec<String>,
    /// The `host[:port]` of each origin, as a `Host` header carries it.
    hosts: Vec<String>,
}

impl OriginPolicy {
    /// Build the allowlist. Refuses an origin that is not `https://host[:port]`
    /// with nothing after it.
    pub fn new(public_origins: &[String], bound: SocketAddr) -> Result<Self, String> {
        let mut origins = self::public_origins(public_origins)?;
        let mut hosts: Vec<String> = origins
            .iter()
            .filter_map(|origin| origin.strip_prefix("https://"))
            .map(str::to_owned)
            .collect();
        debug_assert_eq!(hosts.len(), origins.len(), "every public origin is https");
        // A browser leaves the default port out of both headers.
        let port = match bound.port() {
            80 => String::new(),
            port => format!(":{port}"),
        };
        for host in LOOPBACK_HOSTS {
            origins.push(format!("http://{host}{port}"));
            hosts.push(format!("{host}{port}"));
        }
        Ok(Self { origins, hosts })
    }

    /// Whether a `Host` header names this listener. Stops DNS rebinding.
    pub fn allows_host(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|known| known.eq_ignore_ascii_case(host))
    }

    /// Whether an `Origin` header is one of ours. Stops cross-site WebSocket
    /// hijacking and cross-site posts.
    pub fn allows_origin(&self, origin: &str) -> bool {
        self.origins.iter().any(|known| known == origin)
    }

    pub fn origins(&self) -> &[String] {
        &self.origins
    }

    /// The origin a pairing URL uses: the first public one, else loopback.
    pub fn primary(&self) -> Option<&str> {
        self.origins.first().map(String::as_str)
    }
}

/// The `Content-Security-Policy` of every response. `'self'` covers the socket's `ws:`/`wss:`
/// forms, so no host is spelled out (an IPv6 literal is not a valid source).
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; \
    style-src 'self'; img-src 'self' data:; font-src 'self'; manifest-src 'self'; \
    worker-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; \
    frame-ancestors 'none'";

/// Headers on every response besides the CSP.
pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    (
        "permissions-policy",
        "camera=(), geolocation=(), microphone=(self)",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_take_one_method_each() {
        assert_eq!(route(&Method::GET, HEALTH), Ok(Route::Health));
        assert_eq!(route(&Method::GET, WEBSOCKET), Ok(Route::WebSocket));
        assert_eq!(route(&Method::POST, PAIR_CLAIM), Ok(Route::PairClaim));
        assert_eq!(route(&Method::POST, PAIR_STATUS), Ok(Route::PairStatus));
        for (method, path) in [
            (Method::GET, PAIR_CLAIM),
            (Method::POST, HEALTH),
            (Method::POST, "/index.html"),
            (Method::DELETE, WEBSOCKET),
        ] {
            assert_eq!(
                route(&method, path),
                Err(HttpErrorCode::MethodNotAllowed),
                "{method} {path}"
            );
        }
        assert_eq!(
            route(&Method::GET, "/api/nope"),
            Err(HttpErrorCode::NotFound)
        );
    }

    #[test]
    fn asset_paths_cannot_leave_the_table() {
        assert_eq!(
            route(&Method::GET, "/app/main.js"),
            Ok(Route::Asset("/app/main.js".into()))
        );
        assert_eq!(route(&Method::HEAD, "/"), Ok(Route::Asset("/".into())));
        for bad in [
            "",
            "relative",
            "/a/../b",
            "/a//b",
            "/./a",
            "/a%2e%2e/b",
            "/a\\b",
        ] {
            assert_eq!(
                route(&Method::GET, bad),
                Err(HttpErrorCode::BadRequest),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn head_reads_assets_and_nothing_else() {
        assert_eq!(
            route(&Method::HEAD, "/index.html"),
            Ok(Route::Asset("/index.html".into()))
        );
        for endpoint in [HEALTH, WEBSOCKET, PAIR_CLAIM] {
            assert_eq!(
                route(&Method::HEAD, endpoint),
                Err(HttpErrorCode::MethodNotAllowed),
                "{endpoint}"
            );
        }
        assert_eq!(allowed_methods(PAIR_STATUS), ["POST"]);
        assert_eq!(allowed_methods("/pair"), ["GET", "HEAD"]);
    }

    #[test]
    fn a_public_origin_is_https_and_exactly_as_a_browser_writes_it() {
        for good in [
            "https://box.tail1234.ts.net",
            "https://box.example:8443",
            "https://192.0.2.7",
        ] {
            assert_eq!(public_origin(good), Ok(good.to_owned()), "{good}");
        }
        for bad in [
            "http://box.example",
            "https://box.example/",
            "https://box.example/app",
            "https://box.example?x=1",
            "https://user@box.example",
            "https://BOX.example",
            "https://box.example:443",
            "box.example",
            "",
            "null",
            "https://",
        ] {
            assert!(public_origin(bad).is_err(), "{bad:?}");
        }
        let hint = public_origin("https://BOX.example:443").expect_err("not canonical");
        assert!(hint.contains("`https://box.example`"), "{hint}");
    }

    #[test]
    fn public_origins_are_deduplicated_and_capped() {
        let twice = vec!["https://a.example".to_owned(), "https://a.example".into()];
        assert_eq!(public_origins(&twice), Ok(vec!["https://a.example".into()]));
        let many: Vec<String> = (0..=MAX_PUBLIC_ORIGINS)
            .map(|n| format!("https://h{n}.example"))
            .collect();
        assert!(public_origins(&many).is_err());
        assert!(public_origins(&many[..MAX_PUBLIC_ORIGINS]).is_ok());
    }

    #[test]
    fn the_allowlist_is_the_public_origins_and_loopback_on_the_bound_port() {
        let bound: SocketAddr = "127.0.0.1:8377".parse().expect("addr");
        let policy =
            OriginPolicy::new(&["https://box.tail.ts.net".into()], bound).expect("valid origins");
        assert_eq!(policy.primary(), Some("https://box.tail.ts.net"));
        for host in [
            "box.tail.ts.net",
            "127.0.0.1:8377",
            "localhost:8377",
            "LOCALHOST:8377",
            "[::1]:8377",
        ] {
            assert!(policy.allows_host(host), "{host}");
        }
        for host in [
            "evil.example",
            "127.0.0.1",
            "127.0.0.1:9999",
            "localhost",
            "box.tail.ts.net.evil.example",
            "box.tail.ts.net:8377",
            "",
        ] {
            assert!(!policy.allows_host(host), "{host}");
        }
        for origin in [
            "https://box.tail.ts.net",
            "http://127.0.0.1:8377",
            "http://localhost:8377",
            "http://[::1]:8377",
        ] {
            assert!(policy.allows_origin(origin), "{origin}");
        }
        for origin in [
            "http://box.tail.ts.net",
            "https://BOX.tail.ts.net",
            "https://box.tail.ts.net/",
            "https://127.0.0.1:8377",
            "http://localhost:9999",
            "null",
            "",
        ] {
            assert!(!policy.allows_origin(origin), "{origin}");
        }
        let loopback_only = OriginPolicy::new(&[], bound).expect("no origins is allowed");
        assert_eq!(loopback_only.primary(), Some("http://127.0.0.1:8377"));
        let default_port =
            OriginPolicy::new(&[], "127.0.0.1:80".parse().expect("addr")).expect("port 80");
        assert!(default_port.allows_host("localhost"));
        assert!(default_port.allows_origin("http://localhost"));
        assert!(OriginPolicy::new(&["http://plain.example".into()], bound).is_err());
    }

    #[test]
    fn only_state_changing_routes_need_an_origin() {
        assert!(Route::WebSocket.needs_origin());
        assert!(Route::PairClaim.needs_origin());
        assert!(!Route::Health.needs_origin());
        assert!(!Route::Asset("/".into()).needs_origin());
    }

    #[test]
    fn the_csp_allows_only_this_origin_and_names_no_host() {
        let csp = CONTENT_SECURITY_POLICY;
        let directives: Vec<Vec<&str>> = csp
            .split(';')
            .map(|directive| directive.split_whitespace().collect())
            .collect();
        assert_eq!(directives[0], ["default-src", "'none'"], "{csp}");
        assert!(directives.contains(&vec!["connect-src", "'self'"]), "{csp}");
        // Keywords and one scheme: a host, above all `[::1]`, is never a source.
        for directive in &directives {
            let (name, sources) = directive.split_first().expect("a directive has a name");
            assert!(name.ends_with("-src") || name.contains('-'), "{name}");
            assert!(
                !sources.is_empty(),
                "{name} allows something or says 'none'"
            );
            for source in sources {
                let known = ["'self'", "'none'", "data:"].contains(source);
                assert!(known, "{name} has the source {source}");
            }
        }
        assert!(!csp.contains("unsafe"), "{csp}");
    }

    #[test]
    fn a_public_origin_is_matched_whatever_the_case_of_its_host() {
        let bound = "127.0.0.1:8377".parse().expect("addr");
        let policy =
            OriginPolicy::new(&["https://box.tail.ts.net".into()], bound).expect("an origin");
        assert!(policy.allows_host("BOX.tail.ts.net"));
        assert!(!policy.allows_host("evil.example"));
        assert!(!policy.allows_origin("https://evil.example"));
        assert!(policy.allows_origin("http://[::1]:8377"));
    }
}
