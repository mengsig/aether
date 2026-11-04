//! Listener path mounts: `/archon/*` HTTP and WebSocket traffic forwarded unchanged to a
//! loopback daemon, under the publication-status lease (vNext ARCHITECTURE.md §d).

mod forward;
mod lease;
pub(crate) mod leased;
pub(crate) mod tunnel;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use aether_protocol::remote::RemoteMount;
use hyper::HeaderMap;
use hyper::header::{CONNECTION, HeaderName};

pub(crate) use forward::{Failure, Forwarded, forward};
pub use lease::{Blocked, LEASE_AGE, REFRESH_EVERY};
pub(crate) use lease::{Lease, Ticket};
pub(crate) use tunnel::{Tunnel, carry};

/// How the host's listener is published, as one status read saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publication {
    /// Not on the public internet: private to the tailnet, or not published.
    Private,
    /// Published through Tailscale Funnel.
    Funnel,
}

/// Headers that belong to one hop and are never forwarded (RFC 9110 §7.6.1).
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Everything below it is the listener's own: `/api/health`, pairing, and what may follow.
const API_ROOT: &str = "/api";

/// A request that falls under a mount: where it goes, and the path it goes with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mounted {
    pub target: SocketAddr,
    /// The path with the prefix stripped, and the query as it came.
    pub path_and_query: String,
}

/// Why a path under a mount is not forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BadPath;

/// The mount `path` falls under, if any. A path under a mount that is not a clean absolute
/// path (an empty, `.` or `..` segment, `%`, `\`) is refused rather than forwarded.
pub(crate) fn find(
    mounts: &[RemoteMount],
    path: &str,
    query: Option<&str>,
) -> Option<Result<Mounted, BadPath>> {
    let mount = mounts.iter().find(|mount| under(path, &mount.prefix))?;
    let rest = &path[mount.prefix.len()..];
    let rest = if rest.is_empty() { "/" } else { rest };
    if !crate::http::is_clean_path(rest) {
        return Some(Err(BadPath));
    }
    let Some(target) = target_address(&mount.target) else {
        tracing::error!(target = %mount.target, "a mount target that is not loopback reached the listener");
        return Some(Err(BadPath));
    };
    let path_and_query = match query {
        Some(query) => format!("{rest}?{query}"),
        None => rest.to_owned(),
    };
    debug_assert!(
        path_and_query.starts_with('/'),
        "origin-form starts with a slash"
    );
    Some(Ok(Mounted {
        target,
        path_and_query,
    }))
}

/// Whether `path` is the prefix itself or below it, never merely starting with its letters.
fn under(path: &str, prefix: &str) -> bool {
    debug_assert!(
        prefix.starts_with('/') && !prefix.ends_with('/'),
        "{prefix}"
    );
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The loopback socket address of a target `validate_mounts` accepts, `http://127.0.0.1:<port>`
/// or `http://[::1]:<port>` with a decimal port (zero-padded too); `None` for anything else.
pub(crate) fn target_address(target: &str) -> Option<SocketAddr> {
    let (ip, port) = match target.strip_prefix("http://127.0.0.1:") {
        Some(port) => (IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        None => (
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            target.strip_prefix("http://[::1]:")?,
        ),
    };
    // Digits only, as the validator reads them: `u16::from_str` would also take a `+`.
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    let address = SocketAddr::new(ip, port);
    debug_assert!(address.ip().is_loopback(), "only loopback is built");
    Some(address)
}

/// Whether the listener answers paths under `prefix` itself: its endpoints, or an embedded
/// file. A mount there would shadow them, so none may be made.
pub fn is_served_here(prefix: &str) -> bool {
    debug_assert!(prefix.starts_with('/'), "{prefix}");
    let own = [crate::http::WEBSOCKET, API_ROOT];
    own.iter()
        .any(|path| under(path, prefix) || under(prefix, path))
        || crate::assets::ASSETS
            .iter()
            .any(|(path, _)| under(path, prefix))
}

/// Whether `path` is one of the listener's own endpoints, which no mount may take over.
pub(crate) fn is_endpoint(path: &str) -> bool {
    path == crate::http::WEBSOCKET || under(path, API_ROOT)
}

/// The headers of one hop, without those that belong to it alone: the fixed set, and every
/// header the `Connection` header names.
pub(crate) fn end_to_end(headers: &HeaderMap) -> HeaderMap {
    let named: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();
    let mut kept = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        let hop = HOP_BY_HOP.contains(&name.as_str()) || named.contains(name);
        if !hop {
            kept.append(name.clone(), value.clone());
        }
    }
    debug_assert!(kept.len() <= headers.len(), "nothing is added");
    debug_assert!(
        !kept.contains_key(CONNECTION),
        "the hop's own header stays behind"
    );
    kept
}

#[cfg(test)]
pub(crate) mod tests;
