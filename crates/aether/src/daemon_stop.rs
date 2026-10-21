//! Fail-closed signalling for a daemon that refuses the shutdown protocol.

#[cfg(target_os = "linux")]
use anyhow::Context as _;

pub struct Peer {
    #[cfg(target_os = "linux")]
    pid: i32,
    #[cfg(target_os = "linux")]
    pidfd: std::os::fd::OwnedFd,
}

impl Peer {
    #[cfg(target_os = "linux")]
    pub fn identify(stream: &tokio::net::UnixStream) -> anyhow::Result<Self> {
        let cred = stream
            .peer_cred()
            .context("reading socket peer credentials")?;
        let pid = cred.pid().context("socket peer has no process ID")?;
        validate_identity(pid, cred.uid(), nix::unistd::geteuid().as_raw())?;
        // SO_PEERPIDFD pins the socket's process, unlike pidfd_open(peer_pid),
        // which could pin an unrelated process after PID reuse.
        let pidfd = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerPidfd)
            .context("obtaining socket peer pidfd (requires Linux 6.5 or newer)")?;
        Ok(Self { pid, pidfd })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn identify(_stream: &tokio::net::UnixStream) -> anyhow::Result<Self> {
        anyhow::bail!("this platform cannot pin the socket peer for race-free signalling")
    }

    pub fn pid(&self) -> i32 {
        #[cfg(target_os = "linux")]
        return self.pid;
        #[cfg(not(target_os = "linux"))]
        unreachable!("identification is unsupported on this platform")
    }

    #[cfg(target_os = "linux")]
    pub async fn terminate(self) -> anyhow::Result<()> {
        // `register` is unsafe (forbidden here); an owned pidfd moved in stays the
        // same open fd for the AsyncFd's life, which is the contract it asks for.
        #[allow(deprecated)]
        let pidfd = tokio::io::unix::AsyncFd::new(self.pidfd)
            .context("watching the pinned daemon for exit before signalling")?;
        rustix::process::pidfd_send_signal(pidfd.get_ref(), rustix::process::Signal::TERM)
            .with_context(|| format!("sending SIGTERM to the pinned daemon (pid {})", self.pid))?;
        let _ready = tokio::time::timeout(std::time::Duration::from_secs(5), pidfd.readable())
            .await
            .context("SIGTERM was sent but the daemon has not exited within 5s; no forced kill or socket removal attempted")?
            .context("waiting for the signalled daemon to exit")?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub async fn terminate(self) -> anyhow::Result<()> {
        anyhow::bail!("race-free signalling is unsupported on this platform")
    }
}

#[cfg(target_os = "linux")]
fn validate_identity(pid: i32, uid: u32, our_uid: u32) -> anyhow::Result<()> {
    anyhow::ensure!(pid > 1, "refusing to signal invalid or init peer pid {pid}");
    anyhow::ensure!(
        pid != std::process::id() as i32,
        "this process is itself listening on the socket"
    );
    anyhow::ensure!(
        uid == our_uid,
        "socket peer belongs to uid {uid}, not our uid {our_uid}"
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[allow(clippy::expect_used)]
    fn owned_child() -> (std::process::Child, Peer) {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("child");
        let pid = child.id() as i32;
        // This child is ours and unreaped, so its numeric PID cannot be reused.
        let pidfd = rustix::process::pidfd_open(
            rustix::process::Pid::from_raw(pid).expect("positive pid"),
            rustix::process::PidfdFlags::empty(),
        )
        .expect("pin owned child");
        (child, Peer { pid, pidfd })
    }

    #[tokio::test]
    async fn a_pinned_process_receives_sigterm() {
        let (mut child, peer) = owned_child();
        let result = peer.terminate().await;
        if result.is_err() && child.try_wait().expect("status").is_none() {
            child.kill().expect("clean up child");
        }
        let status = child.wait().expect("reap child");
        result.expect("SIGTERM and exit");
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(15)
        );
    }

    #[tokio::test]
    async fn an_exited_peer_is_an_error_not_a_numeric_pid_fallback() {
        let (mut child, peer) = owned_child();
        child.kill().expect("stop owned child");
        child.wait().expect("reap owned child");
        let error = peer
            .terminate()
            .await
            .expect_err("cannot signal an exited peer");
        assert_eq!(
            error.downcast_ref::<rustix::io::Errno>(),
            Some(&rustix::io::Errno::SRCH)
        );
    }

    #[test]
    fn invalid_self_and_foreign_peers_are_rejected() {
        for pid in [-1, 0, 1, std::process::id() as i32] {
            assert!(validate_identity(pid, 1000, 1000).is_err());
        }
        assert!(validate_identity(2, 1001, 1000).is_err());
        assert!(validate_identity(2, 1000, 1000).is_ok());
    }
}
