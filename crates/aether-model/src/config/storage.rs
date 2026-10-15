use std::num::{NonZeroU16, NonZeroU32};

use serde::{Deserialize, Serialize};

/// Filesystem retention. Zero days disables cleanup of that kind.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageSection {
    pub retention_days: u16,
    pub workspaces_days: Option<u16>,
    pub engine_sessions_days: Option<u16>,
    pub backups_days: Option<u16>,
    pub logs_days: Option<u16>,
    /// Explicit override of the existing artifact policy; zero keeps all payloads.
    pub artifacts_days: Option<u16>,
    /// Explicit override of completed-run retention; zero disables history expiry.
    pub event_history_days: Option<u16>,
    pub workspace_grace_seconds: u32,
    pub backups_keep: NonZeroU16,
    pub gc_interval_seconds: NonZeroU32,
    /// None uses the scheduler's floor; zero disables the low-disk guard.
    pub free_space_floor_bytes: Option<u64>,
}

impl Default for StorageSection {
    fn default() -> Self {
        Self {
            retention_days: 14,
            workspaces_days: None,
            engine_sessions_days: None,
            backups_days: None,
            logs_days: None,
            artifacts_days: None,
            event_history_days: None,
            workspace_grace_seconds: 3600,
            backups_keep: const { NonZeroU16::new(3).unwrap() },
            gc_interval_seconds: const { NonZeroU32::new(3600).unwrap() },
            free_space_floor_bytes: None,
        }
    }
}

impl StorageSection {
    pub fn days(&self, override_days: Option<u16>) -> u16 {
        override_days.unwrap_or(self.retention_days)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn defaults_and_explicit_disable() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(config.storage.retention_days, 14);
        assert_eq!(config.storage.backups_keep.get(), 3);
        let config: AetherConfig = toml::from_str(
            "[storage]\nworkspaces_days = 0\nbackups_days = 21\nworkspace_grace_seconds = 0",
        )
        .unwrap();
        assert_eq!(config.storage.days(config.storage.workspaces_days), 0);
        assert_eq!(config.storage.days(config.storage.backups_days), 21);
        assert_eq!(config.storage.days(config.storage.logs_days), 14);
    }

    #[test]
    fn invalid_storage_is_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        for setting in [
            "retention_days = -1",
            "logs_days = 65536",
            "retention_days = 1.5",
            "backups_keep = 0",
            "gc_interval_seconds = 0",
            "workspace_grace_seconds = -1",
            "free_space_floor_bytes = -1",
            "retentoin_days = 14",
        ] {
            std::fs::write(
                dir.path().join("config.toml"),
                format!("[storage]\n{setting}\n"),
            )
            .unwrap();
            assert!(load_config(dir.path()).is_err(), "{setting}");
        }
    }
}
