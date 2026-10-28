//! Whether the daemon's last run ended in a clean shutdown.
//!
//! A daemon marks its store as running when it starts and removes the mark
//! when it stops cleanly. A start that finds the mark left behind knows the
//! run before it was killed, crashed or lost power, and records that for
//! `aether doctor` until the next clean stop.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{read_bounded, resolved_beside};
use crate::time::{Timestamp, now};

const RUNNING: &str = ".running.json";
const UNCLEAN: &str = ".unclean.json";

/// A daemon run, as its start recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonRun {
    pub pid: u32,
    pub started_at: Timestamp,
}

/// A run that ended without a clean shutdown, found by the start after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncleanShutdown {
    /// `None` when the run's mark could not be read.
    pub run: Option<DaemonRun>,
    pub found_at: Timestamp,
}

impl UncleanShutdown {
    pub fn summary(&self) -> String {
        let run = match &self.run {
            Some(run) => format!(
                "The daemon started at {} (pid {})",
                run.started_at.to_rfc3339(),
                run.pid
            ),
            None => "A daemon run".to_owned(),
        };
        format!(
            "{run} stopped without a clean shutdown: it was killed, crashed or lost power. \
             The start at {} recovered its work.",
            self.found_at.to_rfc3339()
        )
    }
}

/// Mark `store` as run by `pid`, and record whether the run before ended
/// cleanly. Called under the store lock, before recovery runs.
pub fn begin(store: &Path, pid: u32) -> io::Result<Option<UncleanShutdown>> {
    assert!(pid > 0, "a daemon has a process id");
    let running = resolved_beside(store, RUNNING)?;
    let unclean_path = resolved_beside(store, UNCLEAN)?;
    let left_behind = match read_json::<DaemonRun>(&running) {
        Ok(None) => None,
        Ok(Some(run)) => Some(Some(run)),
        // A mark cut short by the same power loss still says the run ended badly.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
            ) =>
        {
            Some(None)
        }
        Err(error) => return Err(error),
    };
    let unclean = left_behind.map(|run| UncleanShutdown {
        run,
        found_at: now(),
    });
    match &unclean {
        Some(found) => write_json(&unclean_path, found)?,
        None => remove(&unclean_path)?,
    }
    write_json(
        &running,
        &DaemonRun {
            pid,
            started_at: now(),
        },
    )?;
    debug_assert!(unclean.is_some() == unclean_path.exists());
    Ok(unclean)
}

/// The daemon stopped cleanly: nothing is left to warn about.
pub fn end(store: &Path) -> io::Result<()> {
    remove(&resolved_beside(store, RUNNING)?)?;
    remove(&resolved_beside(store, UNCLEAN)?)
}

/// The run a start found ended uncleanly, until the next clean stop.
pub fn unclean(store: &Path) -> io::Result<Option<UncleanShutdown>> {
    read_json(&resolved_beside(store, UNCLEAN)?)
}

/// The run that marked `store` and has not stopped cleanly: the live daemon,
/// or one that died since, when none answers.
pub fn running(store: &Path) -> io::Result<Option<DaemonRun>> {
    read_json(&resolved_beside(store, RUNNING)?)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    let Some(bytes) = read_bounded(path)? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    debug_assert!(bytes.len() <= super::MAX_RECORD_BYTES);
    crate::private_file::write(path, &bytes)
}

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_start_after_a_kill_finds_the_unclean_run_and_a_clean_stop_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("aether.db");
        assert_eq!(begin(&store, 41).unwrap(), None, "a first start is clean");
        assert_eq!(running(&store).unwrap().map(|r| r.pid), Some(41));
        // Killed: no `end`. The next start finds the mark.
        let found = begin(&store, 42).unwrap().expect("unclean");
        assert_eq!(found.run.as_ref().map(|r| r.pid), Some(41));
        assert_eq!(unclean(&store).unwrap(), Some(found.clone()));
        assert!(found.summary().contains("pid 41"), "{}", found.summary());
        end(&store).unwrap();
        assert_eq!(unclean(&store).unwrap(), None);
        assert_eq!(running(&store).unwrap(), None);
        assert_eq!(
            begin(&store, 43).unwrap(),
            None,
            "the stop before was clean"
        );
        assert_eq!(unclean(&store).unwrap(), None);
    }

    #[test]
    fn a_torn_running_mark_still_counts_as_an_unclean_end() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("aether.db");
        std::fs::write(super::super::beside(&store, RUNNING), b"{\"pid\":").unwrap();
        let found = begin(&store, 7).unwrap().expect("unclean");
        assert_eq!(found.run, None);
        assert!(found.summary().starts_with("A daemon run"));
        assert_eq!(running(&store).unwrap().map(|r| r.pid), Some(7));
    }
}
