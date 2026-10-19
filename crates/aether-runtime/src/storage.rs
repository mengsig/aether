//! Bounded, observable filesystem maintenance.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use aether_core::id::AgentId;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;

use crate::{Result, Runtime, RuntimeError};

const MAX_ENTRIES: usize = 10_000;

mod agent_temp;
mod build_cache;
mod engine_sessions;

#[derive(Debug, Clone)]
pub struct StoragePaths {
    pub store: PathBuf,
    pub logs: PathBuf,
}

#[derive(Debug, Default, Serialize)]
struct FileReport {
    bytes: u64,
    bytes_eligible: u64,
    bytes_removed: u64,
    truncated: bool,
    entries: Vec<FileDecision>,
}

#[derive(Debug, Serialize)]
struct FileDecision {
    path: PathBuf,
    bytes: u64,
    action: &'static str,
    reason: &'static str,
}

fn invalid(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Invalid(format!("storage maintenance: {error}"))
}

impl Runtime {
    /// A preview uses exactly the cleanup checks but never mutates files.
    /// Reports are bounded inventories, not a reservation against later writes.
    pub fn storage_gc(&self, dry_run: bool) -> Result<Value> {
        self.storage_gc_with(dry_run, false)
    }

    /// `prune_build_output` also clears build output from finished workspaces
    /// that are kept for another reason.
    pub fn storage_gc_with(&self, dry_run: bool, prune_build_output: bool) -> Result<Value> {
        let _guard = if dry_run {
            None
        } else {
            Some(
                self.storage_gc_lock
                    .try_lock()
                    .ok_or_else(|| invalid("another storage sweep is running"))?,
            )
        };
        let config = &self.models.config().storage;
        let grace = Duration::from_secs(u64::from(config.workspace_grace_seconds));
        let workspaces = if config.days(config.workspaces_days) == 0 {
            json!({"disabled": true, "reason": "workspace cleanup disabled"})
        } else if prune_build_output {
            serde_json::to_value(self.gc_workspaces_pruning_build_output(grace, dry_run)?)
                .map_err(invalid)?
        } else {
            serde_json::to_value(self.gc_workspaces(grace, dry_run)?).map_err(invalid)?
        };
        let mut report = json!({
            "at": aether_core::time::now(), "dry_run": dry_run,
            "workspaces": workspaces,
            "engine_sessions": self.gc_engine_sessions(config.days(config.engine_sessions_days), SystemTime::now(), dry_run)?,
            "artifacts": {"policy": "existing payload retention; storage.artifacts_days overrides age",
                "retention_days": self.config.retention.max_age_days, "max_total_bytes": self.config.retention.max_total_bytes},
            "event_history": {"policy": "existing completed-run retention; storage.event_history_days overrides age",
                "retention_days": config.event_history_days.unwrap_or(self.models.config().retention.completed_run_days.get())},
        });
        if let Some(paths) = &self.config.storage_paths {
            let root = paths
                .store
                .parent()
                .ok_or_else(|| invalid("store has no parent"))?;
            report["backups"] = serde_json::to_value(
                prune_backups(
                    &root.join("migration-backups"),
                    BackupRetention {
                        days: config.days(config.backups_days),
                        keep: usize::from(config.backups_keep.get()),
                        cap: usize::from(config.migration_backups_keep.get()),
                    },
                    SystemTime::now(),
                    dry_run,
                )
                .map_err(invalid)?,
            )
            .map_err(invalid)?;
            if let Some(dir) = aether_core::bridge_bin::default_dir(&paths.store) {
                report["bridge_copies"] =
                    serde_json::to_value(sweep_bridge_copies(&dir, dry_run).map_err(invalid)?)
                        .map_err(invalid)?;
            }
            report["logs"] =
                serde_json::to_value(inventory(&paths.logs).map_err(invalid)?).map_err(invalid)?;
            report["database_bytes"] = json!(fs::metadata(&paths.store).map_err(invalid)?.len());
            report["available_bytes"] = json!(fs4::available_space(root).map_err(invalid)?);
        }
        if let Some(root) = &self.config.agent_temp_root {
            report["agent_temp"] = serde_json::to_value(agent_temp::report(root).map_err(invalid)?)
                .map_err(invalid)?;
        }
        if let Some(root) = &self.config.build_cache_root {
            report["build_cache"] =
                serde_json::to_value(self.gc_build_cache(root, dry_run)?).map_err(invalid)?;
        }
        let artifacts = self.store.live_artifacts()?;
        let mut digests = std::collections::HashSet::new();
        report["artifacts"]["bytes"] = json!(
            artifacts
                .iter()
                .filter(|a| {
                    digests.insert(a.digest.clone().unwrap_or_else(|| a.id.to_string()))
                })
                .map(|a| a.size_bytes)
                .sum::<u64>()
        );
        let previous = self.storage_last_run.lock().clone();
        if !dry_run {
            *self.storage_last_run.lock() = Some(report.clone());
        }
        report["last_gc"] = json!(previous);
        Ok(report)
    }
}

