//! The listener, pairing and devices: everything `aether remote` asks of
//! the local daemon. Host-only; refused with `--remote`.

use std::path::PathBuf;

use aether_client::Client;
use aether_protocol::remote::{Device, DeviceId, DeviceRole, RemoteSettings, RemoteStatus};
use aether_protocol::{Command, Reply};
use anyhow::bail;

use super::requests;
use crate::cli::{acted, connect, sane, short};

mod pair;
#[cfg(test)]
mod tests;

pub(super) use pair::pair;

/// Widest device name a table row shows.
const NAME_COLUMN: usize = 24;

/// What `aether remote enable` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Enable {
    /// Replaces the stored public origins when not empty.
    pub(super) origins: Vec<String>,
    pub(super) pairing: bool,
    /// `None` keeps the stored setting.
    pub(super) push: Option<bool>,
}

impl Enable {
    /// The settings to ask for, given the ones `stored` now.
    fn over(self, stored: RemoteSettings) -> RemoteSettings {
        let public_origins = if self.origins.is_empty() {
            stored.public_origins
        } else {
            self.origins
        };
        RemoteSettings {
            enabled: true,
            pairing: self.pairing,
            public_origins,
            push: self.push.unwrap_or(stored.push),
        }
    }
}

pub(super) async fn status(socket: PathBuf, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let status = requests::status(&client).await?;
    let devices = requests::devices(&client, false).await?;
    if json {
        let both = serde_json::json!({"status": status, "devices": devices});
        println!("{}", serde_json::to_string_pretty(&both)?);
        return Ok(());
    }
    print!("{}", describe(&status));
    if !devices.is_empty() {
        print!("\n{}", table(&devices));
    }
    Ok(())
}

/// The listener in one phrase: open where, closed, or failing why.
fn listener(status: &RemoteStatus) -> String {
    match (&status.listening, &status.error) {
        (Some(bound), _) => format!("enabled, listening on {}", sane(bound)),
        (None, Some(error)) => format!("enabled, not listening: {}", sane(error)),
        (None, None) if status.settings.enabled => "enabled, not listening".into(),
        (None, None) => "disabled".into(),
    }
}

/// `aether remote status` for a reader: one labelled line per fact.
fn describe(status: &RemoteStatus) -> String {
    let settings = &status.settings;
    let push = match (settings.push, status.push_previews) {
        (false, _) => "off",
        (true, false) => "on, without previews",
        (true, true) => "on, with previews",
    };
    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        out.push_str(&format!("{label:<12}{value}\n"));
    };
    line("remote", &listener(status));
    line("address", &sane(&status.address));
    line(
        "pairing",
        if settings.pairing {
            "allowed"
        } else {
            "closed"
        },
    );
    line("push", push);
    if settings.public_origins.is_empty() {
        line("origins", "none: reachable from this machine only");
    }
    for (index, origin) in settings.public_origins.iter().enumerate() {
        line(if index == 0 { "origins" } else { "" }, &sane(origin));
    }
    let devices = format!(
        "{} paired, {} connected",
        status.devices, status.connections
    );
    line("devices", &devices);
    out
}

pub(super) async fn enable(socket: PathBuf, enable: Enable, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let current = requests::status(&client).await?;
    let settings = enable.over(current.settings.clone());
    let status = requests::configure(&client, settings, current.revision).await?;
    if let Some(error) = &status.error {
        bail!("remote control is enabled, but its listener is not open: {error}");
    }
    let line = format!("remote control {}", listener(&status));
    acted(json, &line, serde_json::to_value(&status)?)
}

pub(super) async fn disable(socket: PathBuf, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let current = requests::status(&client).await?;
    let settings = RemoteSettings {
        enabled: false,
        ..current.settings.clone()
    };
    let status = requests::configure(&client, settings, current.revision).await?;
    anyhow::ensure!(
        status.listening.is_none(),
        "the daemon accepted the change and is still listening"
    );
    let line = format!("remote control {}", listener(&status));
    acted(json, &line, serde_json::to_value(&status)?)
}

