use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

#[derive(Debug, thiserror::Error)]
#[error("local voice cleanup failed: {0:#}")]
pub struct CleanupFailure(#[source] pub anyhow::Error);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Run,
    Stop,
    Cancel,
}

#[derive(Debug)]
struct Group {
    child: Child,
    pid: Pid,
    cleaned: bool,
}

impl Group {
    fn kill(&self) -> anyhow::Result<()> {
        match killpg(self.pid, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error).context("kill local voice process group"),
        }
    }

    async fn cleanup(&mut self) -> anyhow::Result<()> {
        let killed = self.kill();
        let reaped = tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .context("timed out reaping local voice process")
            .and_then(|result| result.context("reap local voice process"));
        self.cleaned = killed.is_ok() && reaped.is_ok();
        killed?;
        reaped?;
        Ok(())
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if !self.cleaned {
            // Unwinding still kills the group; Tokio's kill_on_drop reaper owns the leader.
            if let Err(error) = self.kill() {
                tracing::error!(%error, "local voice emergency cleanup failed");
            }
        }
    }
}

async fn bounded_read<R: AsyncRead + Unpin>(
    reader: R,
    bytes: &mut Vec<u8>,
    limit: usize,
) -> anyhow::Result<()> {
    let remaining = limit.saturating_sub(bytes.len());
    reader.take(remaining as u64 + 1).read_to_end(bytes).await?;
    if bytes.len() > limit {
        bail!("local voice process exceeded its {limit}-byte output limit");
    }
    Ok(())
}

async fn interrupted(control: &mut watch::Receiver<Control>, recording: bool) -> bool {
    loop {
        match *control.borrow_and_update() {
            Control::Cancel => return false,
            Control::Stop if recording => return true,
            _ => {}
        }
        if control.changed().await.is_err() {
            return false;
        }
    }
}

pub async fn run(
    command: Command,
    input: &[u8],
    limit: usize,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
    recording: bool,
) -> anyhow::Result<Vec<u8>> {
    run_inner(
        command,
        input,
        limit,
        timeout,
        control,
        Recording {
            enabled: recording,
            ..Recording::default()
        },
    )
    .await
}

pub async fn run_explicit_recording(
    command: Command,
    limit: usize,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
    on_audio: impl FnOnce() + Send + 'static,
) -> anyhow::Result<Vec<u8>> {
    run_inner(
        command,
        &[],
        limit,
        timeout,
        control,
        Recording {
            enabled: true,
            require_explicit_stop: true,
            on_audio: Some(Box::new(on_audio)),
        },
    )
    .await
}

#[derive(Default)]
struct Recording {
    enabled: bool,
    require_explicit_stop: bool,
    on_audio: Option<Box<dyn FnOnce() + Send>>,
}

async fn recording_read<R: AsyncRead + Unpin>(
    mut reader: R,
    bytes: &mut Vec<u8>,
    limit: usize,
    on_audio: Option<Box<dyn FnOnce() + Send>>,
) -> anyhow::Result<()> {
    if let Some(on_audio) = on_audio {
        let mut first = [0; 1];
        if reader.read(&mut first).await? != 0 {
            bytes.push(first[0]);
            on_audio();
        }
    }
    bounded_read(reader, bytes, limit).await
}

