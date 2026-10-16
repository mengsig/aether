use serde::{Deserialize, Serialize};

/// Passive usage freshness policy, in seconds, for every configured provider.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersSection {
    pub usage_stale_after: u64,
}

impl Default for ProvidersSection {
    fn default() -> Self {
        Self {
            usage_stale_after: 15 * 60,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AetherConfig;

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
}
