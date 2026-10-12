//! The `tailscale` command line: what state the machine is in, what it
//! publishes, and publishing the daemon's listener.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, bail};
use serde::Deserialize;

use super::platform::Platform;
use super::runner::{CommandRunner, Output, RunError, display};

const QUICK: Duration = Duration::from_secs(5);
const STATUS: Duration = Duration::from_secs(15);
/// `serve` may wait on the control server for a certificate.
const PUBLISH: Duration = Duration::from_secs(60);

/// Ports `tailscale serve --https` accepts.
pub(super) const SERVE_PORTS: [u16; 3] = [443, 8443, 10000];

/// This machine's place on the tailnet once everything needed is in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Node {
    /// `box.tail1234.ts.net`, without the trailing dot.
    pub(super) dns_name: String,
}

/// How far from usable Tailscale is on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TailscaleState {
    NotInstalled,
    /// The program answers but its background service does not.
    NotRunning {
        detail: String,
    },
    Stopped,
    LoggedOut {
        login_url: Option<String>,
    },
    /// Signed in, waiting for a tailnet admin to approve this machine.
    NeedsApproval,
    Connecting,
    NoMagicDns,
    NoHttpsCerts,
    Ready(Node),
}

pub(super) struct Tailscale<'a, R> {
    runner: &'a R,
    program: &'static str,
}

impl<'a, R: CommandRunner> Tailscale<'a, R> {
    /// The first `tailscale` program of this platform that answers
    /// `version`, or `None` when none is installed.
    pub(super) async fn locate(runner: &'a R, platform: &Platform) -> anyhow::Result<Option<Self>> {
        for &program in platform.tailscale_programs() {
            match runner.run(program, &["version"], QUICK).await {
                Ok(_) => return Ok(Some(Self { runner, program })),
                Err(RunError::NotFound) => {}
                Err(error) => {
                    bail!("`{}`: {error}", display(program, &["version"]))
                }
            }
        }
        Ok(None)
    }

    pub(super) async fn state(&self) -> anyhow::Result<TailscaleState> {
        let args = ["status", "--json"];
        let output = self
            .runner
            .run(self.program, &args, STATUS)
            .await
            .with_context(|| format!("running `{}`", display(self.program, &args)))?;
        if !output.success() {
            return Ok(TailscaleState::NotRunning {
                detail: format!(
                    "`{}` failed: {}",
                    display(self.program, &args),
                    output.complaint()
                ),
            });
        }
        parse_state(&output.stdout)
            .with_context(|| format!("reading the output of `{}`", display(self.program, &args)))
    }

    pub(super) async fn serve_config(&self) -> anyhow::Result<ServeConfig> {
        let args = ["serve", "status", "--json"];
        let output = self
            .runner
            .run(self.program, &args, STATUS)
            .await
            .with_context(|| format!("running `{}`", display(self.program, &args)))?;
        if !output.success() {
            bail!(
                "`{}` failed: {}",
                display(self.program, &args),
                output.complaint()
            );
        }
        parse_serve(&output.stdout)
            .with_context(|| format!("reading the output of `{}`", display(self.program, &args)))
    }

    /// Publishes `target` on this machine's HTTPS `port`, privately unless
    /// `funnel`, and returns what Tailscale made of it.
    pub(super) async fn publish(
        &self,
        port: u16,
        target: &str,
        funnel: bool,
    ) -> anyhow::Result<PublishOutcome> {
        assert!(SERVE_PORTS.contains(&port), "port {port} is not servable");
        assert!(
            target.starts_with("http://"),
            "target {target} is not plain http"
        );
        let https = format!("--https={port}");
        let command = if funnel { "funnel" } else { "serve" };
        let args = [command, "--bg", https.as_str(), target];
        let command = display(self.program, &args);
        match self.runner.run(self.program, &args, PUBLISH).await {
            Ok(output) => Ok(classify_publish(&output, &command)),
            Err(RunError::TimedOut { limit, partial }) => timed_out(&partial, &command, limit),
            Err(error) => Err(error).with_context(|| format!("running `{command}`")),
        }
    }
}

/// `serve` and `funnel` wait, printing the page that enables the feature,
/// until someone does: what they printed before being stopped says why.
fn timed_out(partial: &Output, command: &str, limit: Duration) -> anyhow::Result<PublishOutcome> {
    match classify_publish(partial, command) {
        outcome @ (PublishOutcome::NotEnabled { .. } | PublishOutcome::NotPermitted { .. }) => {
            Ok(outcome)
        }
        _ => bail!(
            "`{command}` did not finish within {}s; it said: {}",
            limit.as_secs(),
            partial.complaint()
        ),
    }
}

