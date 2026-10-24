use std::path::{Component, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Seconds an ended agent's processes get to honour SIGTERM before SIGKILL.
pub const DEFAULT_REAP_GRACE_SECS: u64 = 10;
/// The longest grace accepted: an agent's end must not hold its processes for long.
const MAX_REAP_GRACE_SECS: u64 = 300;

/// `[agents]`: where agents' own processes keep their scratch, and how they end.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsSection {
    /// Private root of the per-agent temp dirs. Unset means `t` beside the
    /// store. Taken as written: no `~` or environment expansion.
    pub temp_root: Option<PathBuf>,
    /// Seconds an ended agent's leftover processes get between SIGTERM and
    /// SIGKILL. Unset means [`DEFAULT_REAP_GRACE_SECS`].
    pub reap_grace_secs: Option<u64>,
}

impl AgentsSection {
    pub fn reap_grace(&self) -> Duration {
        Duration::from_secs(self.reap_grace_secs.unwrap_or(DEFAULT_REAP_GRACE_SECS))
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(secs) = self.reap_grace_secs
            && secs > MAX_REAP_GRACE_SECS
        {
            return Err(format!(
                "agents.reap_grace_secs {secs} is above the maximum of {MAX_REAP_GRACE_SECS}"
            ));
        }
        self.validate_temp_root()
    }

    /// The root must name one place whatever the daemon's working directory,
    /// and become an environment value, so it is absolute, plain and UTF-8.
    fn validate_temp_root(&self) -> Result<(), String> {
        let Some(root) = &self.temp_root else {
            return Ok(());
        };
        let shown = root.display();
        if root.to_str().is_none() {
            return Err(format!("agents.temp_root `{shown}` is not valid UTF-8"));
        }
        if !root.is_absolute() {
            return Err(format!(
                "agents.temp_root `{shown}` must be an absolute path; `~` and variables are not \
                 expanded"
            ));
        }
        if root.components().any(|c| c == Component::ParentDir) {
            return Err(format!("agents.temp_root `{shown}` must not contain `..`"));
        }
        if root.parent().is_none() {
            return Err("agents.temp_root cannot be the filesystem root".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn temp_root_defaults_to_unset_and_takes_an_absolute_path() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(config.agents.temp_root, None);
        let config: AetherConfig = toml::from_str("[agents]\ntemp_root = \"/srv/at\"\n").unwrap();
        assert_eq!(config.agents.temp_root.as_deref(), Some("/srv/at".as_ref()));
        assert_eq!(config.agents.validate(), Ok(()));
    }

    #[test]
    fn reap_grace_defaults_to_ten_seconds_and_is_bounded() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(
            config.agents.reap_grace(),
            std::time::Duration::from_secs(10)
        );
        let config: AetherConfig = toml::from_str("[agents]\nreap_grace_secs = 0\n").unwrap();
        assert_eq!(config.agents.reap_grace(), std::time::Duration::ZERO);
        assert_eq!(config.agents.validate(), Ok(()));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[agents]\nreap_grace_secs = 301\n",
        )
        .unwrap();
        assert!(load_config(dir.path()).is_err());
    }

    #[test]
    fn invalid_temp_roots_are_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        for setting in [
            "temp_root = \"relative/t\"",
            "temp_root = \"~/t\"",
            "temp_root = \"$HOME/t\"",
            "temp_root = \"/srv/../t\"",
            "temp_root = \"/\"",
            "temp_root = 3",
            "tmp_root = \"/srv/t\"",
        ] {
            std::fs::write(
                dir.path().join("config.toml"),
                format!("[agents]\n{setting}\n"),
            )
            .unwrap();
            assert!(load_config(dir.path()).is_err(), "{setting}");
        }
    }
}
