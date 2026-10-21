use crate::Daemon;
use crate::authz::AuthContext;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_protocol::{Command, ErrorReply, Reply};

use super::resolution;

mod agents;
mod control;
mod memory;
mod notifications;
mod reads;
mod runs;
mod schedules;
mod system;
mod tasks;

/// Where to cut `bytes` at about `max` so that text stays text. Slicing at a
/// byte offset can land inside a UTF-8 sequence, and a client decoding the
/// result then reports a text file as binary. Only an *incomplete* sequence
/// at the tail is backed off; genuinely invalid bytes are left alone, so
/// binary content is still cut at exactly `max`.
pub(super) fn text_safe_end(bytes: &[u8], max: usize) -> usize {
    match std::str::from_utf8(&bytes[..max]) {
        Ok(_) => max,
        // `error_len() == None` means "ran out of input", i.e. our own cut.
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => max,
    }
}

/// The most raw artifact bytes one `ReadArtifact` reply carries. Base64 adds a
/// third, and the reply must stay under the client's 64 MiB frame cap.
pub(super) const MAX_ARTIFACT_READ_BYTES: usize = 32 * 1024 * 1024;

/// Up to `max_bytes` (itself capped at `clamp`) of the artifact, cut at a
/// text boundary, and whether more follows. Reads one byte past the cap so
/// that "exactly at the cap" is not reported as truncated.
pub(super) fn read_artifact_slice(
    store: &dyn aether_store::Store,
    id: aether_core::id::ArtifactId,
    max_bytes: usize,
    clamp: usize,
) -> aether_store::Result<(Vec<u8>, bool)> {
    let cap = max_bytes.min(clamp);
    let mut bytes = store
        .read_artifact_range(id, 0, cap.saturating_add(1))?
        .unwrap_or_default();
    if bytes.len() > cap.saturating_add(1) {
        return Err(aether_store::StoreError::Corrupt(format!(
            "Artifact {id} exceeded the requested read limit; run `aether doctor`"
        )));
    }
    let truncated = bytes.len() > cap;
    if truncated {
        bytes.truncate(text_safe_end(&bytes, cap));
    }
    Ok((bytes, truncated))
}

/// Run `work` on the blocking pool. A panic in it is the connection's
/// internal error, not a lost reply.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ErrorReply> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ErrorReply::internal(format!("the task did not finish: {e}")))
}

/// The client's idempotency key on a command that creates work, with the
/// digest of the request it arrived on. `None` when the command carries no
/// key, or is not one that creates work.
///
/// The digest is taken over the *decoded* command's named fingerprint fields
/// ([`Command::keyed_request`]), not over the client's raw line. Two
/// consequences worth knowing: a retry that spells an optional field the other
/// way - omitted where the first sent the default, `5` where it sent `5.0` -
/// fingerprints equal, because both decode to the same typed command; and a
/// field this daemon learns about later cannot change a digest an earlier one
/// wrote. Untyped values (a `trigger` or schedule `payload`) are the exception:
/// they are digested as spelled.
fn submission_of(command: &Command, auth: &AuthContext) -> Result<Option<Submission>, ErrorReply> {
    auth.submission(command)
}

pub(super) async fn dispatch_version(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    auth.authorize(&command)?;
    crate::authz::guard(daemon, auth, &command)?;
    if let Command::ResolveSubmission { command, horizon } = command {
        return resolution::resolve(daemon, *command, horizon, by, auth).await;
    }
    let submission = submission_of(&command, auth)?;
    dispatch_authorized(daemon, command, by, auth, submission).await
}

/// Run a command its connection is already cleared to send. `submission` is
/// the command's own, or its replay-only twin when a resolution reads back.
pub(super) async fn dispatch_authorized(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    // Rule: a family that scans the store (agents, events, tasks) runs it under
    // `blocking()`, so concurrent clients cannot park every async worker.
    if system::owns(&command) {
        return system::dispatch(daemon, command, by, auth).await;
    }
    if agents::owns(&command) {
        return agents::dispatch(daemon, command, by, submission).await;
    }
    if control::owns(&command) {
        return control::dispatch(daemon, command, by, submission).await;
    }
    if runs::owns(&command) {
        return runs::dispatch(daemon, command).await;
    }
    if schedules::owns(&command) {
        return schedules::dispatch(daemon, command, by, submission).await;
    }
    if tasks::owns(&command) {
        return tasks::dispatch(daemon, command, by, submission).await;
    }
    if reads::owns(&command) {
        return reads::dispatch(daemon, command).await;
    }
    if memory::owns(&command) {
        return memory::dispatch(daemon, command, by, submission).await;
    }
    if notifications::owns(&command) {
        return notifications::dispatch(daemon, command, by).await;
    }
    Err(ErrorReply::internal("no handler for this command"))
}

#[cfg(test)]
pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    dispatch_version(daemon, command, by, &AuthContext::Local).await
}