impl Runtime {
    /// Agents with a turn, retake, lease or resource in flight, read under
    /// the scheduler and engine-turn locks the caller holds.
    fn in_flight_agents(
        &self,
        sched: &crate::SchedulerState,
        engine_turns: &std::collections::HashMap<AgentId, crate::turn_engine::EngineTurnHandle>,
    ) -> Result<HashSet<AgentId>> {
        let leases = self.store.leases()?;
        let resources = self.resources.holdings();
        Ok(sched
            .running
            .keys()
            .chain(&sched.engine_retakers)
            .chain(engine_turns.keys())
            .copied()
            .chain(leases.iter().map(|lease| lease.agent_id))
            .chain(resources.iter().map(|holding| holding.agent))
            .collect())
    }
}

/// Removals are worth a line each; the files a pass keeps are only debug detail.
fn log_decision(message: &str, path: &Path, action: &str, reason: &str, bytes: u64) {
    if action == "kept" {
        tracing::debug!(path = %path.display(), action, reason, bytes, "{message}");
    } else {
        tracing::info!(path = %path.display(), action, reason, bytes, "{message}");
    }
}

/// One line per pass, so a pass that keeps everything stays a single entry.
fn log_summary(pass: &str, report: &FileReport) {
    let kept = report.entries.iter().filter(|e| e.action == "kept").count();
    tracing::info!(
        kept,
        changed = report.entries.len() - kept,
        bytes = report.bytes,
        bytes_removed = report.bytes_removed,
        "{pass} pass finished"
    );
}

