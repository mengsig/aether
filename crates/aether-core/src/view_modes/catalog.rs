use super::{Category, Definition, Density};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub name: String,
    pub source: String,
    pub definition: Definition,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[schemars(rename = "ViewModeDiagnostic")]
pub struct Diagnostic {
    pub source: String,
    pub reason: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.source, self.reason)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub modes: BTreeMap<String, Mode>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Default for Catalog {
    fn default() -> Self {
        use Category::*;
        let focus = Definition {
            version: 1,
            label: Some("Focus".into()),
            description: Some("The important entries: what the operator and the agent said".into()),
            density: Density::Compact,
            show: vec![UserText, AssistantText, Question],
            scope: None,
        };
        let standard = Definition {
            version: 1,
            label: Some("Standard".into()),
            description: Some("Complete stock transcript".into()),
            density: Density::Comfortable,
            show: Category::ALL.to_vec(),
            scope: None,
        };
        Self {
            modes: [("standard", standard), ("focus", focus)]
                .into_iter()
                .map(|(name, definition)| {
                    (
                        name.into(),
                        Mode {
                            name: name.into(),
                            source: "built-in".into(),
                            definition,
                        },
                    )
                })
                .collect(),
            diagnostics: Vec::new(),
        }
    }
}

impl Catalog {
    pub fn ordered(&self) -> impl Iterator<Item = &Mode> {
        ["standard", "focus"]
            .into_iter()
            .filter_map(|name| self.modes.get(name))
            .chain(self.modes.values().filter(|mode| mode.source != "built-in"))
    }

    pub fn resolve(
        &self,
        flag: Option<&str>,
        environment: Option<&std::ffi::OsStr>,
        saved: Option<&str>,
    ) -> Result<(String, Option<Diagnostic>), String> {
        let environment = if flag.is_some() {
            None
        } else {
            environment
                .map(|s| s.to_str().ok_or("AETHER_VIEW_MODE must be UTF-8"))
                .transpose()?
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let explicit = flag
            .map(|s| (s, "--view-mode"))
            .or(environment.map(|s| (s, "AETHER_VIEW_MODE")));
        let (name, source) = explicit
            .or(saved.map(|s| (s, "ui.toml view_mode")))
            .unwrap_or(("standard", "default"));
        if self.modes.contains_key(name) {
            return Ok((name.into(), None));
        }
        let reason = format!("unavailable view mode {name:?}; use /mode to inspect definitions");
        if explicit.is_some() {
            Err(format!("{source}: {reason}"))
        } else {
            Ok((
                "standard".into(),
                Some(Diagnostic {
                    source: source.into(),
                    reason: format!(
                        "active mode {name} not found; using standard (saved choice retained)"
                    ),
                }),
            ))
        }
    }
}
