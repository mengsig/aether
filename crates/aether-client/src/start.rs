//! Starting the daemon from a client, and seeing why it did not start.
//!
//! A daemon started here is detached: its own process group, no terminal, no
//! inherited stdio. It is not watched through a pipe, because the client is
//! short-lived and the daemon is not - a pipe would break under a daemon that
//! outlives its client, and only a reader that outlived it could catch a late
//! failure. Nor does the client keep the daemon's stderr on a file: that file
//! would be the daemon's stderr for its whole life, inherited by every tool
//! child it spawns, and would grow without bound.
//!
//! Instead a daemon that will not start writes the error that stopped it to a
//! file beside the socket (`aether daemon` does this; see `cli::daemon`), and
//! the client reports that file with the exit status it saw. A daemon that
//! dies without writing - a panic, a signal - is reported by its status alone.
//!
//! That file holds one start's message: whoever spawns next clears it, so two
//! clients starting at once can overwrite each other's, and a daemon that lost
//! a race leaves one beside a socket that serves. It is read only when nothing
//! answered there, so a stale message is never reported as a live failure.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use crate::{Client, ClientError, Result};

/// How long a client waits for a daemon it started to answer.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Exit status when another daemon holds the store or socket lock.
pub const DAEMON_LOCK_BUSY_EXIT: u8 = 75;

/// How much of a startup log this client will read to quote it.
const STARTUP_LOG_TAIL: u64 = 8 * 1024;

/// Connect to the daemon, starting it if needed.
///
/// `daemon_command` is spawned at most once, when no daemon answers; it must
/// run the daemon in the foreground, since this call is what detaches it. The
/// socket file is left alone: a failed connect does not prove its daemon
/// dead, and a starting daemon clears it only when nothing answers there.
///
/// A successful exit or lock contention can mean another daemon is coming
/// up. Other child failures are reported as soon as they are observed.
pub async fn ensure_daemon(socket: impl AsRef<Path>, daemon_command: Command) -> Result<Client> {
    let socket = socket.as_ref();
    match Client::connect(socket).await {
        Ok(client) => return Ok(client),
        Err(e) if e.is_connection_failure() => {}
        Err(e) => return Err(e),
    }
    let starting = StartingDaemon::spawn(daemon_command, socket)?;
    watch_startup(starting, socket).await
}

/// Wait for `starting` to either answer on `socket` or prove it never will,
/// within [`START_TIMEOUT`]. Shared by `ensure_daemon` and `aether daemon`'s
/// manual, detached start, so both report a dead child the same way.
pub async fn watch_startup(starting: StartingDaemon, socket: &Path) -> Result<Client> {
    watch_startup_with_timeout(starting, socket, START_TIMEOUT).await
}

async fn watch_startup_with_timeout(
    mut starting: StartingDaemon,
    socket: &Path,
    timeout: Duration,
) -> Result<Client> {
    debug_assert!(!socket.as_os_str().is_empty());
    debug_assert!(!timeout.is_zero());
    let deadline = tokio::time::Instant::now() + timeout;
    let mut delay = Duration::from_millis(100);
    let mut exited: Option<ExitStatus> = None;
    loop {
        match Client::connect(socket).await {
            Ok(client) => {
                starting.detach();
                return Ok(client);
            }
            Err(e) if e.is_connection_failure() => {}
            Err(e) => {
                starting.detach();
                return Err(e);
            }
        }
        // A successful connection takes precedence over the child exit status.
        if exited.is_none() {
            match starting.exited() {
                Ok(status) => exited = status,
                Err(e) => {
                    starting.detach();
                    return Err(e.into());
                }
            }
        }
        let failed = exited.is_some_and(|status| {
            !status.success() && status.code() != Some(i32::from(DAEMON_LOCK_BUSY_EXIT))
        });
        if failed || tokio::time::Instant::now() >= deadline {
            let failure = starting.never_answered(socket, exited);
            starting.detach();
            return Err(failure);
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + delay).min(deadline)).await;
        delay = (delay * 2).min(Duration::from_secs(1));
    }
}

/// A daemon process started by this client, watched while it comes up.
#[derive(Debug)]
pub struct StartingDaemon {
    child: Child,
    startup_log: PathBuf,
}

