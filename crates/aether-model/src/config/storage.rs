use std::collections::BTreeMap;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Component, Path, PathBuf};

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
    /// Hard cap: migration backups beyond the newest this many are removed
    /// whatever their age.
    pub migration_backups_keep: NonZeroU16,
    pub gc_interval_seconds: NonZeroU32,
    /// None uses the scheduler's floor; zero disables the low-disk guard.
    pub free_space_floor_bytes: Option<u64>,
    /// Directory names whose wholly Git-ignored copies a finished workspace
    /// may lose to GC: regenerable build output, never source.
    pub build_output_dirs: Vec<String>,
    /// Replaces `build_output_dirs` for workspaces of one source checkout.
    pub repository_build_output_dirs: BTreeMap<PathBuf, Vec<String>>,
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
            migration_backups_keep: const { NonZeroU16::new(2).unwrap() },
            gc_interval_seconds: const { NonZeroU32::new(3600).unwrap() },
            free_space_floor_bytes: None,
            build_output_dirs: ["target", "node_modules", ".venv", "__pycache__", "dist"]
                .map(String::from)
                .to_vec(),
            repository_build_output_dirs: BTreeMap::new(),
        }
    }
}

impl StorageSection {
    pub fn days(&self, override_days: Option<u16>) -> u16 {
        override_days.unwrap_or(self.retention_days)
    }

    /// The build output names for workspaces of `source`.
    pub fn build_output_dirs_for(&self, source: &Path) -> &[String] {
        self.repository_build_output_dirs
            .get(source)
            .unwrap_or(&self.build_output_dirs)
    }

    /// Names are matched against single path components; a separator, `..`
    /// or `.git` could make GC delete something that is not build output.
    pub fn validate(&self) -> Result<(), String> {
        check_build_output_names("storage.build_output_dirs", &self.build_output_dirs)?;
        for (repository, names) in &self.repository_build_output_dirs {
            let shown = repository.display();
            if !repository.is_absolute()
                || repository
                    .components()
                    .any(|part| part == Component::ParentDir)
            {
                return Err(format!(
                    "storage.repository_build_output_dirs key `{shown}` must be an absolute \
                     path without `..`"
                ));
            }
            check_build_output_names(
                &format!("storage.repository_build_output_dirs.\"{shown}\""),
                names,
            )?;
        }
        Ok(())
    }
}

fn check_build_output_names(setting: &str, names: &[String]) -> Result<(), String> {
    for name in names {
        let plain = Path::new(name).components().collect::<Vec<_>>();
        if !matches!(plain.as_slice(), [Component::Normal(part)] if *part == name.as_str())
            || name.contains('\0')
            || name.eq_ignore_ascii_case(".git")
        {
            return Err(format!(
                "{setting}: `{name}` must be one plain directory name, not `.git`"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn defaults_and_explicit_disable() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(config.storage.retention_days, 14);
        assert_eq!(config.storage.backups_keep.get(), 3);
        assert_eq!(config.storage.migration_backups_keep.get(), 2);
        assert_eq!(config.storage.days(config.storage.engine_sessions_days), 14);
        let config: AetherConfig = toml::from_str(
            "[storage]\nworkspaces_days = 0\nengine_sessions_days = 0\nbackups_days = 21\nworkspace_grace_seconds = 0",
        )
        .unwrap();
        assert_eq!(config.storage.days(config.storage.workspaces_days), 0);
        assert_eq!(config.storage.days(config.storage.engine_sessions_days), 0);
        assert_eq!(config.storage.days(config.storage.backups_days), 21);
        assert_eq!(config.storage.days(config.storage.logs_days), 14);
    }

    #[test]
    fn build_output_defaults_and_repository_override() {
        let config: AetherConfig = toml::from_str("").unwrap();
        let defaults = ["target", "node_modules", ".venv", "__pycache__", "dist"];
        assert_eq!(config.storage.build_output_dirs, defaults);
        assert_eq!(config.storage.validate(), Ok(()));
        let config: AetherConfig = toml::from_str(
            "[storage]\nbuild_output_dirs = []\n\
             [storage.repository_build_output_dirs]\n\"/srv/repo\" = [\"build\"]\n",
        )
        .unwrap();
        assert_eq!(config.storage.validate(), Ok(()));
        let names = config.storage.build_output_dirs_for("/srv/repo".as_ref());
        assert_eq!(names, ["build"]);
        assert!(
            config
                .storage
                .build_output_dirs_for("/srv/other".as_ref())
                .is_empty()
        );
    }

    #[test]
    fn invalid_storage_is_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        for setting in [
            "retention_days = -1",
            "logs_days = 65536",
            "retention_days = 1.5",
            "backups_keep = 0",
            "migration_backups_keep = 0",
            "gc_interval_seconds = 0",
            "workspace_grace_seconds = -1",
            "free_space_floor_bytes = -1",
            "retentoin_days = 14",
            "build_output_dirs = [\"target/debug\"]",
            "build_output_dirs = [\"..\"]",
            "build_output_dirs = [\".\"]",
            "build_output_dirs = [\"\"]",
            "build_output_dirs = [\".git\"]",
            "build_output_dirs = [\"/target\"]",
            "build_output_dirs = \"target\"",
            "repository_build_output_dirs = { \"relative/repo\" = [\"target\"] }",
            "repository_build_output_dirs = { \"/srv/../repo\" = [\"target\"] }",
            "repository_build_output_dirs = { \"/srv/repo\" = [\"a/b\"] }",
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
