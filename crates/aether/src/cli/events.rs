use std::{io::IsTerminal, time::Duration};

use aether_client::Endpoint;
use aether_core::{event::Event, time::Timestamp};
use anyhow::Context as _;

use super::{
    AgentRef, Client, EventQuery, RunRef, SubscriptionFrame, TaskRef, connect, tasks::summarize,
};

#[derive(Debug, clap::Args)]
pub struct EventArgs {
    /// Follow live events after the selected history.
    #[arg(long, short = 'f')]
    follow: bool,
    /// Only events with a sequence greater than this cursor.
    #[arg(long, default_value_t = 0)]
    after: u64,
    /// Filter by agent (id, prefix or name); `--agent` is the same.
    #[arg(conflicts_with = "agent_flag")]
    agent: Option<AgentRef>,
    /// Filter by agent, as the positional argument does.
    #[arg(long = "agent", id = "agent_flag", value_name = "AGENT")]
    agent_flag: Option<AgentRef>,
    /// Filter by run.
    #[arg(long)]
    run: Option<RunRef>,
    /// Filter by task.
    #[arg(long)]
    task: Option<TaskRef>,
    /// Event type, e.g. tool.failed; repeat or separate with commas.
    #[arg(long = "type", value_delimiter = ',')]
    types: Vec<String>,
    /// Event family, e.g. tool; repeat or separate with commas.
    #[arg(long = "family", value_delimiter = ',')]
    families: Vec<String>,
    /// Only events this recent (integer seconds, minutes, hours or days: 30s, 5m, 2h, 1d).
    #[arg(long, value_parser = super::units::parse_duration)]
    since: Option<Duration>,
    /// Latest N matching events (default 50 on a terminal; unlimited when piped).
    #[arg(long)]
    tail: Option<usize>,
    /// Case-sensitive literal substring in the full event JSON; filtered locally.
    #[arg(long)]
    grep: Option<String>,
    /// One event per line, so `-f --json` is a stream.
    #[arg(long)]
    json: bool,
}

impl EventArgs {
    async fn query(&self, client: &Client, socket: &Endpoint) -> anyhow::Result<EventQuery> {
        let mut query = EventQuery {
            types: self.types.clone(),
            families: self.families.clone(),
            ..Default::default()
        };
        if let Some(agent) = self.agent.as_ref().or(self.agent_flag.as_ref()) {
            query.agent = Some(agent.resolve(client, socket).await?);
        }
        if let Some(run) = &self.run {
            query.run = Some(run.resolve(client, socket).await?);
        }
        if let Some(task) = &self.task {
            query.task = Some(task.resolve(client).await?);
        }
        debug_assert_eq!(
            query.agent.is_some(),
            self.agent.is_some() || self.agent_flag.is_some()
        );
        Ok(query)
    }

    fn tail_limit(&self, terminal: bool) -> Option<usize> {
        self.tail.or(terminal.then_some(50))
    }

    fn cutoff(&self, now: Timestamp) -> anyhow::Result<Option<Timestamp>> {
        self.since
            .map(|duration| {
                let seconds =
                    i64::try_from(duration.as_secs()).context("--since duration is too large")?;
                let timestamp = now
                    .timestamp()
                    .checked_sub(seconds)
                    .context("--since timestamp overflow")?;
                Timestamp::from_timestamp(timestamp, now.timestamp_subsec_nanos())
                    .context("--since is outside the timestamp range")
            })
            .transpose()
    }

    fn matches(&self, event: &Event, cutoff: Option<Timestamp>) -> anyhow::Result<bool> {
        if cutoff.is_some_and(|since| event.at < since) {
            return Ok(false);
        }
        match &self.grep {
            Some(needle) => Ok(serde_json::to_string(event)?.contains(needle)),
            None => Ok(true),
        }
    }
}

/// Exit status of `events --follow` when the daemon ended the stream.
pub const STREAM_ENDED_EXIT: i32 = 3;

