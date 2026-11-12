//! The daemon's presentation catalog, without host filesystem paths.
use aether_core::view_modes::{Category, Density, Diagnostic, Mode, Scope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewMode {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub density: Density,
    pub show: Vec<Category>,
    pub scope: Option<Scope>,
    pub builtin: bool,
}

impl From<&Mode> for ViewMode {
    fn from(mode: &Mode) -> Self {
        let definition = &mode.definition;
        Self {
            name: mode.name.clone(),
            label: definition.label.clone(),
            description: definition.description.clone(),
            density: definition.density,
            show: definition.show.clone(),
            scope: definition.scope.clone(),
            builtin: mode.source == "built-in",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ViewModes {
    pub modes: Vec<ViewMode>,
    pub diagnostics: Vec<Diagnostic>,
}
