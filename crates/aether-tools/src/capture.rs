//! Disk-backed shell output. Only a bounded page enters memory or model context.
use std::io;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::tool::{ToolContext, ToolError};

pub(crate) const PAGE_BYTES: usize = 24_000;
const CAPTURE_BYTES: u64 = 8 * 1024 * 1024;
/// Default cap for a foreground command's log; the artifact copy is bounded by it too.
pub(crate) const FOREGROUND_CAPTURE_BYTES: u64 = 256 * 1024 * 1024;
const TRUNCATED: &[u8] = b"\n[output truncated: capture byte limit reached]\n";

#[derive(Debug)]
pub(crate) struct Capture {
    pub path: PathBuf,
    directory: Option<tempfile::TempDir>,
    dropped: AtomicU64,
    limit: u64,
    reader: std::fs::File,
    writer: tokio::sync::Mutex<tokio::fs::File>,
    size: AtomicU64,
    error: Mutex<Option<String>>,
    pub failed: CancellationToken,
}

impl Capture {
    pub fn new(ctx: &ToolContext, limit: u64) -> Result<Arc<Self>, ToolError> {
        Self::create(ctx, false, limit)
    }

    pub fn background(ctx: &ToolContext) -> Result<Arc<Self>, ToolError> {
        Self::create(ctx, true, CAPTURE_BYTES)
    }

    fn create(ctx: &ToolContext, background: bool, limit: u64) -> Result<Arc<Self>, ToolError> {
        assert!(
            limit > TRUNCATED.len() as u64,
            "capture limit {limit} cannot hold the truncation marker"
        );
        // The agent temp dir keeps logs out of the worktree, where they would make a
        // finished workspace look dirty to GC. Without one, fall back to the cwd.
        let parent = match &ctx.temp_dir {
            Some(dir) => {
                aether_core::agent_temp::ensure_leaf(dir, &ctx.instance_id)
                    .map_err(|e| ToolError::Internal(format!("agent temp dir: {e}")))?;
                dir
            }
            None => &ctx.cwd,
        };
        // An exclusive private directory avoids overwriting user files or following
        // existing symlinks. Its `.gitignore` keeps the cwd fallback clean for git.
        let directory = tempfile::Builder::new()
            .prefix(".aether-shell-")
            .tempdir_in(parent)?;
        aether_core::private_file::write(&directory.path().join(".gitignore"), b"*\n")?;
        let path = directory.path().join("output.log");
        use std::os::unix::fs::OpenOptionsExt;
        let writer = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        let reader = writer.try_clone()?;
        let directory = if background {
            Some(directory)
        } else {
            let _ = directory.keep();
            None
        };
        Ok(Arc::new(Self {
            path,
            directory,
            dropped: AtomicU64::new(0),
            limit,
            reader,
            writer: tokio::sync::Mutex::new(tokio::fs::File::from_std(writer)),
            size: AtomicU64::new(0),
            error: Mutex::new(None),
            failed: CancellationToken::new(),
        }))
    }

    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> usize {
        usize::try_from(self.dropped.load(Ordering::Acquire)).unwrap_or(usize::MAX)
    }

    pub fn error(&self) -> Result<Option<String>, ToolError> {
        self.error
            .lock()
            .map(|e| e.clone())
            .map_err(|_| ToolError::Internal("shell output error state poisoned".into()))
    }

    pub fn fail(&self, error: impl std::fmt::Display) {
        match self.error.lock() {
            Ok(mut slot) => {
                slot.get_or_insert_with(|| error.to_string());
            }
            Err(_) => tracing::error!("shell output error state poisoned"),
        }
        self.failed.cancel();
    }

    async fn append(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().await;
        let available = self
            .limit
            .saturating_sub(TRUNCATED.len() as u64 + self.size());
        let kept = bytes.len().min(available as usize);
        writer.write_all(&bytes[..kept]).await?;
        let lost = bytes.len() - kept;
        let marker = lost > 0 && self.dropped.load(Ordering::Acquire) == 0;
        if marker {
            writer.write_all(TRUNCATED).await?;
        }
        // Publish only bytes already visible to concurrent page readers.
        writer.flush().await?;
        self.size.fetch_add(
            (kept + if marker { TRUNCATED.len() } else { 0 }) as u64,
            Ordering::Release,
        );
        self.dropped.fetch_add(lost as u64, Ordering::Release);
        Ok(())
    }

    pub fn page(&self, offset: u64, finished: bool) -> io::Result<(String, u64)> {
        let remaining = self.size().saturating_sub(offset);
        let mut bytes = vec![0; remaining.min(PAGE_BYTES as u64) as usize];
        self.reader.read_exact_at(&mut bytes, offset)?;
        let at_end = finished && remaining <= PAGE_BYTES as u64;
        let end = complete_utf8_end(&bytes, at_end);
        Ok((
            String::from_utf8_lossy(&bytes[..end]).into_owned(),
            offset + end as u64,
        ))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take()
            && let Err(error) = directory.close()
        {
            tracing::error!(path = %self.path.display(), %error, "cannot remove shell capture");
        }
    }
}