pub(super) async fn devices(socket: PathBuf, all: bool, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let devices = requests::devices(&client, all).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
        return Ok(());
    }
    if devices.is_empty() {
        println!("no paired devices (pair one with `aether remote pair`)");
        return Ok(());
    }
    print!("{}", table(&devices));
    Ok(())
}

pub(super) fn role_label(role: DeviceRole) -> &'static str {
    match role {
        DeviceRole::Operator => "operator",
        DeviceRole::Observer => "observer",
    }
}

/// Where a device stands: revoked, connected now, or merely paired.
fn standing(device: &Device) -> String {
    let mut standing = match (device.revoked_at, device.connections) {
        (Some(at), _) => format!("revoked {}", at.format("%Y-%m-%d")),
        (None, 0) => "paired".to_owned(),
        (None, 1) => "connected".to_owned(),
        (None, many) => format!("connected ×{many}"),
    };
    if device.push {
        standing.push_str(" · push");
    }
    standing
}

/// Devices as rows, oldest first as the daemon lists them.
fn table(devices: &[Device]) -> String {
    let mut out = String::new();
    let mut row = |id: &str, name: &str, role: &str, seen: &str, standing: &str| {
        out.push_str(&format!(
            "{id:<30}  {name:<25}  {role:<8}  {seen:<23}  {standing}\n"
        ));
    };
    row("id", "name", "role", "last seen", "state");
    for device in devices {
        let seen = match device.last_seen_at {
            Some(at) => at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => "never".to_owned(),
        };
        row(
            &device.id.to_string(),
            &short(&device.name, NAME_COLUMN),
            role_label(device.role),
            &seen,
            &standing(device),
        );
    }
    out
}

/// The device `wanted` names: an id, or the name of exactly one paired device.
fn resolve(devices: &[Device], wanted: &str) -> anyhow::Result<DeviceId> {
    let named: Vec<&Device> = devices
        .iter()
        .filter(|device| device.revoked_at.is_none() && device.name == wanted)
        .collect();
    match named.as_slice() {
        [only] => Ok(only.id),
        [] => bail!("no paired device is named `{wanted}`; `aether remote devices` lists them"),
        several => {
            let ids: Vec<String> = several.iter().map(|device| device.id.to_string()).collect();
            bail!(
                "{} paired devices are named `{wanted}`; name one by id: {}",
                several.len(),
                ids.join(", ")
            )
        }
    }
}

async fn find(client: &Client, wanted: &str) -> anyhow::Result<DeviceId> {
    if let Ok(id) = wanted.parse() {
        return Ok(id);
    }
    resolve(&requests::devices(client, false).await?, wanted)
}

pub(super) async fn revoke(
    socket: PathBuf,
    device: String,
    reason: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let id = find(&client, &device).await?;
    let revoked = match client.request(Command::RevokeDevice { id, reason }).await? {
        Reply::Device(device) => device,
        other => return Err(requests::unexpected("revoke_device", &other)),
    };
    anyhow::ensure!(
        revoked.id == id && revoked.revoked_at.is_some(),
        "the daemon answered without revoking {id}"
    );
    let line = format!("revoked {} ({})", revoked.name, revoked.id);
    acted(json, &line, serde_json::to_value(&revoked)?)
}

pub(super) async fn rename(
    socket: PathBuf,
    device: String,
    name: String,
    json: bool,
) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let id = find(&client, &device).await?;
    let update = Command::UpdateDevice {
        id,
        name: Some(name),
        role: None,
    };
    let renamed = match client.request(update).await? {
        Reply::Device(device) => device,
        other => return Err(requests::unexpected("update_device", &other)),
    };
    anyhow::ensure!(renamed.id == id, "the daemon answered about another device");
    let line = format!("renamed {} to {}", renamed.id, renamed.name);
    acted(json, &line, serde_json::to_value(&renamed)?)
}
