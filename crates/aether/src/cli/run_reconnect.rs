use super::{recovered_run_outcome, sane};
use aether_client::{Client, ClientError, Endpoint, Subscription};
use aether_protocol::{ErrorCode, EventQuery};
use anyhow::bail;
use std::time::Duration;

type RunConnection = (Client, Subscription, Option<&'static str>);

pub(super) async fn reconnect_run(
    endpoint: &Endpoint,
    previous: &Client,
    query: &EventQuery,
    last_seq: u64,
) -> anyhow::Result<RunConnection> {
    debug_assert!(previous.welcome().is_some());
    debug_assert!(query.agent.is_some());
    eprintln!(
        "{} connection lost; reconnecting (streamed text may be incomplete)",
        endpoint.display()
    );
    let mut after = Some(last_seq);
    if previous.history_expired() {
        clear_expired_cursor(&mut after);
    }
    let mut wait = Duration::from_secs(1);
    loop {
        let connected = tokio::select! {
            _ = tokio::signal::ctrl_c() => bail!("remote run monitoring interrupted; check the agent on the host"),
            result = reconnect_after(endpoint, wait) => result,
        };
        let client = match connected {
            Ok(client) => client,
            Err(error) => {
                eprintln!("reconnect failed: {}", sane(&error.to_string()));
                wait = (wait * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        anyhow::ensure!(
            client.welcome().map(|welcome| &welcome.instance_id)
                == previous.welcome().map(|welcome| &welcome.instance_id),
            "remote daemon identity changed; old run cursor discarded; reconnect in the TUI to reload the new host state"
        );
        if let Some(connection) = resume_monitor(client, query, after).await? {
            return Ok(connection);
        }
        clear_expired_cursor(&mut after);
    }
}

async fn reconnect_after(endpoint: &Endpoint, wait: Duration) -> anyhow::Result<Client> {
    debug_assert!(!wait.is_zero());
    debug_assert!(wait <= Duration::from_secs(30));
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .subsec_nanos();
    let jitter = Duration::from_millis(u64::from(seed) % (wait.as_millis() as u64 / 2 + 1));
    tokio::time::sleep((wait + jitter).min(Duration::from_secs(30))).await;
    Ok(Client::connect_endpoint(endpoint).await?)
}

async fn resume_monitor(
    client: Client,
    query: &EventQuery,
    after: Option<u64>,
) -> anyhow::Result<Option<RunConnection>> {
    let sub = match client.subscribe(after, query.clone(), true).await {
        Ok(sub) => sub,
        Err(ClientError::Protocol(error)) if error.code == ErrorCode::NotFound => return Ok(None),
        Err(_) if client.history_expired() => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let outcome = match recovered_run_outcome(&client, query).await {
        Err(_) if client.history_expired() => return Ok(None),
        result => result?,
    };
    if client.history_expired() {
        return Ok(None);
    }
    Ok(Some((client, sub, outcome)))
}

fn clear_expired_cursor(after: &mut Option<u64>) {
    *after = None;
    eprintln!("remote event history expired; reloading the agent snapshot");
}
