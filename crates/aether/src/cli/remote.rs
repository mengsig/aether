//! `aether remote`: remote control of this daemon, and SSH remotes.

use std::path::PathBuf;

use crate::args::RemoteCmd;

/// iOS gives a home-screen app its own storage, so it must be the one that pairs.
const IPHONE_FIRST: &str = "iPhone or iPad: add Aether to the Home Screen first (the page shows how) \
and type the code in that app.\nIt keeps its own pairing, so one made in Safari does not carry over.";

mod admin;
mod qr;
mod requests;
mod setup;
mod ssh;

pub(crate) use ssh::{check_selectors, endpoint};

pub(crate) async fn run(socket: PathBuf, command: RemoteCmd) -> anyhow::Result<()> {
    match command {
        RemoteCmd::Setup {
            yes,
            json,
            proxy_url,
            funnel,
        } => {
            let options = setup::Options {
                yes,
                json,
                proxy_url,
                funnel,
            };
            setup::setup(socket, options).await
        }
        RemoteCmd::Status { json } => admin::status(socket, json).await,
        RemoteCmd::Enable {
            origins,
            no_pairing,
            push,
            no_push,
            json,
        } => {
            let enable = admin::Enable {
                origins,
                pairing: !no_pairing,
                push: crate::flag_pair(push, no_push),
            };
            admin::enable(socket, enable, json).await
        }
        RemoteCmd::Disable { json } => admin::disable(socket, json).await,
        RemoteCmd::Pair { role } => admin::pair(socket, role.into()).await,
        RemoteCmd::Devices { all, json } => admin::devices(socket, all, json).await,
        RemoteCmd::Revoke {
            device,
            reason,
            json,
        } => admin::revoke(socket, device, reason, json).await,
        RemoteCmd::Rename { device, name, json } => admin::rename(socket, device, name, json).await,
        RemoteCmd::Add {
            name,
            ssh,
            executable,
            remote_socket,
            json,
        } => ssh::add(name, ssh, executable, remote_socket, json).await,
        RemoteCmd::List { json } => ssh::list(json),
        RemoteCmd::Remove { name, json } => ssh::remove(name, json),
    }
}
