//! Host services for a client off the host (docs/remote.md specifies each reply). Every input is
//! attacker-influenced once a device is paired: text is checked here and all work is bounded.

use std::path::PathBuf;
use std::time::Duration;

use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};
use tokio::sync::Semaphore;

use crate::Daemon;

mod attention;
mod complete;
mod git;
mod workspaces;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

/// Longest path or prefix a client may send, in bytes (Linux `PATH_MAX`).
const MAX_PATH_BYTES: usize = 4096;
/// Filesystem jobs that may run at once; a hung mount can pin a thread
/// forever, so the rest are refused rather than queued behind it.
const MAX_FS_JOBS: usize = 8;
/// How long a filesystem job may take before the client is told so.
const FS_JOB_TIMEOUT: Duration = Duration::from_secs(5);

static FS_JOBS: Semaphore = Semaphore::const_new(MAX_FS_JOBS);

pub(super) async fn dispatch(daemon: &Daemon, command: Command) -> Result<Reply, ErrorReply> {
    match command {
        Command::GetAttention => attention::get_attention(daemon).await,
        Command::CompletePath {
            kind,
            prefix,
            workspace,
            limit,
        } => complete::complete_path(kind, prefix, workspace, limit).await,
        Command::ListWorkspaces { limit } => workspaces::list_workspaces(daemon, limit).await,
        Command::ValidateWorkspace { path } => workspaces::validate_workspace(path).await,
        Command::GetGitSummary { workspace } => workspaces::git_summary(workspace).await,
        other => Err(ErrorReply::internal(format!(
            "`{}` is not a host service",
            other.name()
        ))),
    }
}

fn unavailable(message: impl Into<String>) -> ErrorReply {
    ErrorReply::new(ErrorCode::Unavailable, message)
}

/// Text a client sent that names a path: bounded and free of NUL and control characters, which
/// would reach a terminal or a log unescaped.
fn check_text(field: &str, text: &str) -> Result<(), ErrorReply> {
    if text.len() > MAX_PATH_BYTES {
        return Err(ErrorReply::invalid(format!(
            "`{field}` is longer than {MAX_PATH_BYTES} bytes"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(ErrorReply::invalid(format!(
            "`{field}` contains a control character"
        )));
    }
    Ok(())
}

/// A path a client sent: checked as text, then required to be absolute.
fn check_absolute(field: &str, path: &std::path::Path) -> Result<PathBuf, ErrorReply> {
    let text = path
        .to_str()
        .ok_or_else(|| ErrorReply::invalid(format!("`{field}` is not valid UTF-8")))?;
    check_text(field, text)?;
    if !path.is_absolute() {
        return Err(ErrorReply::invalid(format!("`{field}` must be absolute")));
    }
    Ok(path.to_path_buf())
}

/// A client's `limit` against its default and ceiling.
fn effective_limit(
    limit: Option<u32>,
    default: usize,
    ceiling: usize,
) -> Result<usize, ErrorReply> {
    match limit {
        None => Ok(default),
        Some(0) => Err(ErrorReply::invalid("`limit` must be at least 1")),
        Some(n) => Ok((n as usize).min(ceiling)),
    }
}

/// Run `work` on the blocking pool, at most [`MAX_FS_JOBS`] at once, each given [`FS_JOB_TIMEOUT`].
/// A job past it keeps its slot until it returns, so a stuck mount cannot exhaust the pool.
async fn on_blocking_pool<T: Send + 'static>(
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ErrorReply> {
    run_bounded(&FS_JOBS, FS_JOB_TIMEOUT, what, work).await
}

async fn run_bounded<T: Send + 'static>(
    slots: &'static Semaphore,
    timeout: Duration,
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ErrorReply> {
    let slot = slots
        .try_acquire()
        .map_err(|_| unavailable(format!("too many {what} requests are running; try again")))?;
    let job = tokio::task::spawn_blocking(move || {
        let output = work();
        drop(slot);
        output
    });
    match tokio::time::timeout(timeout, job).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(ErrorReply::internal(format!("the {what} job failed: {e}"))),
        Err(_) => Err(unavailable(format!("{what} took too long"))),
    }
}

/// The daemon user's home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|home| home.is_absolute())
}
