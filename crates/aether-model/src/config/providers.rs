use serde::{Deserialize, Serialize};

/// Usage freshness and opt-in probe limits, in seconds, for configured providers.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersSection {
    pub usage_stale_after: u64,
    pub usage_refresh_interval: u64,
    pub usage_refresh_timeout: u64,
}

impl Default for ProvidersSection {
    fn default() -> Self {
        Self {
            usage_stale_after: 15 * 60,
            usage_refresh_interval: 10 * 60,
            usage_refresh_timeout: 15,
        }
    }
}

impl ProvidersSection {
    pub fn validate(&self) -> Result<(), String> {
        if self.usage_refresh_interval == 0 {
            return Err("providers.usage_refresh_interval must be greater than 0 seconds".into());
        }
        if self.usage_refresh_timeout == 0 {
            return Err("providers.usage_refresh_timeout must be greater than 0 seconds".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{AetherConfig, load_config};

    #[test]
    fn usage_freshness_defaults_and_accepts_seconds() {
        let defaults: AetherConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.providers.usage_stale_after, 900);
        let custom: AetherConfig = toml::from_str("[providers]\nusage_stale_after = 60").unwrap();
        assert_eq!(custom.providers.usage_stale_after, 60);
        for invalid in ["-1", "1.5", "\"15m\""] {
            assert!(
                toml::from_str::<AetherConfig>(&format!(
                    "[providers]\nusage_stale_after = {invalid}"
                ))
                .is_err()
            );
        }
        assert!(toml::from_str::<AetherConfig>("[providers]\nusage_stale_aftr = 60").is_err());
    }

    #[test]
    fn refresh_defaults_preserve_legacy_config_and_accept_seconds() {
        let legacy: AetherConfig = toml::from_str("[providers]\nusage_stale_after = 60").unwrap();
        assert_eq!(legacy.providers.usage_refresh_interval, 600);
        assert_eq!(legacy.providers.usage_refresh_timeout, 15);
        assert_eq!(legacy.providers.validate(), Ok(()));
        let custom: AetherConfig =
            toml::from_str("[providers]\nusage_refresh_interval = 30\nusage_refresh_timeout = 5")
                .unwrap();
        assert_eq!(custom.providers.usage_refresh_interval, 30);
        assert_eq!(custom.providers.usage_refresh_timeout, 5);
        assert_eq!(custom.providers.validate(), Ok(()));
        let encoded = toml::to_string(&custom.providers).unwrap();
        assert_eq!(
            toml::from_str::<super::ProvidersSection>(&encoded).unwrap(),
            custom.providers
        );
    }

    #[test]
    fn refresh_limits_are_positive_seconds_at_the_load_boundary() {
        let dir = tempfile::tempdir().unwrap();
        for field in ["usage_refresh_interval", "usage_refresh_timeout"] {
            for invalid in ["0", "-1", "1.5", "\"unbounded\"", "\"15s\""] {
                std::fs::write(
                    dir.path().join("config.toml"),
                    format!("[providers]\n{field} = {invalid}"),
                )
                .unwrap();
                let error = load_config(dir.path()).unwrap_err();
                assert!(error.reason.contains(field), "{field}={invalid}: {error}");
                assert!(error.path.ends_with("config.toml"));
            }
        }
        std::fs::write(
            dir.path().join("config.toml"),
            "[providers]\nusage_stale_after = 0\nusage_refresh_interval = 1\nusage_refresh_timeout = 1",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.providers.usage_stale_after, 0);
        assert_eq!(config.providers.usage_refresh_timeout, 1);
    }
}