/// What `tailscale serve` or `funnel` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PublishOutcome {
    Published,
    /// This user may not change Tailscale's settings (Linux operator).
    NotPermitted {
        command: String,
    },
    /// A feature is off for the tailnet; the page that turns it on.
    NotEnabled {
        url: Option<String>,
        command: String,
    },
    Failed {
        command: String,
        complaint: String,
    },
}

fn classify_publish(output: &Output, command: &str) -> PublishOutcome {
    if output.success() {
        return PublishOutcome::Published;
    }
    let text = format!("{}\n{}", output.stderr, output.stdout);
    let lower = text.to_ascii_lowercase();
    let command = command.to_owned();
    if lower.contains("access denied") || lower.contains("--operator") {
        return PublishOutcome::NotPermitted { command };
    }
    if lower.contains("not enabled") || lower.contains("to enable") {
        return PublishOutcome::NotEnabled {
            url: first_tailscale_url(&text),
            command,
        };
    }
    PublishOutcome::Failed {
        command,
        complaint: output.complaint(),
    }
}

/// The first `https://….tailscale.com/…` address in `text`.
fn first_tailscale_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/'))
        .find(|word| word.starts_with("https://") && word.contains(".tailscale.com/"))
        .map(str::to_owned)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct RawStatus {
    backend_state: String,
    #[serde(rename = "AuthURL")]
    auth_url: String,
    #[serde(rename = "Self")]
    me: Option<RawSelf>,
    current_tailnet: Option<RawTailnet>,
    cert_domains: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawSelf {
    #[serde(rename = "DNSName")]
    dns_name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTailnet {
    #[serde(rename = "MagicDNSEnabled")]
    magic_dns_enabled: bool,
}

/// `tailscale status --json`, read for the one question the wizard asks:
/// can this machine be reached by name over HTTPS.
pub(super) fn parse_state(json: &str) -> anyhow::Result<TailscaleState> {
    let raw: RawStatus = serde_json::from_str(json).context("not the JSON that was expected")?;
    let state = match raw.backend_state.as_str() {
        "Running" => return running_state(&raw),
        "Stopped" => TailscaleState::Stopped,
        "NeedsLogin" => TailscaleState::LoggedOut {
            login_url: Some(raw.auth_url).filter(|url| !url.is_empty()),
        },
        "NeedsMachineAuth" => TailscaleState::NeedsApproval,
        "Starting" | "NoState" => TailscaleState::Connecting,
        other => TailscaleState::NotRunning {
            detail: format!("Tailscale reports the state `{other}`"),
        },
    };
    Ok(state)
}

fn running_state(raw: &RawStatus) -> anyhow::Result<TailscaleState> {
    let dns_name = raw
        .me
        .as_ref()
        .map(|me| me.dns_name.trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_default();
    let magic_dns = raw
        .current_tailnet
        .as_ref()
        .is_none_or(|t| t.magic_dns_enabled)
        && !dns_name.is_empty();
    if !magic_dns {
        return Ok(TailscaleState::NoMagicDns);
    }
    let certs = raw.cert_domains.as_deref().unwrap_or_default();
    if !certs.iter().any(|d| d.eq_ignore_ascii_case(&dns_name)) {
        return Ok(TailscaleState::NoHttpsCerts);
    }
    assert!(
        !dns_name.ends_with('.') && !dns_name.contains(char::is_whitespace),
        "`{dns_name}` is not a usable host name"
    );
    assert!(certs.len() <= 64, "implausibly many certificate domains");
    Ok(TailscaleState::Ready(Node { dns_name }))
}

/// What this machine already publishes with `tailscale serve`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ServeConfig {
    /// `host:port` → the target of its `/` handler, when it has one.
    roots: BTreeMap<String, Option<String>>,
    tcp_ports: Vec<u16>,
    funnel: Vec<String>,
    /// Parts of the status this wizard does not know how to read.
    unrecognized: Vec<String>,
}

/// What stands on one HTTPS port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PortUse {
    Free,
    /// Already publishing the target; `funnel` when open to the internet.
    Ours {
        funnel: bool,
    },
    /// Something else; what it forwards to.
    Taken(String),
    /// The status has parts this wizard cannot read, so the port may be in
    /// use; what it could not read.
    Unknown(String),
}

/// One scope of the serve configuration. The top level is the background
/// one; each `Foreground` session and each `Services` entry has its own.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawServe {
    #[serde(rename = "TCP")]
    tcp: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "Web")]
    web: BTreeMap<String, RawWeb>,
    #[serde(rename = "AllowFunnel")]
    allow_funnel: BTreeMap<String, bool>,
    /// Sessions of `tailscale serve` run without `--bg`, by session id.
    #[serde(rename = "Foreground")]
    foreground: BTreeMap<String, RawServe>,
    /// Tailscale Services, which answer on their own names.
    #[serde(rename = "Services")]
    services: BTreeMap<String, RawServe>,
    #[serde(rename = "ETag")]
    _etag: Option<String>,
    #[serde(flatten)]
    other: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWeb {
    #[serde(rename = "Handlers")]
    handlers: BTreeMap<String, RawHandler>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawHandler {
    #[serde(rename = "Proxy")]
    proxy: Option<String>,
    #[serde(rename = "Path")]
    path: Option<String>,
    #[serde(rename = "Text")]
    text: Option<String>,
}

/// `tailscale serve status --json`. No configuration is `{}` or nothing.
pub(super) fn parse_serve(json: &str) -> anyhow::Result<ServeConfig> {
    if json.trim().is_empty() {
        return Ok(ServeConfig::default());
    }
    let raw: RawServe = serde_json::from_str(json).context("not the JSON that was expected")?;
    let mut config = ServeConfig::default();
    config.absorb(raw, Scope::Node);
    Ok(config)
}

/// Whose ports a part of the configuration describes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// This machine's own name: the background configuration and every
    /// foreground session.
    Node,
    /// A Tailscale Service, which has its own address; only the web hosts it
    /// names can collide with ours.
    Service,
}

