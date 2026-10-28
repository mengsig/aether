//! WAL checkpoints on their own connection and thread.
//!
//! SQLite's automatic checkpoint runs inside the COMMIT that crosses its
//! threshold: on the store's one connection, under its mutex and the
//! runtime's write gate. Its fsyncs took 15-30 s on a contended disk, and every
//! request queued behind them. A passive checkpoint from a second connection
//! blocks neither the writer nor readers, so the store turns the automatic
//! one off and runs this instead.

use super::change_watch::ChangeLog;
use crate::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

/// Often enough that the WAL stays a few MB at the daemon's write rate. An
/// interval with nothing new to copy costs a shared-memory read and no I/O.
pub(super) const INTERVAL: Duration = Duration::from_secs(5);

/// Passes run back to back while commits keep landing mid-pass. The WAL only
/// restarts after a pass no commit overlapped, and each retry copies less.
const CHASES: u32 = 8;

/// A WAL this long and still behind means passes keep falling short. 64 MiB
/// at 4 KiB pages.
const LONG_WAL_FRAMES: i64 = 16_384;

/// A fully copied WAL above this many frames (8 MiB at 4 KiB pages) is
/// truncated: a passive pass reuses the file but never shrinks it, so one
/// large write would leave its peak size on disk for good. Not while a history
/// screen is open: to it a truncation looks like another connection's commit.
const TRUNCATE_FRAMES: i64 = 2_048;

/// Stops and joins the checkpoint thread on drop.
pub(super) struct Checkpointer {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Checkpointer {
    /// Start checkpointing the WAL database at `path` every `interval`. The
    /// database must already exist and be in WAL mode.
    pub(super) fn start(path: &Path, interval: Duration, screens: Arc<ChangeLog>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Truncating takes the writer lock: never wait for it, so a reader
        // that still holds the WAL costs one retry and not a stalled commit.
        conn.busy_timeout(Duration::ZERO)?;
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("sqlite-checkpoint".into())
            .spawn(move || run(&conn, &stopped, interval, &screens))?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("WAL checkpoint thread panicked");
        }
    }
}

/// Pass every `interval`, chasing commits that overlap a pass, until stopped.
fn run(conn: &Connection, stopped: &mpsc::Receiver<()>, interval: Duration, screens: &ChangeLog) {
    let mut wait = interval;
    let mut chases = 0;
    let mut last_frames = -1;
    let mut warned = false;
    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(wait) {
        wait = interval;
        // A failed pass leaves the WAL longer, not wrong: the next
        // one copies what this one did not.
        let wal = match checkpoint(conn) {
            Ok(wal) => wal,
            Err(error) => {
                tracing::warn!(%error, "WAL checkpoint failed; retrying next interval");
                continue;
            }
        };
        // A pass reports the WAL as it was when the pass began, so
        // only an unchanged length shows no commit overlapped the last one.
        let overlapped = wal.frames != last_frames;
        last_frames = wal.frames;
        if overlapped && chases < CHASES {
            chases += 1;
            wait = Duration::ZERO;
            continue;
        }
        chases = 0;
        if wal.copied >= wal.frames && wal.frames > TRUNCATE_FRAMES {
            if !screens.watching() {
                truncate_wal(conn);
            }
            continue;
        }
        let behind = overlapped || wal.copied < wal.frames;
        let long = behind && wal.frames > LONG_WAL_FRAMES;
        if long && !warned {
            tracing::warn!(
                wal_frames = wal.frames,
                uncopied = wal.frames - wal.copied,
                "WAL keeps growing: checkpoint passes cannot finish between commits"
            );
        }
        warned = long;
    }
}

/// WAL length when a pass began and how much of it is copied, in frames.
struct Wal {
    frames: i64,
    copied: i64,
}

/// Copy every WAL frame no reader still needs into the database file.
fn checkpoint(conn: &Connection) -> Result<Wal> {
    let wal = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
        Ok(Wal {
            frames: row.get(1)?,
            copied: row.get(2)?,
        })
    })?;
    Ok(wal)
}

/// Shrink the fully copied WAL file to nothing. The connection has no busy
/// timeout, so it gives up at once when a writer or reader is in the way; the
/// WAL stays as long as it is and the next pass tries again.
fn truncate_wal(conn: &Connection) {
    let result = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        row.get::<_, i64>(0)
    });
    match result {
        Ok(0) => {}
        Ok(_) => tracing::debug!("WAL truncate found the database busy; retrying next interval"),
        Err(error) => tracing::warn!(%error, "WAL truncate failed; retrying next interval"),
    }
}
