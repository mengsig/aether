use serde::{Deserialize, Serialize};

/// What `aether` does about newer releases when it starts.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateSection {
    /// Look for a newer release on launch and offer to install it.
    pub check: bool,
}

impl Default for UpdateSection {
    fn default() -> Self {
        Self { check: true }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AetherConfig;

    #[test]
    fn launch_check_is_on_unless_switched_off() {
        let config: AetherConfig = toml::from_str("").unwrap();
        assert!(config.update.check);
        let config: AetherConfig = toml::from_str("[update]\ncheck = false").unwrap();
        assert!(!config.update.check);
    }

    #[test]
    fn generated_template_documents_a_working_update_override() {
        let template = super::super::CONFIG_TEMPLATE;
        assert!(template.contains("# [update]"));
        let uncommented =
            template.replace("# [update]\n# check = false", "[update]\ncheck = false");
        let config: AetherConfig = toml::from_str(&uncommented).unwrap();
        assert!(!config.update.check);
    }
}
