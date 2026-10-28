use super::{connect, short};
use aether_client::Client;
use aether_core::id::NotificationId;
use aether_core::notification::{Notification, NotificationState};
use anyhow::{Context as _, bail};

/// The daemon never lists more than this many at once.
const LIST_LIMIT: usize = 1000;
const OPEN_STATES: [NotificationState; 2] = [NotificationState::Unread, NotificationState::Read];

pub async fn notify_list(
    socket: impl Into<aether_client::Endpoint>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    let client = connect_to_center(socket).await?;
    let states = if all {
        Vec::new()
    } else {
        OPEN_STATES.to_vec()
    };
    let listed = client.notifications(states, Some(LIST_LIMIT)).await?;
    if json {
        println!("{}", serde_json::to_string(&listed)?);
        return Ok(());
    }
    if listed.is_empty() {
        println!("no {}notifications", if all { "" } else { "open " });
        return Ok(());
    }
    let now = aether_core::time::now();
    for n in &listed {
        println!("{}", list_line(n, now));
    }
    Ok(())
}

pub async fn notify_resolve(
    socket: impl Into<aether_client::Endpoint>,
    id: Option<NotificationId>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    assert!(
        id.is_some() != all,
        "clap requires exactly one of <id> and --all"
    );
    let client = connect_to_center(socket).await?;
    if let Some(id) = id {
        let resolved = client
            .resolve_notification(id)
            .await
            .with_context(|| format!("resolve notification {id}"))?;
        return report_resolved(json, std::slice::from_ref(&resolved), false);
    }
    let resolved = resolve_all_open(&client).await?;
    report_resolved(json, &resolved, true)
}

async fn connect_to_center(socket: impl Into<aether_client::Endpoint>) -> anyhow::Result<Client> {
    let client = connect(socket).await?;
    if !client.serves_notifications() {
        bail!("this daemon has no notification center; update it and try again");
    }
    Ok(client)
}

/// Resolves every open notification, a listing page at a time.
async fn resolve_all_open(client: &Client) -> anyhow::Result<Vec<Notification>> {
    let mut resolved = Vec::new();
    loop {
        let open = client
            .notifications(OPEN_STATES.to_vec(), Some(LIST_LIMIT))
            .await?;
        if open.is_empty() {
            return Ok(resolved);
        }
        for n in open {
            let done = client
                .resolve_notification(n.id)
                .await
                .with_context(|| format!("resolve notification {}", n.id))?;
            assert_eq!(
                done.state,
                NotificationState::Resolved,
                "daemon left {} unresolved",
                n.id
            );
            resolved.push(done);
        }
    }
}

fn report_resolved(json: bool, resolved: &[Notification], counted: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(resolved)?);
    } else if counted {
        println!("resolved {}", resolved.len());
    } else {
        println!("resolved {}", resolved[0].id);
    }
    Ok(())
}

fn list_line(n: &Notification, now: aether_core::time::Timestamp) -> String {
    let age = age_label(now.signed_duration_since(n.created_at).num_seconds());
    format!(
        "{} {:<8} {:>4} {}",
        n.id,
        n.kind.as_str(),
        age,
        short(&n.title, 120)
    )
}

fn age_label(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::age_label;

    #[test]
    fn ages_use_the_largest_whole_unit() {
        assert_eq!(age_label(-5), "0s");
        assert_eq!(age_label(59), "59s");
        assert_eq!(age_label(60), "1m");
        assert_eq!(age_label(7_200), "2h");
        assert_eq!(age_label(172_800), "2d");
    }
}
