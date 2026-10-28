//! Which providers can take a turn: those with credentials, and local
//! servers that need none but only count once they answer with a model.

use std::time::Duration;

use aether_core::model::ModelRef;
use aether_model::auth::NO_AUTH_SOURCE;
use aether_protocol::{AuthInfo, ProviderInfo};

use super::Check;

/// How long a local server gets to list its models. They are probed
/// together, so this bounds the whole probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// The most of a model list read; a local server's is a few kilobytes.
const MAX_MODEL_LIST_BYTES: usize = 1 << 20;

/// A provider that needs no credentials, and the base URL it serves on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServer {
    pub provider: String,
    pub base_url: String,
}

/// Why a local server cannot take a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Nothing answered within the probe's time.
    Unreachable,
    /// It answered with an empty model list: nothing is loaded or pulled.
    NoModels,
}

/// A local server that cannot take a turn, by `host:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotServing {
    pub provider: String,
    pub address: String,
    pub fault: Fault,
}

impl NotServing {
    /// The status `aether auth` and `aether providers` both show.
    pub fn label(&self) -> &'static str {
        match self.fault {
            Fault::Unreachable => "unreachable",
            Fault::NoModels => "no models",
        }
    }

    fn describe(&self) -> String {
        match self.fault {
            Fault::Unreachable => format!("{} not reachable at {}", self.provider, self.address),
            Fault::NoModels => format!("{} at {} serves no models", self.provider, self.address),
        }
    }
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

/// Ask every server for its models at once, and name those that did not
/// answer within `timeout` or listed none.
pub async fn servers_not_serving(
    servers: &[LocalServer],
    timeout: Duration,
) -> anyhow::Result<Vec<NotServing>> {
    assert!(
        !timeout.is_zero(),
        "a zero timeout reports every server down"
    );
    // Local servers: a configured HTTP proxy would answer for them.
    let client = reqwest::Client::builder().no_proxy().build()?;
    let probes = servers.iter().map(|server| probe(&client, server, timeout));
    let found: Vec<NotServing> = futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect();
    debug_assert!(found.len() <= servers.len());
    Ok(found)
}

async fn probe(
    client: &reqwest::Client,
    server: &LocalServer,
    timeout: Duration,
) -> Option<NotServing> {
    let not_serving = |address: String, fault| NotServing {
        provider: server.provider.clone(),
        address,
        fault,
    };
    let Some(address) = server_address(&server.base_url) else {
        return Some(not_serving(server.base_url.clone(), Fault::Unreachable));
    };
    let url = format!("{}/models", server.base_url.trim_end_matches('/'));
    match tokio::time::timeout(timeout, models_listed(client, &url)).await {
        Ok(Ok(Some(0))) => Some(not_serving(address, Fault::NoModels)),
        // A list it could not give or that did not parse may still mean a
        // server taking named models: only an empty one proves it idle.
        Ok(Ok(_)) => None,
        Ok(Err(_)) | Err(_) => Some(not_serving(address, Fault::Unreachable)),
    }
}

/// How many models an OpenAI-compatible `/models` lists; `None` when the
/// server answered with something else. Errs when nothing answered.
async fn models_listed(client: &reqwest::Client, url: &str) -> reqwest::Result<Option<usize>> {
    let mut response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_MODEL_LIST_BYTES {
            return Ok(None);
        }
    }
    let list: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(list) => list,
        Err(_) => return Ok(None),
    };
    Ok(list
        .get("data")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len))
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
/// none; `not_serving` are local servers that cannot take a turn.
pub fn check_providers(
    auth: &[AuthInfo],
    not_serving: &[NotServing],
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
    let is_down = |provider: &str| not_serving.iter().any(|u| u.provider == provider);
    let ready: Vec<&str> = auth
        .iter()
        .filter(|a| a.configured && !is_down(&a.provider))
        .map(|a| a.provider.as_str())
        .collect();
    let down = not_serving
        .iter()
        .map(NotServing::describe)
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
