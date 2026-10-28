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
    let rt = &daemon.runtime;
    let store = rt.store().as_ref();
    match command {
        Command::GetEvents {
            after,
            limit,
            query,
        } => get_events(daemon, after, limit, query),
        Command::GetTranscript { agent, after } => Ok(Reply::Transcript(
            store
                .transcript(agent, after)
                .map_err(store_err)?
                .into_iter()
                .map(Into::into)
                .collect(),
        )),
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
        Command::ReadArtifact { id, max_bytes } => read_artifact(daemon, id, max_bytes).await,
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

fn get_events(
    daemon: &Daemon,
    after: u64,
    limit: usize,
    query: aether_protocol::EventQuery,
) -> Result<Reply, ErrorReply> {
    let store = daemon.runtime.store().as_ref();
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
