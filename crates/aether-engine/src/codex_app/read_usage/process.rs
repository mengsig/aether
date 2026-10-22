use std::os::unix::process::CommandExt;
use std::process::Stdio;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use tokio::time::Instant;

use super::{EngineError, failure};

pub(super) struct UsageProcess {
    pub(super) child: Child,
}

impl UsageProcess {
    pub(super) fn spawn(program: &str, hidden_env: &[String]) -> Result<Self, EngineError> {
        let mut command = Command::new(program);
        command
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        aether_spawn::reset_signals(command.as_std_mut());
        crate::strip_hidden(&mut command, hidden_env);
        let child = command.spawn().map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => EngineError::NotInstalled {
                program: program.into(),
            },
            _ => EngineError::Spawn {
                program: program.into(),
                source,
            },
        })?;
        Ok(Self { child })
    }

    pub(super) async fn finish(&mut self, deadline: Instant) -> Result<(), EngineError> {
        // Signal before wait: the unreaped leader keeps its process-group id from being reused.
        let killed = self.kill_group();
        if killed.is_err() {
            self.child.start_kill().map_err(|error| {
                failure(format!(
                    "group cleanup failed ({killed:?}); killing server: {error}"
                ))
            })?;
        }
        tokio::time::timeout_at(deadline, self.child.wait())
            .await
            .map_err(|_| failure("timed out reaping server"))?
            .map_err(|error| failure(format!("reaping server: {error}")))?;
        debug_assert!(self.child.id().is_none(), "wait must reap the server");
        killed
    }

    fn kill_group(&self) -> Result<(), EngineError> {
        let Some(pid) = self.child.id() else {
            return Ok(());
        };
        assert!(pid > 1, "never signal the host process group");
        assert!(pid <= i32::MAX as u32, "process group must fit pid_t");
        match killpg(Pid::from_raw(pid as i32), Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(failure(format!("killing server group {pid}: {error}"))),
        }
    }
}

impl Drop for UsageProcess {
    fn drop(&mut self) {
        // Runtime shutdown can drop the supervisor; Child also kills/reaps its leader on drop.
        if let Err(error) = self.kill_group() {
            tracing::error!(%error, "usage server group cleanup failed; falling back to child kill");
        }
    }
}