pub(crate) async fn pump(mut pipe: impl AsyncRead + Unpin, capture: Arc<Capture>, terminal: bool) {
    let mut chunk = [0; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => return,
            // Linux PTY masters signal slave closure with EIO, rather than EOF.
            Err(e) if terminal && e.raw_os_error() == Some(nix::libc::EIO) => return,
            Err(e) => {
                capture.fail(format!("reading command output: {e}"));
                return;
            }
            Ok(n) => {
                if let Err(e) = capture.append(&chunk[..n]).await {
                    capture.fail(format!("writing shell log {}: {e}", capture.path.display()));
                    return;
                }
            }
        }
    }
}

fn complete_utf8_end(bytes: &[u8], at_end: bool) -> usize {
    if at_end {
        return bytes.len();
    }
    // Walk past invalid sequences too: valid characters later in binary output
    // must not be split merely because an earlier byte was invalid UTF-8.
    let mut start = 0;
    while start < bytes.len() {
        match std::str::from_utf8(&bytes[start..]) {
            Ok(_) => return bytes.len(),
            Err(e) => match e.error_len() {
                Some(n) => start += e.valid_up_to() + n,
                None => return start + e.valid_up_to(),
            },
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_in;

    #[tokio::test]
    async fn capture_is_bounded_and_reports_discarded_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let log = Capture::background(&ctx_in(dir.path())).unwrap();
        let chunk = vec![b'x'; 8192];
        let produced = CAPTURE_BYTES as usize + chunk.len();
        for _ in 0..produced / chunk.len() {
            log.append(&chunk).await.unwrap();
        }
        assert_eq!(log.size(), CAPTURE_BYTES);
        assert_eq!(std::fs::metadata(&log.path).unwrap().len(), CAPTURE_BYTES);
        assert_eq!(
            log.dropped(),
            produced - (CAPTURE_BYTES as usize - TRUNCATED.len())
        );
        let (tail, _) = log
            .page(CAPTURE_BYTES - TRUNCATED.len() as u64, true)
            .unwrap();
        assert_eq!(tail.as_bytes(), TRUNCATED);
        assert!(!log.failed.is_cancelled());
        let path = log.path.parent().unwrap().to_owned();
        drop(log);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn foreground_capture_is_bounded_and_lives_in_the_agent_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("t");
        let mut ctx = ctx_in(dir.path());
        aether_core::agent_temp::prepare_root(&root, &ctx.instance_id).unwrap();
        ctx.temp_dir = Some(aether_core::agent_temp::leaf_path(&root, ctx.agent_id));
        let limit = 4096;
        let log = Capture::new(&ctx, limit).unwrap();
        assert!(log.path.starts_with(ctx.temp_dir.as_ref().unwrap()));
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".aether-shell-")
        }));
        let chunk = vec![b'x'; 1000];
        for _ in 0..8 {
            log.append(&chunk).await.unwrap();
        }
        assert_eq!(log.size(), limit);
        assert_eq!(std::fs::metadata(&log.path).unwrap().len(), limit);
        assert_eq!(log.dropped(), 8000 - (limit as usize - TRUNCATED.len()));
        let (tail, _) = log.page(limit - TRUNCATED.len() as u64, true).unwrap();
        assert_eq!(tail.as_bytes(), TRUNCATED);
    }

    #[tokio::test]
    async fn pages_preserve_unicode_across_boundaries_and_incomplete_writes() {
        let dir = tempfile::tempdir().unwrap();
        let log = Capture::new(&ctx_in(dir.path()), FOREGROUND_CAPTURE_BYTES).unwrap();
        let prefix = "x".repeat(PAGE_BYTES - 1);
        log.append(prefix.as_bytes()).await.unwrap();
        let crab = "🦀".as_bytes();
        log.append(&crab[..2]).await.unwrap();
        let (first, next) = log.page(0, false).unwrap();
        assert_eq!(first, prefix);
        assert_eq!(next, prefix.len() as u64);
        assert_eq!(log.page(next, false).unwrap(), (String::new(), next));
        log.append(&crab[2..]).await.unwrap();
        log.append(b"tail").await.unwrap();
        let (second, end) = log.page(next, true).unwrap();
        assert_eq!(second, "🦀tail");
        assert_eq!(end, log.size());
        assert_eq!(
            std::fs::read(&log.path).unwrap(),
            format!("{prefix}🦀tail").into_bytes()
        );
    }

    #[tokio::test]
    async fn log_write_failure_is_visible_and_signals_the_process_owner() {
        let dir = tempfile::tempdir().unwrap();
        let log = Capture::new(&ctx_in(dir.path()), FOREGROUND_CAPTURE_BYTES).unwrap();
        // A read-only handle deterministically fails writes without relying on disk space.
        *log.writer.lock().await =
            tokio::fs::File::from_std(std::fs::File::open(&log.path).unwrap());
        pump(&b"data"[..], log.clone(), false).await;
        assert!(log.failed.is_cancelled());
        assert!(log.error().unwrap().unwrap().contains("writing shell log"));
        assert_eq!(log.size(), 0);
    }
}
