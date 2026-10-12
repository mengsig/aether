//! The remote administration commands as typed requests, one place that knows
//! each reply; `aether remote` and the setup wizard both send through here.

use aether_client::Client;
use aether_protocol::remote::{Device, DeviceRole, Pairing, RemoteSettings, RemoteStatus};
use aether_protocol::{Command, Reply};

/// A reply of another kind than the command has: the daemon and this
/// binary disagree about the protocol.
pub(super) fn unexpected(command: &str, reply: &Reply) -> anyhow::Error {
    let tag = serde_json::to_value(reply)
        .ok()
        .and_then(|v| v.get("reply").and_then(|r| r.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    anyhow::anyhow!("the daemon answered `{command}` with a `{tag}` reply")
}

pub(super) async fn status(client: &Client) -> anyhow::Result<RemoteStatus> {
    match client.request(Command::GetRemote).await? {
        Reply::Remote(status) => Ok(status),
        other => Err(unexpected("get_remote", &other)),
    }
}

/// Replace the settings read at `expected_revision`. Another change in
/// between is refused by the daemon rather than overwritten.
pub(super) async fn configure(
    client: &Client,
    settings: RemoteSettings,
    expected_revision: u64,
) -> anyhow::Result<RemoteStatus> {
    let command = Command::ConfigureRemote {
        settings,
        expected_revision,
    };
    match client.request(command).await? {
        Reply::Remote(status) => Ok(status),
        other => Err(unexpected("configure_remote", &other)),
    }
}

pub(super) async fn devices(client: &Client, include_revoked: bool) -> anyhow::Result<Vec<Device>> {
    match client
        .request(Command::ListDevices { include_revoked })
        .await?
    {
        Reply::Devices(devices) => Ok(devices),
        other => Err(unexpected("list_devices", &other)),
    }
}

async fn pairing(client: &Client, command: Command, name: &str) -> anyhow::Result<Pairing> {
    match client.request(command).await? {
        Reply::Pairing(pairing) => Ok(pairing),
        other => Err(unexpected(name, &other)),
    }
}

pub(super) async fn start_pairing(client: &Client, role: DeviceRole) -> anyhow::Result<Pairing> {
    pairing(client, Command::StartPairing { role }, "start_pairing").await
}

pub(super) async fn get_pairing(client: &Client, id: &str) -> anyhow::Result<Pairing> {
    let command = Command::GetPairing { id: id.to_owned() };
    pairing(client, command, "get_pairing").await
}

pub(super) async fn confirm_pairing(
    client: &Client,
    id: &str,
    accept: bool,
) -> anyhow::Result<Pairing> {
    let command = Command::ConfirmPairing {
        id: id.to_owned(),
        accept,
    };
    pairing(client, command, "confirm_pairing").await
}

pub(super) async fn cancel_pairing(client: &Client, id: &str) -> anyhow::Result<Pairing> {
    let command = Command::CancelPairing { id: id.to_owned() };
    pairing(client, command, "cancel_pairing").await
}