impl ServeConfig {
    fn absorb(&mut self, raw: RawServe, scope: Scope) {
        for (key, web) in raw.web {
            let root = web.handlers.get("/").map(describe_handler);
            let slot = self.roots.entry(key.to_ascii_lowercase()).or_default();
            if slot.is_none() {
                *slot = root;
            }
        }
        if scope == Scope::Node {
            self.tcp_ports
                .extend(raw.tcp.keys().filter_map(|p| p.parse::<u16>().ok()));
        }
        self.funnel.extend(
            raw.allow_funnel
                .into_iter()
                .filter_map(|(key, on)| on.then(|| key.to_ascii_lowercase())),
        );
        if scope == Scope::Node {
            let keys = raw.other.into_keys();
            self.unrecognized.extend(keys.map(|key| format!("`{key}`")));
        }
        for session in raw.foreground.into_values() {
            self.absorb(session, scope);
        }
        for service in raw.services.into_values() {
            self.absorb(service, Scope::Service);
        }
    }
}

fn describe_handler(handler: &RawHandler) -> String {
    match (&handler.proxy, &handler.path, &handler.text) {
        (Some(proxy), _, _) => proxy.clone(),
        (None, Some(path), _) => format!("files at {path}"),
        (None, None, Some(_)) => "a fixed text".to_owned(),
        (None, None, None) => "something else".to_owned(),
    }
}

/// A forwarding target compared without regard to `localhost` or a trailing `/`.
fn same_target(a: &str, b: &str) -> bool {
    let normal = |t: &str| {
        t.trim_end_matches('/')
            .to_ascii_lowercase()
            .replace("localhost", "127.0.0.1")
    };
    normal(a) == normal(b)
}

impl ServeConfig {
    /// What `host`'s HTTPS `port` is doing, given that `target` is the
    /// address the wizard wants served.
    pub(super) fn port_use(&self, host: &str, port: u16, target: &str) -> PortUse {
        let key = format!("{}:{port}", host.to_ascii_lowercase());
        match self.roots.get(&key) {
            Some(Some(existing)) if same_target(existing, target) => PortUse::Ours {
                funnel: self.funnel.contains(&key),
            },
            Some(Some(existing)) => PortUse::Taken(existing.clone()),
            Some(None) => PortUse::Taken("other paths of its own".to_owned()),
            None if self.tcp_ports.contains(&port) => PortUse::Taken("a TCP forward".to_owned()),
            None if !self.unrecognized.is_empty() => PortUse::Unknown(self.unrecognized.join(", ")),
            None => PortUse::Free,
        }
    }
}