async fn run_inner(
    mut command: Command,
    input: &[u8],
    limit: usize,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
    recording: Recording,
) -> anyhow::Result<Vec<u8>> {
    let Recording {
        enabled: recording,
        require_explicit_stop,
        on_audio,
    } = recording;
    if *control.borrow() == Control::Cancel {
        bail!("cancelled");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        aether_spawn::detach_session(command.as_std_mut());
    }
    let child = command
        .spawn()
        .context("start local voice executable; check voice.toml paths")?;
    let pid = child.id().context("local voice process has no pid")?;
    let mut group = Group {
        child,
        pid: Pid::from_raw(pid as i32),
        cleaned: false,
    };
    let mut stdin = group.child.stdin.take().context("missing voice stdin")?;
    let stdout = group.child.stdout.take().context("missing voice stdout")?;
    let stderr = group.child.stderr.take().context("missing voice stderr")?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut explicitly_stopped = false;
    let mut result = {
        let io = async {
            let (_, _, _, status) = tokio::try_join!(
                async move {
                    stdin.write_all(input).await?;
                    stdin.shutdown().await?;
                    drop(stdin);
                    Ok::<_, anyhow::Error>(())
                },
                recording_read(stdout, &mut output, limit, on_audio),
                bounded_read(stderr, &mut errors, 16_384),
                async { group.child.wait().await.map_err(anyhow::Error::from) },
            )?;
            if !status.success() {
                bail!(
                    "local voice executable exited with {status}; check model/device configuration"
                );
            }
            Ok(())
        };
        tokio::select! {
            biased;
            result = io => result,
            stop = interrupted(control, recording) => {
                explicitly_stopped = stop;
                if stop { Ok(()) } else { Err(anyhow::anyhow!("cancelled")) }
            }
            _ = tokio::time::sleep(timeout) => {
                if recording { Ok(()) } else { Err(anyhow::anyhow!("local voice process timed out")) }
            }
        }
    };
    if recording && result.is_ok() {
        match group.child.try_wait() {
            Ok(Some(status)) if !status.success() || require_explicit_stop => {
                result = Err(anyhow::anyhow!("recorder exited with {status} before stop"));
            }
            Err(error) => result = Err(error.into()),
            _ => {}
        }
    }
    if require_explicit_stop && result.is_ok() && !explicitly_stopped {
        result = Err(anyhow::anyhow!(
            "voice reply recording stopped without the reply shortcut (timeout or recorder exit); nothing sent"
        ));
    }
    let cleanup = group.cleanup().await.map_err(CleanupFailure);
    match (result, cleanup) {
        (Err(cause), Err(cleanup)) => Err(cause.context(cleanup)),
        (Err(cause), Ok(())) => Err(cause),
        (Ok(()), Err(cleanup)) => Err(cleanup.into()),
        (Ok(()), Ok(())) => Ok(output),
    }
}

/// A build's own log can run long; cap what setup keeps on disk per step
/// rather than trusting cmake/make never to be noisy.
const LOG_CAP: usize = 4 * 1024 * 1024;

/// Copies `reader` into `file` up to `cap` bytes, then keeps draining
/// (discarding) so the child never blocks writing to a full pipe.
async fn drain_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    mut file: tokio::fs::File,
    cap: usize,
) -> anyhow::Result<()> {
    let mut buf = [0u8; 8192];
    let mut written = 0usize;
    loop {
        let read = reader.read(&mut buf).await?;
        if read == 0 {
            return Ok(());
        }
        if written >= cap {
            continue;
        }
        let take = (cap - written).min(read);
        file.write_all(&buf[..take]).await?;
        written += take;
        if written == cap {
            file.write_all(b"\n... [setup log truncated] ...\n").await?;
        }
    }
}