impl StartingDaemon {
    /// Spawn `command` detached, and clear the startup log beside `socket` so
    /// that whatever is found there afterwards is this daemon's.
    pub fn spawn(mut command: Command, socket: &Path) -> std::io::Result<Self> {
        use std::os::unix::process::CommandExt as _;

        if socket.as_os_str().is_empty() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "the daemon socket path is empty",
            ));
        }
        let startup_log = aether_protocol::paths::startup_log_path_for(socket);
        // An older start's words would otherwise be read as this one's.
        match std::fs::remove_file(&startup_log) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!(
                        "cannot clear the daemon startup log {}: {e}",
                        startup_log.display()
                    ),
                ));
            }
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        Ok(Self { child, startup_log })
    }

    /// Where this daemon leaves the error that stopped it.
    pub fn startup_log(&self) -> &Path {
        &self.startup_log
    }

    /// Let the daemon go. It outlives this client on purpose, so something
    /// must still reap it: a client that runs for hours - the TUI - would
    /// otherwise hold a zombie from the moment its daemon exits.
    pub fn detach(mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let mut child = self.child;
        let pid = child.id();
        let reaping = std::thread::Builder::new().spawn(move || {
            // There is nothing to report a late exit to: this client already
            // has its answer, and the daemon was never its to supervise.
            let _ = child.wait();
        });
        if let Err(e) = reaping {
            // The daemon itself is unaffected, and the client has its answer;
            // all this costs is one zombie once that daemon exits.
            tracing::warn!(pid, error = %e, "cannot start a thread to reap the daemon");
        }
    }

    /// The daemon's exit status once it has stopped, `None` while it runs.
    fn exited(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Why nothing answered on `socket` within the wait: what the daemon this
    /// client started did, in its own words wherever it left any.
    fn never_answered(&self, socket: &Path, exited: Option<ExitStatus>) -> ClientError {
        let Some(status) = exited else {
            return ClientError::Io(std::io::Error::new(
                ErrorKind::TimedOut,
                format!(
                    "the daemon did not answer on {} within {}s; it is still running, and \
                     anything it wrote while starting is in {}",
                    socket.display(),
                    START_TIMEOUT.as_secs(),
                    self.startup_log.display()
                ),
            ));
        };
        ClientError::DaemonStartup(format!(
            "aetherd exited ({status}) before it listened: {}",
            self.startup_output()
        ))
    }

    /// What the daemon wrote while starting, short enough for one error
    /// message. A log that cannot be read says so rather than reading empty.
    fn startup_output(&self) -> String {
        let nothing = || format!("it wrote nothing to {}", self.startup_log.display());
        match read_tail(&self.startup_log, STARTUP_LOG_TAIL) {
            Ok(text) => {
                let tail = tail(&text);
                if tail.is_empty() { nothing() } else { tail }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => nothing(),
            Err(e) => format!(
                "its startup log {} could not be read: {e}",
                self.startup_log.display()
            ),
        }
    }
}

/// The last `limit` bytes of `path` as text. The whole file is never read:
/// a client must stay bounded whatever it is pointed at.
fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    // A late replacement must not block before its descriptor can be validated.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() > 1 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "the daemon startup log must be a regular file without multiple hard links",
        ));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "the daemon startup log is not owned by the current user",
        ));
    }
    let len = metadata.len();
    if len > limit {
        file.seek(SeekFrom::Start(len - limit))?;
    }
    let mut bytes = Vec::with_capacity(len.min(limit) as usize);
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The end of a startup log: the error a daemon dies with is its last lines,
/// and a panic backtrace before it must not become the whole message.
fn tail(output: &str) -> String {
    const LINES: usize = 10;
    const MAX_BYTES: usize = 2000;

    let kept: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    let text = kept[kept.len().saturating_sub(LINES)..].join("\n");
    if text.len() <= MAX_BYTES {
        return text;
    }
    let cut = (text.len() - MAX_BYTES..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    format!("…{}", &text[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failed_child_returns_before_the_startup_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let mut command = Command::new("sh");
        command.args(["-c", "exit 3"]);
        let starting = StartingDaemon::spawn(command, &socket).unwrap();
        std::fs::write(starting.startup_log(), "bad configuration").unwrap();
        let began = std::time::Instant::now();
        let error = watch_startup(starting, &socket).await.unwrap_err();
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(matches!(error, ClientError::DaemonStartup(_)));
        let message = error.to_string();
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(
            message.contains("before it listened: bad configuration"),
            "{message}"
        );
        assert!(!message.contains("10s"), "{message}");
    }

    #[tokio::test]
    async fn successful_and_lock_busy_children_keep_waiting_for_another_daemon() {
        for status in [0, i32::from(DAEMON_LOCK_BUSY_EXIT)] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("absent.sock");
            let mut command = Command::new("sh");
            command.args(["-c", &format!("exit {status}")]);
            let starting = StartingDaemon::spawn(command, &socket).unwrap();
            let timeout = Duration::from_millis(400);
            let began = std::time::Instant::now();
            let error = watch_startup_with_timeout(starting, &socket, timeout)
                .await
                .unwrap_err();
            assert!(began.elapsed() >= timeout, "status {status} returned early");
            assert!(matches!(error, ClientError::DaemonStartup(_)), "{error}");
            assert!(
                error
                    .to_string()
                    .contains(&format!("exit status: {status}"))
            );
        }
    }

    #[test]
    fn a_tail_keeps_the_last_lines_and_drops_the_blank_ones() {
        let log = "first\n\nsecond\n";
        assert_eq!(tail(log), "first\nsecond");
        assert_eq!(tail(""), "");

        let many: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let tail = tail(&many);
        assert!(tail.starts_with("line 30\n"), "{tail}");
        assert!(tail.ends_with("line 39"), "{tail}");
    }

    #[test]
    fn a_tail_stays_bounded_on_one_enormous_line() {
        let huge = format!("é{}", "x".repeat(10_000));
        let tail = tail(&huge);
        assert!(tail.len() <= 2004, "{} bytes", tail.len());
        assert!(tail.ends_with("xxx"), "the end is what is kept");
    }

    /// A daemon's stderr can be any size; the client reads a bounded end of
    /// it, and that end is where the error that stopped it is.
    #[test]
    fn only_the_end_of_a_long_startup_log_is_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("aetherd.startup.log");
        let mut text = "chatter\n".repeat(4_000);
        text.push_str("Error: the last word\n");
        assert!(text.len() > STARTUP_LOG_TAIL as usize * 2, "a long log");
        std::fs::write(&log, &text).expect("the log");

        let read = read_tail(&log, STARTUP_LOG_TAIL).expect("the tail");
        assert!(
            read.len() as u64 <= STARTUP_LOG_TAIL,
            "{} bytes",
            read.len()
        );
        assert!(tail(&read).ends_with("Error: the last word"), "{read}");
        assert_eq!(
            read_tail(&log, STARTUP_LOG_TAIL * 1_000).expect("a short log whole"),
            text,
            "a file under the limit is read whole"
        );
    }

    #[test]
    fn startup_diagnostics_reject_links_without_reading_the_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("private.txt");
        let log = directory.path().join("aetherd.startup.log");
        std::fs::write(&target, "private target bytes").expect("target");

        symlink(&target, &log).expect("symlink");
        assert!(read_tail(&log, STARTUP_LOG_TAIL).is_err());
        std::fs::remove_file(&log).expect("remove symlink");

        std::fs::hard_link(&target, &log).expect("hard link");
        assert!(read_tail(&log, STARTUP_LOG_TAIL).is_err());
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "private target bytes"
        );
    }

    #[test]
    fn startup_diagnostics_reject_a_fifo_without_waiting_for_a_writer() {
        let directory = tempfile::tempdir().expect("tempdir");
        let fifo = directory.path().join("aetherd.startup.log");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
        )
        .expect("FIFO");

        // A subprocess keeps a regression in the synchronous open from hanging the suite.
        let mut fixture = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "start::tests::diagnostic_fifo_fixture",
            ])
            .env("AETHER_DIAGNOSTIC_FIFO", &fifo)
            .stdout(Stdio::null())
            .spawn()
            .expect("fixture");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = fixture.try_wait().expect("fixture status") {
                assert!(status.success(), "FIFO rejection fixture failed: {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                fixture.kill().expect("stop blocked fixture");
                fixture.wait().expect("reap blocked fixture");
                panic!("startup diagnostic reader blocked on a FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "subprocess fixture exercised by the FIFO regression"]
    fn diagnostic_fifo_fixture() {
        let fifo = std::env::var_os("AETHER_DIAGNOSTIC_FIFO").expect("FIFO fixture path");
        let error = read_tail(Path::new(&fifo), STARTUP_LOG_TAIL).expect_err("reject FIFO");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }
}
