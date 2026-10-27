//! New turns wait while the filesystem holding the store runs low: a turn
//! started then is likely to have its writes refused.

use std::{path::Path, sync::Arc, time::Duration};

use aether_core::disk::LowDisk;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::time::now;
use aether_store::{Commit, Mutation};
use parking_lot::Mutex;

use crate::{Runtime, config::DiskGuard};

const INTERVAL: Duration = Duration::from_secs(30);

const DISK_LOW: &str = "disk.low";
const DISK_RECOVERED: &str = "disk.recovered";
/// The open `disk.low` payload, or empty once recovered; written with the
/// event so startup need not search the log for it.
const OPEN_LOW_META: &str = "disk.open_low";

/// What the guard has seen, and what the event log has yet to hear of it.
#[derive(Default)]
pub(crate) struct DiskState {
    /// Set while new turns wait.
    low: Option<LowDisk>,
    /// The latest change not yet recorded: a disk this low may refuse it.
    unrecorded: Option<EventDraft>,
    /// The store refused `unrecorded` already, and the log said so.
    refusal_logged: bool,
    /// The last measurement failed; logged once per streak.
    unmeasured: bool,
}

impl DiskState {
    /// Fold in one measurement. True when it releases held turns.
    fn observe(&mut self, path: &Path, floor: u64, available: u64) -> bool {
        self.unmeasured = false;
        let path = path.display().to_string();
        match &mut self.low {
            None if available < floor => {
                tracing::warn!(
                    %path,
                    available_bytes = available,
                    floor_bytes = floor,
                    "disk is low; new turns wait until it has room"
                );
                let since = now();
                let payload = serde_json::json!({
                    "path": path,
                    "available_bytes": available,
                    "floor_bytes": floor,
                    "since": since,
                });
                self.low = Some(LowDisk {
                    path,
                    available_bytes: available,
                    floor_bytes: floor,
                    since,
                });
                self.unrecorded(DISK_LOW, payload);
                false
            }
            // A tenth of the floor to spare, so a disk hovering at it does
            // not flap new turns and events.
            Some(low) if available >= floor.saturating_add(floor / 10) => {
                tracing::info!(
                    %path,
                    available_bytes = available,
                    floor_bytes = floor,
                    "disk has room again; new turns start"
                );
                let payload = serde_json::json!({
                    "path": path,
                    "available_bytes": available,
                    "floor_bytes": floor,
                    "low_since": low.since,
                });
                self.low = None;
                self.unrecorded(DISK_RECOVERED, payload);
                true
            }
            Some(low) => {
                low.available_bytes = available;
                false
            }
            None => false,
        }
    }

    /// Replaces any change still unrecorded: the log only needs the latest,
    /// and a recovery carries when the low began.
    fn unrecorded(&mut self, name: &str, payload: serde_json::Value) {
        self.unrecorded = Some(EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: name.into(),
                payload,
            },
        ));
        self.refusal_logged = false;
    }
}

impl Runtime {
    /// Set while the store's filesystem is below its floor.
    pub(crate) fn low_disk(&self) -> Option<LowDisk> {
        self.disk.lock().low.clone()
    }

    pub(crate) fn disk_is_low(&self) -> bool {
        self.disk.lock().low.is_some()
    }
}

/// The low the log last announced without a recovery after it: the daemon
/// that announced it may have stopped before the disk recovered.
fn unrecovered_low(runtime: &Runtime) -> Option<LowDisk> {
    let recorded = match runtime.store.meta(OPEN_LOW_META) {
        Ok(Some(recorded)) => recorded,
        Ok(None) => return unrecovered_low_in_log(runtime),
        Err(error) => {
            tracing::warn!(%error, "could not read the disk's last recorded state");
            return None;
        }
    };
    if recorded.is_empty() {
        return None;
    }
    serde_json::from_str(&recorded)
        .map_err(|error| tracing::warn!(%error, "ignoring an unreadable open disk low"))
        .ok()
}

/// A store from before [`OPEN_LOW_META`]: find the state in the log once,
/// and keep it there for the next start.
fn unrecovered_low_in_log(runtime: &Runtime) -> Option<LowDisk> {
    let last = match runtime
        .store
        .latest_custom_event(&[DISK_LOW, DISK_RECOVERED])
    {
        Ok(last) => last,
        Err(error) => {
            tracing::warn!(%error, "could not read the disk's last recorded state");
            return None;
        }
    };
    let open = match last.map(|event| event.kind) {
        Some(EventKind::Custom { name, payload }) if name == DISK_LOW => Some(payload),
        other => {
            debug_assert!(
                other.as_ref().is_none_or(
                    |kind| matches!(kind, EventKind::Custom { name, .. } if name == DISK_RECOVERED)
                ),
                "the query asks for disk.low and disk.recovered only"
            );
            None
        }
    };
    let kept = runtime.apply(Commit::new().mutate(Mutation::SetMeta {
        key: OPEN_LOW_META.into(),
        value: open.as_ref().map(ToString::to_string).unwrap_or_default(),
    }));
    if let Err(error) = kept {
        tracing::warn!(%error, "could not keep the disk's last recorded state; the next start searches again");
    }
    serde_json::from_value(open?)
        .map_err(|error| tracing::warn!(%error, "ignoring an unreadable disk.low event"))
        .ok()
}

