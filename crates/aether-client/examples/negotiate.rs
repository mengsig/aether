//! Connect to a daemon the way every Rust client does, ask its status, and
//! print what the handshake settled on as one JSON line. The upgrade lane
//! (`scripts/upgrade_compat.py`, docs/ci.md) runs it against the previous
//! release, since the CLI itself reaches the TCP listener through no command.
//!
//! ```sh
//! cargo run -p aether-client --example negotiate -- unix "$AETHER_SOCKET"
//! AETHER_TOKEN=... cargo run -p aether-client --example negotiate -- tcp 127.0.0.1:8377
//! ```

use std::net::SocketAddr;

use aether_client::Client;
use aether_protocol::{Command, Reply};
use anyhow::{Context as _, bail};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [transport, target] = args.as_slice() else {
        bail!("usage: negotiate unix <socket> | negotiate tcp <host:port> (token in AETHER_TOKEN)");
    };
    let client = connect(transport, target).await?;
    let welcome = client
        .welcome()
        .context("a connected client holds the daemon's welcome")?
        .clone();
    let Reply::Status(status) = client.request(Command::Status).await? else {
        bail!("the daemon did not answer `status` with its status");
    };
    if status.version != welcome.daemon_version {
        bail!(
            "status names daemon {} but the welcome named {}",
            status.version,
            welcome.daemon_version
        );
    }
    let report = json!({
        "transport": transport,
        "protocol_version": welcome.protocol_version,
        "daemon_version": welcome.daemon_version,
        "daemon_protocol_version": welcome.daemon_protocol_version,
        "schema_version": welcome.schema_version,
        "features": welcome.features,
        "client_protocol_version": aether_protocol::PROTOCOL_VERSION,
    });
    println!("{report}");
    Ok(())
}

async fn connect(transport: &str, target: &str) -> anyhow::Result<Client> {
    match transport {
        "unix" => Client::connect(target)
            .await
            .with_context(|| format!("connecting to the socket {target}")),
        "tcp" => {
            let addr: SocketAddr = target
                .parse()
                .with_context(|| format!("`{target}` is not a host:port address"))?;
            let token = std::env::var("AETHER_TOKEN")
                .context("the TCP transport needs the daemon's token in AETHER_TOKEN")?;
            Client::connect_tcp(addr, token)
                .await
                .with_context(|| format!("connecting to {addr} over TCP"))
        }
        other => bail!("unknown transport `{other}`: use unix or tcp"),
    }
}
