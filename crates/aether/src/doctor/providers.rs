//! Which providers can take a turn: those with credentials, and local
//! servers that need none but only count once something listens there.

use std::time::Duration;

use aether_core::model::ModelRef;
use aether_model::auth::NO_AUTH_SOURCE;
use aether_protocol::{AuthInfo, ProviderInfo};

use super::Check;

/// How long a local server gets to accept a connection. They are probed
/// together, so this bounds the whole probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// A provider that needs no credentials, and the base URL it serves on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServer {
    pub provider: String,
    pub base_url: String,
}

/// A local server that did not accept a connection, by `host:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreachable {
    pub provider: String,
    pub address: String,
}

/// The providers in `auth` that are ready only because they need no
/// credentials, with the base URL the daemon's `providers` reply gives them.
pub fn local_servers(auth: &[AuthInfo], providers: &[ProviderInfo]) -> Vec<LocalServer> {
    auth.iter()
        .filter(|a| a.configured && a.source.as_deref() == Some(NO_AUTH_SOURCE))
        .filter_map(|a| {
            let info = providers.iter().find(|p| p.provider == a.provider)?;
            Some(LocalServer {
                provider: a.provider.clone(),
                base_url: info.base_url.clone(),
            })
        })
        .collect()
}

/// Connect to every server at once, and name those that refused or did not
/// answer within `timeout`.
pub async fn unreachable_servers(servers: &[LocalServer], timeout: Duration) -> Vec<Unreachable> {
    assert!(
        !timeout.is_zero(),
        "a zero timeout reports every server down"
    );
    let probes = servers.iter().map(|server| async move {
        let Some(address) = server_address(&server.base_url) else {
            return Some(Unreachable {
                provider: server.provider.clone(),
                address: server.base_url.clone(),
            });
        };
        let connect = tokio::net::TcpStream::connect(address.as_str());
        let answered = matches!(tokio::time::timeout(timeout, connect).await, Ok(Ok(_)));
        (!answered).then(|| Unreachable {
            provider: server.provider.clone(),
            address,
        })
    });
    let found: Vec<Unreachable> = futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect();
    debug_assert!(found.len() <= servers.len());
    found
}

/// `host:port` of a base URL, the port defaulted from its scheme; `None`
/// when it names no host.
fn server_address(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    Some(format!("{host}:{port}"))
}

/// At least one provider can take a turn, and the daemon's default model is
/// on one of them. `default_model` is the daemon's pick, `None` when it has
/// none; `unreachable` are local servers nothing answered at.
pub fn check_providers(
    auth: &[AuthInfo],
    unreachable: &[Unreachable],
    default_model: Option<&ModelRef>,
    command: &str,
) -> Check {
    if auth.is_empty() {
        return Check::warn(
            "providers",
            "no providers known",
            format!("run `{command} auth` against a running daemon to see them"),
        );
    }
    let is_down = |provider: &str| unreachable.iter().any(|u| u.provider == provider);
    let ready: Vec<&str> = auth
        .iter()
        .filter(|a| a.configured && !is_down(&a.provider))
        .map(|a| a.provider.as_str())
        .collect();
    let down = unreachable
        .iter()
        .map(|u| format!("{} not reachable at {}", u.provider, u.address))
        .collect::<Vec<_>>()
        .join(", ");
    let with_down = |detail: String| {
        if down.is_empty() {
            detail
        } else {
            format!("{detail} · {down}")
        }
    };
    if ready.is_empty() {
        return Check::fail(
            "providers",
            with_down(format!("none of {} providers are ready", auth.len())),
            format!(
                "`{command} login claude-code` for a subscription, or `{command} login anthropic` for a key"
            ),
        );
    }
    let summary = format!(
        "{} of {} ready: {}",
        ready.len(),
        auth.len(),
        ready.join(", ")
    );
    let Some(default) = default_model else {
        return Check::warn(
            "providers",
            with_down(format!("{summary} · no default model")),
            format!(
                "set `default` under [model] in config.toml, or name one with `{command} run --model`"
            ),
        );
    };
    if !ready.contains(&default.provider.as_str()) {
        return Check::fail(
            "providers",
            with_down(format!(
                "{summary} · the default model {default} is on `{}`, which is not ready",
                default.provider
            )),
            format!(
                "`{command} login {}`, start its server, or set `default` under [model] in config.toml to a ready provider's model",
                default.provider
            ),
        );
    }
    Check::ok(
        "providers",
        with_down(format!("{summary} · default {default}")),
    )
}
