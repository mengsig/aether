//! The daemon operations the wizard needs: the same protocol commands
//! `aether remote enable` and `aether remote pair` send.

use std::path::PathBuf;

use aether_client::{Client, ClientError};
use aether_protocol::remote::{Device, DeviceRole, Pairing, RemoteSettings, RemoteStatus};
use anyhow::Context as _;

use crate::cli::remote::requests;

pub(super) trait Daemon {
    /// Whether a daemon is answering. A daemon that refuses this build is an
    /// error, not an absence.
    async fn connect(&mut self) -> anyhow::Result<bool>;

    /// Starts a daemon and connects to it.
    async fn start(&mut self) -> anyhow::Result<()>;

    async fn remote(&mut self) -> anyhow::Result<RemoteStatus>;

    /// Saves `settings` if the revision is still `expected_revision`.
    async fn configure(
        &mut self,
        settings: RemoteSettings,
        expected_revision: u64,
    ) -> anyhow::Result<RemoteStatus>;

    async fn start_pairing(&mut self, role: DeviceRole) -> anyhow::Result<Pairing>;
    async fn pairing(&mut self, id: &str) -> anyhow::Result<Pairing>;
    async fn confirm_pairing(&mut self, id: &str, accept: bool) -> anyhow::Result<Pairing>;
    async fn cancel_pairing(&mut self, id: &str) -> anyhow::Result<Pairing>;
    async fn devices(&mut self) -> anyhow::Result<Vec<Device>>;
}

/// The daemon behind a Unix socket.
#[derive(Debug)]
pub(super) struct SocketDaemon {
    socket: PathBuf,
    client: Option<Client>,
}

impl SocketDaemon {
    pub(super) fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            client: None,
        }
    }

    fn client(&self, what: &str) -> anyhow::Result<&Client> {
        self.client
            .as_ref()
            .with_context(|| format!("not connected to the daemon (needed to {what})"))
    }
}

/// Say what the wizard was asking for when the daemon refused or failed.
fn asked<T>(what: &str, answer: anyhow::Result<T>) -> anyhow::Result<T> {
    answer.with_context(|| format!("asking the daemon to {what}"))
}

impl Daemon for SocketDaemon {
    async fn connect(&mut self) -> anyhow::Result<bool> {
        match Client::connect(&self.socket).await {
            Ok(client) => {
                self.client = Some(client);
                Ok(true)
            }
            Err(error) if error.is_connection_failure() => Ok(false),
            Err(error @ ClientError::Incompatible(_)) => Err(crate::cli::failed_connection(
                &self.socket,
                error,
                String::new(),
            )),
            Err(error) => Err(anyhow::Error::new(error).context("connecting to the daemon")),
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        self.client = Some(crate::cli::connect_or_start(&self.socket).await?);
        Ok(())
    }

    async fn remote(&mut self) -> anyhow::Result<RemoteStatus> {
        let what = "report its remote access settings";
        asked(what, requests::status(self.client(what)?).await)
    }

    async fn configure(
        &mut self,
        settings: RemoteSettings,
        expected_revision: u64,
    ) -> anyhow::Result<RemoteStatus> {
        let what = "change its remote access settings";
        let client = self.client(what)?;
        asked(
            what,
            requests::configure(client, settings, expected_revision).await,
        )
    }

    async fn start_pairing(&mut self, role: DeviceRole) -> anyhow::Result<Pairing> {
        let what = "open a pairing";
        asked(
            what,
            requests::start_pairing(self.client(what)?, role).await,
        )
    }

    async fn pairing(&mut self, id: &str) -> anyhow::Result<Pairing> {
        let what = "read the pairing";
        asked(what, requests::get_pairing(self.client(what)?, id).await)
    }

    async fn confirm_pairing(&mut self, id: &str, accept: bool) -> anyhow::Result<Pairing> {
        let what = "answer the pairing";
        let client = self.client(what)?;
        asked(what, requests::confirm_pairing(client, id, accept).await)
    }

    async fn cancel_pairing(&mut self, id: &str) -> anyhow::Result<Pairing> {
        let what = "cancel the pairing";
        asked(what, requests::cancel_pairing(self.client(what)?, id).await)
    }

    async fn devices(&mut self) -> anyhow::Result<Vec<Device>> {
        let what = "list the paired devices";
        asked(what, requests::devices(self.client(what)?, false).await)
    }
}