fn read_directory(path: &Path) -> io::Result<Option<fs::ReadDir>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path).map(Some),
        Ok(_) => Err(io::Error::other(format!(
            "{} is not a real directory",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn backup_name(name: &str) -> bool {
    let Some((from, rest)) = name
        .strip_prefix("schema-")
        .and_then(|s| s.split_once("-before-"))
    else {
        return false;
    };
    let Some((to, id)) = rest.split_once('-') else {
        return false;
    };
    from.parse::<u32>().is_ok()
        && to.parse::<u32>().is_ok()
        && id
            .strip_suffix(".sqlite.zst")
            .or_else(|| id.strip_suffix(".sqlite"))
            .is_some_and(|id| id.parse::<ulid::Ulid>().is_ok())
}

/// Keep the newest `keep` or anything younger than `days` (zero: no age
/// cleanup), but never more than the newest `cap`.
#[derive(Debug, Clone, Copy)]
struct BackupRetention {
    days: u16,
    keep: usize,
    cap: usize,
}

fn prune_backups(
    root: &Path,
    retention: BackupRetention,
    now: SystemTime,
    dry_run: bool,
) -> io::Result<FileReport> {
    let BackupRetention { days, keep, cap } = retention;
    assert!(cap > 0, "the backup cap must keep the newest backup");
    let mut report = FileReport::default();
    let Some(entries) = read_directory(root)? else {
        return Ok(report);
    };
    let mut files = Vec::new();
    for entry in entries.take(MAX_ENTRIES + 1) {
        let entry = entry?;
        if files.len() == MAX_ENTRIES {
            return Err(io::Error::other(
                "backup directory exceeds scan limit; no backups deleted",
            ));
        }
        let meta = fs::symlink_metadata(entry.path())?;
        files.push((entry.path(), meta));
    }
    // Read and validate every modification time before deleting any file.
    let mut files = files
        .into_iter()
        .map(|(path, meta)| {
            let modified = meta.modified()?;
            Ok((path, meta, modified))
        })
        .collect::<io::Result<Vec<_>>>()?;
    files.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let mut backups = 0;
    for (path, meta, modified) in files {
        let managed = meta.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(backup_name);
        let rank = backups;
        if managed {
            backups += 1;
        }
        let old = now
            .duration_since(modified)
            .is_ok_and(|age| age >= Duration::from_secs(u64::from(days) * 86400));
        let over_cap = managed && rank >= cap;
        let eligible = over_cap || (managed && days > 0 && rank >= keep && old);
        let reason = if !managed {
            "unrecognized file retained"
        } else if over_cap {
            "beyond the newest migration_backups_keep backups"
        } else if days == 0 {
            "backup age cleanup disabled"
        } else if rank < keep {
            "latest backups retained"
        } else if !old {
            "within retention"
        } else {
            "older than retention and outside latest backups"
        };
        report.bytes += meta.len();
        let action = if eligible {
            report.bytes_eligible += meta.len();
            if dry_run {
                "would_remove"
            } else {
                fs::remove_file(&path)?;
                report.bytes_removed += meta.len();
                "removed"
            }
        } else {
            "kept"
        };
        log_decision(
            "backup retention decision",
            &path,
            action,
            reason,
            meta.len(),
        );
        report.entries.push(FileDecision {
            path,
            bytes: meta.len(),
            action,
            reason,
        });
    }
    debug_assert!(backups <= cap || report.bytes_eligible > 0);
    log_summary("backup retention", &report);
    Ok(report)
}

/// Daemons' copies of their own build, removed once their daemon is gone.
fn sweep_bridge_copies(dir: &Path, dry_run: bool) -> io::Result<FileReport> {
    let mut report = FileReport::default();
    for file in aether_core::bridge_bin::sweep(dir, dry_run)? {
        log_decision(
            "bridge copy decision",
            &file.path,
            file.action.as_str(),
            file.reason,
            file.bytes,
        );
        report.bytes += file.bytes;
        match file.action {
            aether_core::bridge_bin::SweepAction::Kept => {}
            aether_core::bridge_bin::SweepAction::WouldRemove => {
                report.bytes_eligible += file.bytes
            }
            aether_core::bridge_bin::SweepAction::Removed => {
                report.bytes_eligible += file.bytes;
                report.bytes_removed += file.bytes;
            }
        }
        report.entries.push(FileDecision {
            path: file.path,
            bytes: file.bytes,
            action: file.action.as_str(),
            reason: file.reason,
        });
    }
    debug_assert!(
        report.bytes_removed <= report.bytes_eligible && report.bytes_eligible <= report.bytes
    );
    log_summary("bridge copy", &report);
    Ok(report)
}

/// Never follows symlinks; stop at a fixed number of directory entries.
fn inventory(root: &Path) -> io::Result<FileReport> {
    let mut report = FileReport::default();
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(path) = pending.pop() {
        let Some(entries) = read_directory(&path)? else {
            continue;
        };
        for entry in entries {
            if count == MAX_ENTRIES {
                report.truncated = true;
                return Ok(report);
            }
            count += 1;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                report.bytes += meta.len();
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn retention(days: u16, keep: usize, cap: usize) -> BackupRetention {
        BackupRetention { days, keep, cap }
    }
    fn backup(root: &Path, seconds: u64) -> PathBuf {
        let path = root.join(format!(
            "schema-1-before-2-{}.sqlite.zst",
            ulid::Ulid::generate()
        ));
        fs::write(&path, b"backup").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
        path
    }
    /// Lines logged at INFO and above while `run` executes.
    fn info_lines(run: impl FnOnce()) -> Vec<String> {
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        // Concurrent scoped subscribers race on tracing's callsite filtering.
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            // Callsites first hit by a concurrent test cached "disabled".
            tracing_core::callsite::rebuild_interest_cache();
            run();
        });
        let output = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        output.lines().map(str::to_owned).collect()
    }
    #[test]
    fn a_pass_that_keeps_every_backup_logs_one_info_line() {
        let dir = tempfile::tempdir().unwrap();
        for seconds in 0..3 {
            backup(dir.path(), 86400 + seconds);
        }
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86400 + 10);
        let lines = info_lines(|| {
            let report = prune_backups(dir.path(), retention(14, 1, 10), now, false).unwrap();
            assert_eq!(report.bytes_removed, 0);
        });
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("kept=3"), "{lines:?}");
    }
    #[test]
    fn a_pass_that_removes_backups_logs_each_removal() {
        let dir = tempfile::tempdir().unwrap();
        let old = [backup(dir.path(), 0), backup(dir.path(), 1)];
        backup(dir.path(), 86400 * 5);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86400 * 5);
        let lines = info_lines(|| {
            prune_backups(dir.path(), retention(1, 1, 10), now, false).unwrap();
        });
        let removals: Vec<_> = lines
            .iter()
            .filter(|line| line.contains("backup retention decision"))
            .collect();
        assert_eq!(removals.len(), 2, "{lines:?}");
        for path in &old {
            assert!(removals.iter().any(|l| l.contains(path.to_str().unwrap())));
        }
        assert!(lines.iter().any(|l| l.contains("pass finished")));
    }
    #[test]
    fn backups_keep_latest_or_recent_and_preview_matches_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let old = backup(dir.path(), 0);
        let boundary = backup(dir.path(), 86400);
        let recent = backup(dir.path(), 86401);
        let newest = backup(dir.path(), 86402);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2 * 86400);
        let preview = prune_backups(dir.path(), retention(1, 1, 10), now, true).unwrap();
        assert_eq!(preview.bytes_eligible, 12);
        assert!(old.exists() && boundary.exists());
        let actual = prune_backups(dir.path(), retention(1, 1, 10), now, false).unwrap();
        assert_eq!(preview.bytes_eligible, actual.bytes_removed);
        assert!(!old.exists() && !boundary.exists());
        assert!(recent.exists() && newest.exists());
    }
    #[test]
    fn three_recent_migrations_with_a_cap_of_two_lose_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86400);
        let oldest = backup(dir.path(), 9 * 86400);
        let middle = backup(dir.path(), 9 * 86400 + 1);
        let newest = backup(dir.path(), 9 * 86400 + 2);
        let legacy = dir.path().join(format!(
            "schema-1-before-2-{}.sqlite",
            ulid::Ulid::generate()
        ));
        fs::copy(&oldest, &legacy).unwrap();
        fs::File::options()
            .write(true)
            .open(&legacy)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(8 * 86400))
            .unwrap();
        // Every backup is within the 14-day window and `keep` would hold three.
        let report = prune_backups(dir.path(), retention(14, 3, 2), now, false).unwrap();
        assert_eq!(report.bytes, 24);
        assert_eq!(report.bytes_removed, 12);
        assert!(!oldest.exists() && !legacy.exists());
        assert!(middle.exists() && newest.exists());
        assert_eq!(
            prune_backups(dir.path(), retention(14, 3, 2), now, false)
                .unwrap()
                .bytes_removed,
            0
        );
    }
    #[test]
    fn the_cap_applies_when_age_cleanup_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let oldest = backup(dir.path(), 0);
        let middle = backup(dir.path(), 1);
        let newest = backup(dir.path(), 2);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86400);
        let report = prune_backups(dir.path(), retention(0, 3, 2), now, false).unwrap();
        assert_eq!((report.bytes, report.bytes_removed), (18, 6));
        assert!(!oldest.exists());
        assert!(middle.exists() && newest.exists());
    }
    #[test]
    fn disabled_unrecognized_and_symlink_files_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let old = backup(dir.path(), 0);
        fs::write(dir.path().join("personal.sqlite"), b"mine").unwrap();
        assert_eq!(
            prune_backups(dir.path(), retention(0, 0, 10), SystemTime::now(), false)
                .unwrap()
                .bytes_removed,
            0
        );
        let outside = tempfile::tempdir().unwrap();
        let target = backup(outside.path(), 0);
        std::os::unix::fs::symlink(&target, dir.path().join(target.file_name().unwrap())).unwrap();
        prune_backups(dir.path(), retention(1, 0, 10), SystemTime::now(), false).unwrap();
        assert!(!old.exists());
        assert!(target.exists() && dir.path().join("personal.sqlite").exists());
    }
}
