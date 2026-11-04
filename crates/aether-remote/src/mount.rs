//! Listener path mounts: `/archon/*` HTTP and WebSocket traffic forwarded unchanged to a
//! loopback daemon, under the publication-status lease (vNext ARCHITECTURE.md §d).

mod forward;
mod lease;
pub(crate) mod leased;
pub(crate) mod tunnel;

use std::net::SocketAddr;

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

/// The loopback socket address of a validated `http://127.0.0.1:<port>` or
/// `http://[::1]:<port>` target; `None` for anything else.
pub(crate) fn target_address(target: &str) -> Option<SocketAddr> {
    let address: SocketAddr = target.strip_prefix("http://")?.parse().ok()?;
    let exact = format!("http://{address}") == target;
    (address.ip().is_loopback() && address.port() != 0 && exact).then_some(address)
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