/// Runs a setup step whose stdout/stderr are only useful for diagnosis, not
/// for the UI: both are tee'd into `log`, capped, instead of held in
/// memory. Only `Control::Cancel` interrupts a step; a build has no
/// partial-stop concept the way a recording does.
pub(crate) async fn run_logged(
    mut command: Command,
    log: &std::fs::File,
    timeout: Duration,
    control: &mut watch::Receiver<Control>,
) -> anyhow::Result<()> {
    if *control.borrow() == Control::Cancel {
        bail!("cancelled");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        aether_spawn::detach_session(command.as_std_mut());
    }
    let child = command
        .spawn()
        .context("start local voice setup step; check that the tool is installed")?;
    let pid = child.id().context("local voice setup step has no pid")?;
    let mut group = Group {
        child,
        pid: Pid::from_raw(pid as i32),
        cleaned: false,
    };
    let stdout = group.child.stdout.take().context("missing setup stdout")?;
    let stderr = group.child.stderr.take().context("missing setup stderr")?;
    // Cloned fds share one offset, so both drains append in place, not overwrite.
    let out_file = tokio::fs::File::from_std(log.try_clone().context("duplicate install log")?);
    let err_file = tokio::fs::File::from_std(log.try_clone().context("duplicate install log")?);
    let result = {
        let io = async {
            let (_, _, ()) = tokio::try_join!(
                drain_capped(stdout, out_file, LOG_CAP),
                drain_capped(stderr, err_file, LOG_CAP),
                async {
                    let status = group.child.wait().await?;
                    if !status.success() {
                        bail!("setup step exited with {status}; see its log for details");
                    }
                    Ok(())
                },
            )?;
            Ok(())
        };
        tokio::select! {
            biased;
            result = io => result,
            _ = interrupted(control, false) => Err(anyhow::anyhow!("cancelled")),
            _ = tokio::time::sleep(timeout) => Err(anyhow::anyhow!("setup step timed out")),
        }
    };
    let cleanup = group.cleanup().await.map_err(CleanupFailure);
    match (result, cleanup) {
        (Err(cause), Err(cleanup)) => Err(cause.context(cleanup)),
        (Err(cause), Ok(())) => Err(cause),
        (Ok(()), Err(cleanup)) => Err(cleanup.into()),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod reply_recording_tests {
    use super::*;

    fn command(code: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", code]);
        command
    }

    #[tokio::test]
    async fn recording_progress_requires_audio_and_keeps_the_output_bound() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for (audio, limit, expected_calls, success) in [
            (b"".as_slice(), 4, 0, true),
            (b"abc".as_slice(), 3, 1, true),
            (b"abcde".as_slice(), 3, 1, false),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let callback = calls.clone();
            let mut bytes = Vec::new();
            let result = recording_read(
                audio,
                &mut bytes,
                limit,
                Some(Box::new(move || {
                    callback.fetch_add(1, Ordering::SeqCst);
                })),
            )
            .await;
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
            assert_eq!(result.is_ok(), success);
            assert!(bytes.len() <= limit + 1);
            if success {
                assert_eq!(bytes, audio);
            }
        }
    }

    #[tokio::test]
    async fn explicit_recording_progress_is_not_process_start_and_stop_keeps_pcm() {
        let (tx, mut rx) = watch::channel(Control::Run);
        let pcm = run_explicit_recording(
            command("printf audio; sleep 30"),
            1024,
            Duration::from_secs(3),
            &mut rx,
            move || {
                tx.send_replace(Control::Stop);
            },
        )
        .await
        .unwrap();
        assert!(!pcm.is_empty());
        assert!(b"audio".starts_with(&pcm));
        let (_tx, mut rx) = watch::channel(Control::Run);
        run_explicit_recording(
            command("exit 1"),
            1024,
            Duration::from_secs(3),
            &mut rx,
            || panic!("failed recorder produced no PCM"),
        )
        .await
        .unwrap_err();
    }

    #[tokio::test]
    async fn auto_reply_recording_requires_actual_explicit_stop_not_timeout_or_exit() {
        let (_tx, mut rx) = watch::channel(Control::Run);
        for (code, timeout, expected) in [
            ("printf audio", Duration::from_secs(3), "before stop"),
            (
                "sleep 30",
                Duration::from_millis(30),
                "without the reply shortcut",
            ),
        ] {
            let error = run_explicit_recording(command(code), 1024, timeout, &mut rx, || {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        // Reviewed dictation intentionally keeps its existing timeout behavior.
        assert!(
            run(
                command("sleep 30"),
                &[],
                1024,
                Duration::from_millis(30),
                &mut rx,
                true
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn auto_reply_recording_accepts_stop_but_never_cancel() {
        let (_tx, mut rx) = watch::channel(Control::Stop);
        assert!(
            run_explicit_recording(
                command("sleep 30"),
                1024,
                Duration::from_secs(3),
                &mut rx,
                || {}
            )
            .await
            .is_ok()
        );
        let (_tx, mut rx) = watch::channel(Control::Cancel);
        let error = run_explicit_recording(
            command("sleep 30"),
            1024,
            Duration::from_secs(3),
            &mut rx,
            || {},
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }
}