const STREAM_ENDED_MESSAGE: &str = "event stream ended: the daemon stopped";

/// Prints events; returns the process exit status.
pub async fn events(
    socket: impl Into<aether_client::Endpoint>,
    args: EventArgs,
) -> anyhow::Result<i32> {
    let socket = socket.into();
    let cutoff = args.cutoff(aether_core::time::now())?;
    let tail = args.tail_limit(std::io::stdout().is_terminal());
    let client = connect(&socket).await?;
    let query = args.query(&client, &socket).await?;
    let mut cursor = args.after;
    if let Some(limit) = tail {
        cursor = cursor.max(client.status().await?.last_seq);
        for event in tail_events(&client, &args, query.clone(), cutoff, limit, cursor).await? {
            print_event(&event, args.json)?;
        }
    } else if !args.follow {
        print_history(&client, &args, query, cutoff).await?;
        return Ok(0);
    }
    if !args.follow {
        return Ok(0);
    }
    let mut sub = client.subscribe(Some(cursor), query, false).await?;
    while let Some(frame) = sub.next().await {
        match frame {
            SubscriptionFrame::Event(e) if args.matches(&e, cutoff)? => print_event(&e, args.json)?,
            SubscriptionFrame::Lagged { capacity } => anyhow::bail!(
                "event subscription exceeded {capacity} queued frames; resume with --after using the last sequence printed"
            ),
            _ => {}
        }
    }
    eprintln!("{STREAM_ENDED_MESSAGE}");
    Ok(STREAM_ENDED_EXIT)
}

async fn tail_events(
    client: &Client,
    args: &EventArgs,
    mut query: EventQuery,
    cutoff: Option<Timestamp>,
    limit: usize,
    latest: u64,
) -> anyhow::Result<Vec<Event>> {
    let mut before = latest.checked_add(1).context("event cursor overflow")?;
    let mut result = Vec::new();
    while result.len() < limit && before > args.after.saturating_add(1) {
        query.before = Some(before);
        let page_size = if cutoff.is_some() || args.grep.is_some() {
            1000
        } else {
            (limit - result.len()).min(1000)
        };
        let page = client.events(0, page_size, query.clone()).await?;
        anyhow::ensure!(
            page.len() <= page_size,
            "event page exceeded requested limit"
        );
        if page.is_empty() {
            break;
        }
        for event in page {
            anyhow::ensure!(
                event.seq < before,
                "event page did not retreat before {before}"
            );
            before = event.seq;
            if event.seq <= args.after {
                break;
            }
            if args.matches(&event, cutoff)? {
                result.push(event);
                if result.len() == limit {
                    break;
                }
            }
        }
    }
    result.reverse();
    Ok(result)
}

async fn print_history(
    client: &Client,
    args: &EventArgs,
    query: EventQuery,
    cutoff: Option<Timestamp>,
) -> anyhow::Result<()> {
    let mut cursor = args.after;
    loop {
        let page = client.events(cursor, 1000, query.clone()).await?;
        anyhow::ensure!(page.len() <= 1000, "event page exceeded requested limit");
        if page.is_empty() {
            return Ok(());
        }
        for event in page {
            anyhow::ensure!(
                event.seq > cursor,
                "event page did not advance past {cursor}"
            );
            cursor = event.seq;
            if args.matches(&event, cutoff)? {
                print_event(&event, args.json)?;
            }
        }
    }
}

fn event_line(event: &Event) -> String {
    format!(
        "{:>7} {} {:<24} {} {}",
        event.seq,
        event.at.format("%Y-%m-%d %H:%M:%S%.3fZ"),
        event.type_name(),
        event.agent_id.map(|a| a.to_string()).unwrap_or_default(),
        super::sane(&summarize(&event.kind))
    )
}

fn print_event(event: &Event, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(event)?);
    } else {
        println!("{}", event_line(event));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
