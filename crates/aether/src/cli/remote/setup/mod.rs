//! `aether remote setup`: the guided path to a paired phone. Each step skips what
//! is right and repairs the rest; every effect is behind a trait so a fake tests it.

use std::path::PathBuf;

use anyhow::bail;

mod daemon;
mod environment;
mod guidance;
mod pairing;
mod platform;
mod report;
mod runner;
mod steps;
mod summary;
mod tailscale;
mod ui;
mod wizard;

#[cfg(test)]
mod tests;

use daemon::SocketDaemon;
use environment::System;
use report::{Mode, Report, Status};
use runner::SystemRunner;
use ui::{Silent, Terminal, Ui};
use wizard::Wizard;

/// What `aether remote setup` was asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Options {
    /// Accept every default and never wait for a person.
    pub(super) yes: bool,
    pub(super) json: bool,
    /// An `https://` address that already reaches the listener; skips Tailscale.
    pub(super) proxy_url: Option<String>,
    /// Publish on the public internet with Tailscale Funnel.
    pub(super) funnel: bool,
}

pub(super) async fn setup(socket: PathBuf, options: Options) -> anyhow::Result<()> {
    let terminal = Terminal::new();
    let mode = choose_mode(&options, terminal.can_ask())?;
    let proxy_origin = options.proxy_url.as_deref().map(origin_of).transpose()?;
    let mut daemon = SocketDaemon::new(socket);
    let report = if options.json {
        drive(&options, mode, proxy_origin, &mut daemon, Silent).await?
    } else {
        drive(&options, mode, proxy_origin, &mut daemon, terminal).await?
    };
    if options.json {
        println!("{}", report.to_json()?);
        return Ok(());
    }
    match report.status {
        Status::Blocked => bail!("setup is not finished; see the steps above"),
        Status::Complete | Status::Stopped | Status::Pending => Ok(()),
    }
}

async fn drive<U: Ui>(
    options: &Options,
    mode: Mode,
    proxy_origin: Option<String>,
    daemon: &mut SocketDaemon,
    mut ui: U,
) -> anyhow::Result<Report> {
    let (runner, env) = (SystemRunner, System::new());
    Wizard::new(options, mode, proxy_origin, &runner, daemon, &env, &mut ui)
        .run()
        .await
}

/// Who answers the questions. A run with nobody to ask must say so up front.
fn choose_mode(options: &Options, terminal: bool) -> anyhow::Result<Mode> {
    if options.yes {
        return Ok(Mode::Yes);
    }
    if options.json {
        return Ok(Mode::Check);
    }
    if !terminal {
        bail!(
            "`aether remote setup` asks questions and needs a terminal; \
             run it in one, or pass --yes to accept every default, or --json to only look"
        );
    }
    Ok(Mode::Interactive)
}

/// `url` as an origin: `https://host[:port]`, nothing after it.
pub(super) fn origin_of(url: &str) -> anyhow::Result<String> {
    let Some(rest) = url.strip_prefix("https://") else {
        bail!(
            "--proxy-url must start with https:// (got `{url}`); browsers only run Aether over HTTPS"
        );
    };
    if rest.ends_with('/') && !rest[..rest.len() - 1].contains('/') {
        bail!(
            "--proxy-url must not end with `/`; use `{}`",
            url.trim_end_matches('/')
        );
    }
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    let host_ok = !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !host_ok {
        bail!(
            "--proxy-url `{url}` must be https://host or https://host:port, with no path, query or login"
        );
    }
    let port_ok = |port: &str| {
        port.parse::<u16>()
            .is_ok_and(|p| p != 0 && port == p.to_string())
    };
    if port.is_some_and(|port| !port_ok(port)) {
        bail!("--proxy-url `{url}` has a port that is not a number from 1 to 65535");
    }
    // A browser leaves the default port out of `Origin`, and the daemon
    // compares origins byte for byte.
    let authority = match port {
        Some("443") => host,
        Some(_) | None => rest,
    };
    Ok(format!("https://{}", authority.to_ascii_lowercase()))
}
