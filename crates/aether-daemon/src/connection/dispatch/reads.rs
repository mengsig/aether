//! Store reads: events, transcripts, inboxes, snapshots, invocations,
//! checkpoints and artifacts.

use crate::Daemon;
use aether_protocol::{Command, ErrorReply, Reply};
use base64::Engine as _;

use crate::connection::dispatch::{MAX_ARTIFACT_READ_BYTES, blocking, masked_artifact_slice};
use crate::connection::store_err;

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::GetEvents { .. }
            | Command::GetTranscript { .. }
            | Command::GetTranscriptPage { .. }
            | Command::UsageBreakdown { .. }
            | Command::GetInbox { .. }
            | Command::GetSnapshot { .. }
            | Command::GetInvocation { .. }
            | Command::ListInvocations { .. }
            | Command::GetCheckpoint { .. }
            | Command::LatestCheckpoint { .. }
            | Command::GetArtifact { .. }
            | Command::ReadArtifact { .. }
    )
}

pub(super) async fn dispatch(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    if let Command::ReadArtifact { id, max_bytes } = command {
        return read_artifact(daemon, id, max_bytes).await;
    }
    // A long transcript or event read must not hold an async worker the
    // daemon answers other requests on (#333).
    let store = daemon.runtime.store().clone();
    blocking(move || read(store.as_ref(), command)).await?
}

fn read(store: &dyn aether_store::Store, command: Command) -> Result<Reply, ErrorReply> {
    match command {
        Command::GetEvents {
            after,
            limit,
            query,
        } => get_events(store, after, limit, query),
        Command::GetTranscript { agent, after } => Ok(Reply::Transcript(
            store
                .transcript(agent, after)
                .map_err(store_err)?
                .into_iter()
                .map(Into::into)
                .collect(),
        )),
        Command::GetTranscriptPage {
            agent,
            after,
            before,
            limit,
        } => get_transcript_page(store, agent, after, before, limit),
        Command::UsageBreakdown {
            scope,
            by,
            since,
            until,
            limit,
        } => {
            let limit = aether_protocol::read_models::usage_request(&scope, since, until, limit)
                .map_err(ErrorReply::invalid)?;
            let request = crate::usage_breakdown::UsageRequest {
                scope,
                by,
                since,
                until,
                limit: limit as usize,
            };
            let breakdown =
                crate::usage_breakdown::usage_breakdown(store, &request).map_err(store_err)?;
            Ok(Reply::UsageBreakdown(breakdown))
        }
        Command::GetInbox { agent, after } => {
            Ok(Reply::Inbox(store.inbox(agent, after).map_err(store_err)?))
        }
        Command::GetSnapshot { id } => store
            .snapshot(id)
            .map_err(store_err)?
            .map(Reply::Snapshot)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::GetInvocation { id } => store
            .invocation(id)
            .map_err(store_err)?
            .map(Reply::Invocation)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::ListInvocations { agent } => Ok(Reply::Invocations(
            store.invocations_for_agent(agent).map_err(store_err)?,
        )),
        Command::GetCheckpoint { id } => {
            Ok(Reply::Checkpoint(store.checkpoint(id).map_err(store_err)?))
        }
        Command::LatestCheckpoint { agent } => Ok(Reply::Checkpoint(
            store.latest_checkpoint(agent).map_err(store_err)?,
        )),
        Command::GetArtifact { id } => store
            .artifact(id)
            .map_err(store_err)?
            .map(Reply::Artifact)
            .ok_or_else(|| ErrorReply::not_found(id)),
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn read_artifact(
    daemon: &Daemon,
    id: aether_core::id::ArtifactId,
    max_bytes: usize,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let artifact = rt
        .store()
        .artifact(id)
        .map_err(store_err)?
        .ok_or_else(|| ErrorReply::not_found(id))?;
    let store = rt.store().clone();
    let redactor = rt.config().redactor.clone();
    // A published artifact is stored as sent, and an older tool artifact
    // unmasked: every reply is masked, observers' and the operator's alike.
    let read = blocking(move || {
        masked_artifact_slice(
            store.as_ref(),
            id,
            max_bytes,
            MAX_ARTIFACT_READ_BYTES,
            &redactor,
        )
    })
    .await?;
    let (slice, truncated) = read.map_err(store_err)?;
    Ok(Reply::ArtifactBytes {
        artifact,
        base64: base64::engine::general_purpose::STANDARD.encode(slice),
        truncated,
    })
}

fn get_transcript_page(
    store: &dyn aether_store::Store,
    agent: aether_core::id::AgentId,
    after: Option<u64>,
    before: Option<u64>,
    limit: Option<u32>,
) -> Result<Reply, ErrorReply> {
    let limit = aether_protocol::read_models::transcript_request(after, before, limit)
        .map_err(ErrorReply::invalid)?;
    let page = crate::projections::transcript_page(store, agent, after, before, limit as usize)
        .map_err(store_err)?;
    Ok(Reply::TranscriptPage {
        entries: page.entries,
        has_before: page.has_before,
        has_after: page.has_after,
        last_seq: page.last_seq,
    })
}

fn get_events(
    store: &dyn aether_store::Store,
    after: u64,
    limit: usize,
    query: aether_protocol::EventQuery,
) -> Result<Reply, ErrorReply> {
    let before = query.before;
    if before.is_some() && after != 0 {
        return Err(ErrorReply::invalid(
            "before cannot be combined with a nonzero after",
        ));
    }
    let filter = query.into();
    let limit = limit.clamp(1, 5000);
    let events = match before {
        Some(before) => store.events_before(before, limit, &filter),
        None => store.events(after, limit, &filter),
    };
    Ok(Reply::Events(events.map_err(store_err)?))
}
