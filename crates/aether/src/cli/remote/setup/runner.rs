//! Running another program without a shell, with a timeout, behind a trait
//! so the wizard can be driven by a fake.

use std::fmt;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// More than any `tailscale` answer; the rest is dropped.
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Output {
    /// `None` when a signal ended the program.
    pub(super) code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

impl Output {
    pub(super) fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// What the program said when it failed: stderr, else stdout, shortened.
    pub(super) fn complaint(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let text = text.trim();
        if text.is_empty() {
            return "it printed nothing".to_owned();
        }
        let mut shown: String = text.chars().take(400).collect();
        if shown.len() < text.len() {
            shown.push('…');
        }
        shown
    }
}

#[derive(Debug)]
pub(super) enum RunError {
    NotFound,
    /// Killed at `limit`; `partial` is what it had printed by then.
    TimedOut {
        limit: Duration,
        partial: Output,
    },
    Failed(std::io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the program was not found"),
            Self::TimedOut { limit, .. } => {
                write!(f, "it did not finish within {}s", limit.as_secs())
            }
            Self::Failed(error) => write!(f, "it could not be started: {error}"),
        }
    }
}

impl std::error::Error for RunError {}

pub(super) trait CommandRunner {
    /// Runs `program` with `args` and no input, killing it after `timeout`.
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, RunError>;
}

/// `program args…` as a person would type it, for messages.
pub(super) fn display(program: &str, args: &[&str]) -> String {
    let mut line = program.to_owned();
    for arg in args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}

#[derive(Debug)]
pub(super) struct SystemRunner;

impl CommandRunner for SystemRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, RunError> {
        assert!(!program.is_empty(), "a program name is required");
        assert!(!timeout.is_zero(), "a timeout is required");
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => RunError::NotFound,
                _ => RunError::Failed(error),
            })?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let missing = || RunError::Failed(std::io::Error::other("an output pipe is missing"));
        let mut out_pipe = child.stdout.take().ok_or_else(missing)?;
        let mut err_pipe = child.stderr.take().ok_or_else(missing)?;
        let finished = tokio::time::timeout(timeout, async {
            let (out, err, status) = tokio::join!(
                capture(&mut out_pipe, &mut stdout),
                capture(&mut err_pipe, &mut stderr),
                child.wait()
            );
            out.and(err)?;
            status
        })
        .await;
        match finished {
            Ok(status) => Ok(Output {
                code: status.map_err(RunError::Failed)?.code(),
                stdout: lossy_limited(&stdout),
                stderr: lossy_limited(&stderr),
            }),
            Err(_) => Err(RunError::TimedOut {
                limit: timeout,
                partial: Output {
                    code: None,
                    stdout: lossy_limited(&stdout),
                    stderr: lossy_limited(&stderr),
                },
            }),
        }
    }
}

/// Reads `pipe` to its end, keeping the first [`OUTPUT_LIMIT`] bytes and
/// draining the rest so the program is never blocked on a full pipe.
async fn capture(
    pipe: &mut (impl tokio::io::AsyncRead + Unpin),
    into: &mut Vec<u8>,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt as _;
    let mut chunk = [0u8; 4096];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let room = OUTPUT_LIMIT.saturating_sub(into.len());
        into.extend_from_slice(&chunk[..count.min(room)]);
    }
}

fn lossy_limited(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(OUTPUT_LIMIT)]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_program_stopped_at_the_timeout_hands_back_what_it_printed() {
        let script = "echo to-out; echo to-err >&2; sleep 30";
        let ended = SystemRunner
            .run("sh", &["-c", script], Duration::from_secs(2))
            .await;
        let Err(RunError::TimedOut { limit, partial }) = ended else {
            panic!("expected a timeout, got {ended:?}");
        };
        assert_eq!(limit, Duration::from_secs(2));
        assert_eq!(partial.stdout.trim(), "to-out");
        assert_eq!(partial.stderr.trim(), "to-err");
        assert_eq!(partial.code, None);
    }

    #[tokio::test]
    async fn a_program_that_finishes_returns_all_of_its_output_and_code() {
        let script = "echo said; echo oops >&2; exit 3";
        let output = SystemRunner
            .run("sh", &["-c", script], Duration::from_secs(10))
            .await
            .expect("finishes");
        assert_eq!(output.code, Some(3));
        assert_eq!(output.stdout.trim(), "said");
        assert_eq!(output.stderr.trim(), "oops");
    }
}
