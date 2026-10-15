use std::path::{Component, PathBuf};

use serde::{Deserialize, Serialize};

/// `[agents]`: where agents' own processes keep their scratch.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsSection {
    /// Private root of the per-agent temp dirs. Unset means `t` beside the
    /// store. Taken as written: no `~` or environment expansion.
    pub temp_root: Option<PathBuf>,
}

impl AgentsSection {
    /// The root must name one place whatever the daemon's working directory,
    /// and become an environment value, so it is absolute, plain and UTF-8.
    pub fn validate(&self) -> Result<(), String> {
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
