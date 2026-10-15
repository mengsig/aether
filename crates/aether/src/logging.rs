//! Log setup. Interactive processes never log to the terminal they draw on.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

mod bounded;
pub(crate) use bounded::check_log_file;

const LOG_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;
const LOG_SEGMENTS: usize = 7;
const SEGMENTS_DIR: &str = "segments-v1";

/// Log to a rolling file under the data directory; also to stderr when
/// `stderr` is set. Returns the guard that flushes the file on drop.
pub fn init(
    file_name: &str,
    stderr: bool,
) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = aether_protocol::paths::log_dir();
    let appender = managed_appender(&dir, file_name, &aether_model::config::config_dir(None))?;
    let (file_writer, guard) = tracing_appender::non_blocking(appender);
    let filter =
        || EnvFilter::try_from_env("AETHER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_filter(filter());
    let registry = tracing_subscriber::registry().with(file_layer);
    if stderr {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_filter(filter()),
            )
            .init();
    } else {
        registry.init();
    }
    Ok(guard)
}

pub fn log_path_hint(file_name: &str) -> String {
    log_file(file_name).display().to_string()
}

/// The file a stream is written to now; older segments sit beside it.
pub fn log_file(file_name: &str) -> PathBuf {
    aether_protocol::paths::log_dir()
        .join(file_name)
        .join(SEGMENTS_DIR)
        .join("events.0.log")
}

fn managed_appender(
    log_dir: &Path,
    file_name: &str,
    config_dir: &Path,
) -> anyhow::Result<bounded::BoundedLog> {
    // Logging must remain available to bridges and clients after a bad config edit.
    // Daemon startup still validates its configuration through the normal loader.
    let policy = (|| -> anyhow::Result<_> {
        let config = aether_model::config::load_config(config_dir)?;
        let disk_floor = match config.storage.free_space_floor_bytes {
            None => config.scheduler.disk_floor().map_err(anyhow::Error::msg)?,
            Some(0) => None,
            Some(bytes) => Some(aether_core::disk::DiskFloor::Bytes(bytes)),
        };
        Ok((config.storage, disk_floor))
    })();
    let (storage, disk_floor) = policy.unwrap_or_else(|error| {
        eprintln!("aether: cannot load log policy from {}: {error}; using default log retention and disk floor", config_dir.display());
        (aether_model::config::StorageSection::default(), Some(aether_core::disk::DiskFloor::Default))
    });
    // A versioned namespace keeps size rotation separate from daily and
    // pre-migration files, which may still have active writers.
    let directory = log_dir.join(file_name).join(SEGMENTS_DIR);
    bounded::BoundedLog::with_policy(
        directory.clone(),
        LOG_SEGMENT_BYTES,
        LOG_SEGMENTS,
        storage.logs_days.unwrap_or(storage.retention_days),
        disk_floor,
    )
    .with_context(|| {
        format!(
            "initialize log stream at {}",
            directory.join("events.0.log").display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_retention_uses_its_own_namespace_and_preserves_legacy_logs() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[storage]\nfree_space_floor_bytes = 0\n",
        )
        .unwrap();
        let directory = root.path().join("aetherd.log");
        std::fs::create_dir(&directory).unwrap();
        for day in 1..=12 {
            std::fs::write(
                directory.join(format!("events.2020-01-{day:02}.log")),
                "old",
            )
            .unwrap();
        }
        let legacy = root.path().join("aetherd.log.2020-01-01");
        std::fs::write(&legacy, "legacy").unwrap();
        let other = root.path().join("aether.log");
        std::fs::create_dir(&other).unwrap();
        let other_log = other.join("events.2020-01-01.log");
        std::fs::write(&other_log, "other stream").unwrap();
        let mut appender = managed_appender(root.path(), "aetherd.log", root.path()).unwrap();
        appender.write_all(b"current").unwrap();
        drop(appender);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 13);
        assert_eq!(
            std::fs::read_to_string(directory.join("segments-v1/events.0.log")).unwrap(),
            "current"
        );
        assert_eq!(std::fs::read_to_string(legacy).unwrap(), "legacy");
        assert_eq!(std::fs::read_to_string(other_log).unwrap(), "other stream");
    }

    #[test]
    fn configured_log_days_override_or_inherit_storage_retention() {
        for (policy, expires) in [
            ("retention_days = 1", true),
            ("retention_days = 0", false),
            ("retention_days = 1\nlogs_days = 0", false),
            ("retention_days = 0\nlogs_days = 1", true),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(
                root.path().join("config.toml"),
                format!("[storage]\nfree_space_floor_bytes = 0\n{policy}\n"),
            )
            .unwrap();
            let directory = root.path().join("aetherd.log").join(SEGMENTS_DIR);
            std::fs::create_dir_all(&directory).unwrap();
            let active = directory.join("events.0.log");
            std::fs::write(&active, b"expired").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&active)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH),
                )
                .unwrap();
            managed_appender(root.path(), "aetherd.log", root.path()).unwrap();
            let expected = if expires { &b""[..] } else { &b"expired"[..] };
            assert_eq!(std::fs::read(&active).unwrap(), expected, "{policy}");
        }
    }

    #[test]
    fn invalid_log_configuration_falls_back_without_breaking_auxiliary_commands() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[storage]\nlogs_days = -1\n",
        )
        .unwrap();
        assert!(aether_model::config::load_config(root.path()).is_err());
        let _appender = managed_appender(root.path(), "mcp-bridge.log", root.path()).unwrap();
        assert!(
            root.path()
                .join("mcp-bridge.log/segments-v1/events.0.log")
                .is_file()
        );
    }

    #[test]
    fn storage_disk_floor_overrides_or_inherits_the_scheduler_floor() {
        use std::io::Write;
        for (scheduler_mib, storage, blocked) in [
            (u64::MAX >> 20, "", true),
            (
                u64::MAX >> 20,
                "[storage]\nfree_space_floor_bytes = 0",
                false,
            ),
            (0, "", false),
            (
                0,
                "[storage]\nfree_space_floor_bytes = 9223372036854775807",
                true,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(
                root.path().join("config.toml"),
                format!("[scheduler]\nmin_free_disk_mib = {scheduler_mib}\n{storage}\n"),
            )
            .unwrap();
            let mut log = managed_appender(root.path(), "aetherd.log", root.path()).unwrap();
            assert_eq!(log.write_all(b"record\n").is_err(), blocked);
        }
    }
}
