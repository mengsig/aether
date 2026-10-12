//! Remote operations whose delivery the host could not confirm, and the
//! three things a person may decide about one.
use super::{connect_unannounced, sane};
use aether_client::{Client, Endpoint, Settlement, Unreadable, Unresolved};

/// How much of a command is shown on one line before `--json` is the way to read it.
const SHOWN_COMMAND_CHARS: usize = 160;

pub async fn outbox_list(endpoint: impl Into<Endpoint>, json: bool) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = connect_unannounced(&endpoint).await?;
    let undecided = client.undecided().await?;
    if json {
        let mut listed = Vec::new();
        for entry in &undecided.unconfirmed {
            listed.push(serde_json::to_value(entry)?);
        }
        for bad in &undecided.unreadable {
            listed.push(serde_json::to_value(bad)?);
        }
        println!("{}", serde_json::to_string_pretty(&listed)?);
        return Ok(());
    }
    if undecided.is_empty() {
        println!("No remote operation is waiting on a decision.");
        return Ok(());
    }
    let aether = endpoint.command_hint(true);
    for entry in &undecided.unconfirmed {
        println!("{}", describe(entry)?);
    }
    for bad in &undecided.unreadable {
        println!("{}", describe_unreadable(bad));
    }
    println!(
        "Decide each with `{aether} outbox delivered <KEY>`, `{aether} outbox resend <KEY>` \
         or `{aether} outbox discard <KEY>`; an unreadable file can only be discarded."
    );
    Ok(())
}

pub async fn outbox_settle(
    endpoint: impl Into<Endpoint>,
    key: &str,
    choice: Settlement,
    json: bool,
) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = connect_unannounced(&endpoint).await?;
    let reply = client.settle(key, choice).await?;
    assert_eq!(
        reply.is_some(),
        choice == Settlement::SendAgain,
        "only sending again has an answer"
    );
    if json {
        let decision = match choice {
            Settlement::MarkDelivered => "marked_delivered",
            Settlement::SendAgain => "sent_again",
            Settlement::Discard => "discarded",
        };
        let settled = serde_json::json!({"key": key, "decision": decision, "reply": reply});
        println!("{}", serde_json::to_string_pretty(&settled)?);
        return Ok(());
    }
    match choice {
        Settlement::MarkDelivered => println!("Marked {} delivered; nothing was sent.", sane(key)),
        Settlement::Discard => println!("Discarded {}; nothing was sent.", sane(key)),
        Settlement::SendAgain => println!("Sent {} again and the host accepted it.", sane(key)),
    }
    Ok(())
}

/// Say on stderr that decisions are waiting, without holding the command up.
pub(super) async fn announce_unconfirmed(
    client: &Client,
    endpoint: &Endpoint,
) -> anyhow::Result<()> {
    let undecided = client.undecided().await?;
    if undecided.is_empty() {
        return Ok(());
    }
    let aether = endpoint.command_hint(true);
    for entry in &undecided.unconfirmed {
        eprintln!("delivery unconfirmed: {}", describe(entry)?);
    }
    for bad in &undecided.unreadable {
        eprintln!("unreadable outbox file: {}", describe_unreadable(bad));
    }
    if !undecided.unconfirmed.is_empty() {
        eprintln!(
            "{} earlier remote operation(s) may or may not have run; decide each with `{aether} outbox`",
            undecided.unconfirmed.len(),
        );
    }
    if !undecided.unreadable.is_empty() {
        eprintln!(
            "{} unreadable outbox file(s) do not hold up this command; remove each with \
             `{aether} outbox discard <KEY>`",
            undecided.unreadable.len(),
        );
    }
    Ok(())
}

fn describe_unreadable(bad: &Unreadable) -> String {
    format!(
        "{}  (not an operation)\n    {}",
        sane(&bad.key),
        sane(&bad.reason)
    )
}

fn describe(entry: &Unresolved) -> anyhow::Result<String> {
    let reason = entry
        .unconfirmed
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{} is not unconfirmed", entry.key()))?;
    let command = sane(&serde_json::to_string(&entry.command)?);
    let shown: String = command.chars().take(SHOWN_COMMAND_CHARS).collect();
    let cut = if shown.len() < command.len() {
        "…"
    } else {
        ""
    };
    Ok(format!(
        "{}  {}  sent {}  {shown}{cut}\n    {}",
        sane(entry.key()),
        entry.command.name(),
        entry.sent_at.format("%Y-%m-%d %H:%M:%SZ"),
        sane(reason)
    ))
}