/// A low from before the restart is held until the first check clears it,
/// which then records the recovery.
fn resume_low(runtime: &Runtime) {
    if let Some(low) = unrecovered_low(runtime) {
        runtime.disk.lock().low = Some(low);
    }
}

/// A disk with no floor is never low: close a low left open by a daemon
/// that had one.
fn close_unrecovered_low(runtime: &Runtime) {
    let Some(low) = unrecovered_low(runtime) else {
        return;
    };
    tracing::info!(path = %low.path, "no disk floor is set; recording the earlier low as over");
    let mut state = runtime.disk.lock();
    state.unrecorded(
        DISK_RECOVERED,
        serde_json::json!({"path": low.path, "low_since": low.since}),
    );
    drop(state);
    record(&runtime.disk, |commit| runtime.apply(commit).map(drop));
}

/// Measure once now, so a daemon starting on a full disk starts no turn,
/// then every [`INTERVAL`].
pub(crate) fn spawn(runtime: Arc<Runtime>) {
    let Some(guard) = runtime.config.disk_guard.clone() else {
        close_unrecovered_low(&runtime);
        return;
    };
    resume_low(&runtime);
    if check(&runtime, &guard) {
        let evicting = runtime.clone();
        let reporting = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) =
                tokio::task::spawn_blocking(move || evicting.evict_build_cache()).await
            {
                reporting.report_problem("disk.startup".into(), format!("Startup cache eviction failed: {error}; maintenance will retry. Run `aether doctor` and `aether diagnose`."));
            }
        });
    }
    let supervising = runtime.clone();
    tokio::spawn(async move {
        crate::supervision::isolate(
            "disk guard",
            supervising.shutdown.clone(),
            false,
            |detail| supervising.report_problem("background.disk".into(), detail),
            run(runtime, guard),
        )
        .await;
    });
}

async fn run(runtime: Arc<Runtime>, guard: DiskGuard) {
    loop {
        tokio::select! {
            biased;
            _ = runtime.shutdown.cancelled() => return,
            _ = tokio::time::sleep(INTERVAL) => {}
        }
        let (checking, guard) = (runtime.clone(), guard.clone());
        // statvfs can hang on a stuck mount, and recording is a SQLite
        // write; neither belongs on an async worker.
        let checked = tokio::task::spawn_blocking(move || {
            if check(&checking, &guard) {
                checking.evict_build_cache();
            }
        });
        if let Err(error) = checked.await {
            runtime.report_problem("disk.check".into(), format!("Disk check failed: {error}; it will retry in 30 seconds. Run `aether doctor` and `aether diagnose`."));
        } else {
            runtime.clear_problem("disk.check");
        }
    }
}

/// Measure and observe once. True when the disk has just turned low, so the
/// build cache is worth evicting now rather than at the next sweep.
fn check(runtime: &Runtime, guard: &DiskGuard) -> bool {
    let measured = fs4::available_space(&guard.path)
        .and_then(|available| Ok((available, fs4::total_space(&guard.path)?)));
    match measured {
        Ok((available, total)) => {
            let was_low = runtime.disk_is_low();
            observe(runtime, &guard.path, guard.floor.bytes(total), available);
            !was_low && runtime.disk_is_low()
        }
        Err(error) => {
            let mut state = runtime.disk.lock();
            if !std::mem::replace(&mut state.unmeasured, true) {
                tracing::warn!(
                    path = %guard.path.display(),
                    %error,
                    "cannot measure free space; new turns keep the last disk state"
                );
            }
            false
        }
    }
}

/// Take one measurement: hold or release new turns, and record the change.
/// Called by one task at a time.
pub(crate) fn observe(runtime: &Runtime, path: &Path, floor: u64, available: u64) {
    if runtime.disk.lock().observe(path, floor, available) {
        runtime.notify.notify_one();
    }
    record(&runtime.disk, |commit| runtime.apply(commit).map(drop));
}

/// Commit the unrecorded change, if any; one the store refuses is kept for
/// the next check.
fn record(disk: &Mutex<DiskState>, apply: impl FnOnce(Commit) -> crate::Result<()>) {
    let Some(unrecorded) = disk.lock().unrecorded.clone() else {
        return;
    };
    let open_low = match &unrecorded.kind {
        EventKind::Custom { name, payload } if name == DISK_LOW => payload.to_string(),
        kind => {
            debug_assert!(
                matches!(kind, EventKind::Custom { name, .. } if name == DISK_RECOVERED),
                "the guard records disk.low and disk.recovered only"
            );
            String::new()
        }
    };
    // Not held across the commit: `status` reads this state.
    let recorded = apply(Commit::new().event(unrecorded).mutate(Mutation::SetMeta {
        key: OPEN_LOW_META.into(),
        value: open_low,
    }));
    let mut state = disk.lock();
    match recorded {
        Ok(()) => state.unrecorded = None,
        Err(error) => {
            if !std::mem::replace(&mut state.refusal_logged, true) {
                tracing::warn!(%error, "could not record the disk's state; retrying each check");
            }
        }
    }
}

#[cfg(test)]
mod tests;
