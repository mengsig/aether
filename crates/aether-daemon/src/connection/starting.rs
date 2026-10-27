//! Requests that arrive while the startup probes, engine detection and the
//! default model, still run.

use std::time::Duration;

use aether_protocol::{Command, ErrorCode, ErrorReply};
use tokio_util::sync::CancellationToken;

use crate::Daemon;

/// Longest a request waits for the startup probes before it is told to retry.
/// The probes themselves are bounded by their CLI timeouts.
const ENGINE_DETECTION_WAIT: Duration = Duration::from_secs(20);

/// Answered at once: none acts on anything the startup probes set.
fn answers_while_starting(command: &Command) -> bool {
    matches!(command, Command::Ping | Command::Status | Command::Shutdown)
}

/// Hold `command` until the startup probes have finished, so no request acts
/// on a half-initialised daemon; a wait that runs out is a retryable refusal.
/// Like any slow request, the wait holds the later frames on its connection.
pub(super) async fn await_startup_probes(
    daemon: &Daemon,
    command: &Command,
    cancel: &CancellationToken,
) -> Result<(), ErrorReply> {
    if answers_while_starting(command) || daemon.runtime.startup_probes_finished() {
        return Ok(());
    }
    let waited = tokio::select! {
        _ = cancel.cancelled() => {
            return Err(ErrorReply::new(ErrorCode::Unavailable, "the daemon is shutting down"));
        }
        waited = tokio::time::timeout(ENGINE_DETECTION_WAIT, daemon.runtime.await_startup_probes()) => waited,
    };
    if waited.is_err() {
        return Err(ErrorReply::new(
            ErrorCode::Unavailable,
            "daemon starting: engine and default model detection is still running; retry in a moment",
        ));
    }
    debug_assert!(daemon.runtime.startup_probes_finished());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::answers_while_starting;
    use aether_protocol::Command;

    #[test]
    fn only_requests_that_read_no_engine_skip_the_wait() {
        assert!(answers_while_starting(&Command::Ping));
        assert!(answers_while_starting(&Command::Status));
        assert!(answers_while_starting(&Command::Shutdown));
        assert!(!answers_while_starting(&Command::ListModels));
    }
}
