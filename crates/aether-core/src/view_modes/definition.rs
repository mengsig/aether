use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ViewModeCategory")]
pub enum Category {
    UserText,
    AssistantText,
    Delegation,
    AgentMessage,
    Completion,
    Status,
    Question,
    Approval,
    Error,
    Tool,
    Reasoning,
    Memory,
    Compaction,
    Usage,
}

impl Category {
    pub const ALL: [Self; 14] = [
        Self::UserText,
        Self::AssistantText,
        Self::Delegation,
        Self::AgentMessage,
        Self::Completion,
        Self::Status,
        Self::Question,
        Self::Approval,
        Self::Error,
        Self::Tool,
        Self::Reasoning,
        Self::Memory,
        Self::Compaction,
        Self::Usage,
    ];
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ViewModeDensity")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ViewModeScope")]
pub struct Scope {
    pub name: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub version: u32,
    pub label: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub density: Density,
    pub show: Vec<Category>,
    pub scope: Option<Scope>,
}

impl Definition {
    pub fn parse(text: &str) -> Result<Self, String> {
        let definition: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if definition.version != 1 {
            return Err("version must be 1".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        if definition.show.iter().any(|kind| !seen.insert(*kind)) {
            return Err("duplicate category in show".into());
        }
        for (key, value, limit) in [
            ("label", definition.label.as_deref(), 64),
            ("description", definition.description.as_deref(), 160),
            (
                "scope.name",
                definition.scope.as_ref().and_then(|s| s.name.as_deref()),
                128,
            ),
            (
                "scope.role",
                definition.scope.as_ref().and_then(|s| s.role.as_deref()),
                128,
            ),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty()
                    || value.chars().count() > limit
                    || value
                        .chars()
                        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}')))
            {
                return Err(format!(
                    "{key} must be nonblank, single-line, without controls, and at most {limit} characters"
                ));
            }
        }
        if definition
            .scope
            .as_ref()
            .is_some_and(|s| s.name.is_none() && s.role.is_none())
        {
            return Err("scope must specify name or role".into());
        }
        Ok(definition)
    }

    pub fn matches(&self, name: Option<&str>, role: Option<&str>) -> bool {
        self.scope.as_ref().is_none_or(|scope| {
            scope
                .name
                .as_deref()
                .is_none_or(|wanted| Some(wanted) == name)
                && scope
                    .role
                    .as_deref()
                    .is_none_or(|wanted| Some(wanted) == role)
        })
    }
}

pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 48
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}
