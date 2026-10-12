//! Owned SSH carrier; dropping the last client cancels and reaps its child.

use crate::{Client, ClientError, Result, SshRemote, Transport};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) struct Carrier {
    _cancel: oneshot::Sender<()>,
    diagnostics: Arc<Mutex<String>>,
}

impl Carrier {
    #[cfg(test)]
    pub(crate) fn test_carrier(cancel: oneshot::Sender<()>) -> Self {
        Self {
            _cancel: cancel,
            diagnostics: Arc::new(Mutex::new(String::new())),
        }
    }

    pub(crate) fn diagnostics(&self) -> String {
        self.diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Connect at the newest version the host serves, trying each this build
/// can speak from its own down to the oldest.
pub(crate) async fn connect(remote: &SshRemote) -> Result<Client> {
    let versions =
        aether_protocol::compat::MIN_PROTOCOL_VERSION..=aether_protocol::PROTOCOL_VERSION;
    let mut refusals: Vec<String> = Vec::new();
    for version in versions.rev() {
        match connect_version(remote, version).await {
            Err(ClientError::Incompatible(refusal)) if refusals.is_empty() => {
                refusals.push(refusal);
            }
            Err(ClientError::Incompatible(refusal)) => {
                refusals.push(format!("protocol {version} also refused: {refusal}"));
            }
            result => return result,
        }
    }
    debug_assert!(
        !refusals.is_empty(),
        "the window holds at least one version"
    );
    Err(ClientError::Incompatible(refusals.join("; ")))
}

async fn connect_version(remote: &SshRemote, version: u32) -> Result<Client> {
    let mut client = spawn_client(remote, std::ffi::OsStr::new("ssh"))?;
    let welcome = tokio::time::timeout(
        Duration::from_secs(15),
        client.handshake_version(None, version),
    )
    .await;
    match welcome {
        Ok(Ok(welcome)) => {
            if welcome.protocol_version != version || welcome.instance_id.is_empty() {
                return Err(ClientError::Malformed(
                    "remote returned an invalid welcome version or instance identity".into(),
                ));
            }
            finish_connection(&mut client, remote, welcome).await?;
            Ok(client)
        }
        Ok(Err(error @ (ClientError::Incompatible(_) | ClientError::Unauthorized(_)))) => {
            Err(error)
        }
        failure => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(ClientError::Ssh(format!(
                "{} ({}): {}; {}. Check SSH access and start the daemon on the host",
                remote.name(),
                remote.target(),
                match failure {
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                    _ => unreachable!(),
                },
                client
                    .carrier
                    .as_ref()
                    .map(|carrier| carrier.diagnostics())
                    .unwrap_or_default()
            )))
        }
    }
}

fn spawn_client(remote: &SshRemote, program: &std::ffi::OsStr) -> Result<Client> {
    let args = remote.command();
    let mut child = tokio::process::Command::new(program)
        .args(&args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let read = child
        .stdout
        .take()
        .ok_or_else(|| ClientError::Malformed("SSH stdout missing".into()))?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| ClientError::Malformed("SSH stdin missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ClientError::Malformed("SSH stderr missing".into()))?;
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let (cancel, cancelled) = oneshot::channel();
    tokio::spawn(supervise(child, stderr, cancelled, diagnostics.clone()));
    let carrier = Arc::new(Carrier {
        _cancel: cancel,
        diagnostics,
    });
    let mut client = Client::from_transport(Transport::Stdio { read, write });
    client.carrier = Some(carrier);
    Ok(client)
}

async fn finish_connection(
    client: &mut Client,
    remote: &SshRemote,
    welcome: aether_protocol::Welcome,
) -> Result<()> {
    client.keep_alive(welcome.idle_timeout_secs);
    let directory = crate::Endpoint::Ssh(remote.clone())
        .state_dir(&welcome.instance_id)
        .join("outbox");
    client.welcome = Some(welcome);
    let outbox = Arc::new(crate::outbox::Outbox::open(directory).await?);
    client.adopt_outbox(outbox).await
}

async fn supervise(
    mut child: tokio::process::Child,
    stderr: tokio::process::ChildStderr,
    mut cancelled: oneshot::Receiver<()>,
    diagnostics: Arc<Mutex<String>>,
) {
    let mut draining = tokio::spawn(drain(stderr, diagnostics.clone()));
    let status = tokio::select! {
        result = child.wait() => result,
        _ = &mut cancelled => {
            if let Err(error) = child.start_kill() { tracing::debug!(%error, "SSH kill failed; awaiting child"); }
            child.wait().await
        }
    };
    match tokio::time::timeout(Duration::from_secs(1), &mut draining).await {
        Ok(Ok(())) => {}
        result => {
            draining.abort();
            tracing::warn!(?result, "SSH stderr drain did not finish");
        }
    }
    let message = match status {
        Ok(status) => format!("SSH exited ({status})"),
        Err(error) => format!("cannot reap SSH: {error}"),
    };
    let mut text = diagnostics.lock().unwrap_or_else(|e| e.into_inner());
    text.push_str(&format!("\n{message}"));
}

async fn drain(mut stderr: tokio::process::ChildStderr, diagnostics: Arc<Mutex<String>>) {
    let mut buffer = [0; 4096];
    loop {
        let count = match stderr.read(&mut buffer).await {
            Ok(0) => return,
            Ok(count) => count,
            Err(error) => {
                diagnostics
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_str(&format!("\nreading SSH stderr: {error}"));
                return;
            }
        };
        let mut text = diagnostics.lock().unwrap_or_else(|e| e.into_inner());
        text.push_str(&String::from_utf8_lossy(&buffer[..count]));
        if text.len() > 8192 {
            let boundary = (text.len() - 8192..text.len())
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(text.len());
            text.drain(..boundary);
        }
    }
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
