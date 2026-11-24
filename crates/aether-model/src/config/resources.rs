use aether_core::agent_slice::{self, AgentSliceLimits};
use serde::{Deserialize, Serialize};

/// `[resources]`: the limits of the systemd slice agents' engines run in.
/// An unset key takes its default; `"off"` leaves that limit unset.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourcesSection {
    pub agents_cpu_weight: Option<ResourceSetting>,
    pub agents_io_weight: Option<ResourceSetting>,
    pub agents_memory_high: Option<ResourceSetting>,
}

/// A limit as written: a number, or text such as `"8G"` or `"off"`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ResourceSetting {
    Number(u64),
    Text(String),
}

const OFF: &str = "off";

impl ResourcesSection {
    /// The slice limits these settings ask for, or why one is not a limit.
    pub fn agent_slice(&self) -> Result<AgentSliceLimits, String> {
        let defaults = AgentSliceLimits::default();
        Ok(AgentSliceLimits {
            cpu_weight: weight(
                "resources.agents_cpu_weight",
                self.agents_cpu_weight.as_ref(),
                defaults.cpu_weight,
            )?,
            io_weight: weight(
                "resources.agents_io_weight",
                self.agents_io_weight.as_ref(),
                defaults.io_weight,
            )?,
            memory_high: memory_high(self.agents_memory_high.as_ref(), defaults.memory_high)?,
        })
    }
}

fn weight(
    what: &str,
    setting: Option<&ResourceSetting>,
    default: Option<u64>,
) -> Result<Option<u64>, String> {
    match setting {
        None => Ok(default),
        Some(ResourceSetting::Number(value)) => agent_slice::parse_weight(what, *value).map(Some),
        Some(ResourceSetting::Text(text)) if text == OFF => Ok(None),
        Some(ResourceSetting::Text(text)) => Err(format!(
            "{what} `{text}` must be a number from {} to {}, or \"off\"",
            agent_slice::WEIGHTS.start(),
            agent_slice::WEIGHTS.end()
        )),
    }
}

fn memory_high(
    setting: Option<&ResourceSetting>,
    default: Option<String>,
) -> Result<Option<String>, String> {
    const WHAT: &str = "resources.agents_memory_high";
    match setting {
        None => Ok(default),
        Some(ResourceSetting::Text(text)) if text == OFF => Ok(None),
        Some(ResourceSetting::Text(text)) => agent_slice::parse_memory_high(WHAT, text).map(Some),
        Some(ResourceSetting::Number(bytes)) => {
            agent_slice::parse_memory_high(WHAT, &bytes.to_string()).map(Some)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AetherConfig;

    fn parsed(text: &str) -> Result<AgentSliceLimits, String> {
        let config: AetherConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        config.resources.agent_slice()
    }

    #[test]
    fn unset_keys_take_the_defaults() {
        assert_eq!(parsed(""), Ok(AgentSliceLimits::default()));
        assert_eq!(parsed("[resources]\n"), Ok(AgentSliceLimits::default()));
    }

    #[test]
    fn off_turns_each_limit_off_alone() {
        let limits = parsed(
            "[resources]\nagents_cpu_weight = \"off\"\nagents_io_weight = 50\n\
             agents_memory_high = \"off\"\n",
        )
        .unwrap();
        assert_eq!(
            limits,
            AgentSliceLimits {
                cpu_weight: None,
                io_weight: Some(50),
                memory_high: None,
            }
        );
        let all_off = parsed(
            "[resources]\nagents_cpu_weight = \"off\"\nagents_io_weight = \"off\"\n\
             agents_memory_high = \"off\"\n",
        )
        .unwrap();
        assert!(all_off.is_off());
    }

    #[test]
    fn memory_high_takes_sizes_and_bytes() {
        let limits = parsed("[resources]\nagents_memory_high = \"4G\"\n").unwrap();
        assert_eq!(limits.memory_high.as_deref(), Some("4G"));
        let limits = parsed("[resources]\nagents_memory_high = 1073741824\n").unwrap();
        assert_eq!(limits.memory_high.as_deref(), Some("1073741824"));
    }

    #[test]
    fn refuses_what_is_not_a_limit() {
        for bad in [
            "agents_cpu_weight = 0",
            "agents_cpu_weight = 20000",
            "agents_io_weight = \"low\"",
            "agents_cpu_weight = \"20\"",
            "agents_memory_high = \"8X\"",
            "agents_memory_high = 0",
            "agents_memory_high = \"\"",
        ] {
            let error = parsed(&format!("[resources]\n{bad}\n")).unwrap_err();
            assert!(error.contains("resources.agents_"), "{bad}: {error}");
        }
        assert!(parsed("[resources]\nagents_memory_max = \"8G\"\n").is_err());
    }
}
