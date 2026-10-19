//! Declarative transcript presentation, independent of ingestion and rendering.
mod files;
pub use files::discover;

use crate::app::transcript::{
    CallStatus, CellModel, ChildOutcome, CompactionState, NoticeKind, UserFrom,
};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    pub name: String,
    pub source: String,
    pub definition: Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
            description: Some("Coordination without tool detail".into()),
            density: Density::Compact,
            show: vec![
                UserText,
                AssistantText,
                Delegation,
                AgentMessage,
                Completion,
                Status,
                Question,
                Approval,
                Error,
            ],
            scope: None,
        };
        let standard = Definition {
            version: 1,
            label: Some("Standard".into()),
            description: Some("Complete stock transcript".into()),
            density: Density::Comfortable,
            show: vec![],
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

pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 48
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

pub fn category(model: &CellModel) -> Category {
    use Category::*;
    match model {
        CellModel::Header { .. } => Status,
        CellModel::User { from, .. } => match from {
            UserFrom::Operator => UserText,
            UserFrom::Task { .. } => Delegation,
            UserFrom::Message { .. } => AgentMessage,
        },
        CellModel::Assistant { .. } => AssistantText,
        CellModel::Reasoning { .. } => Reasoning,
        CellModel::Exec { .. }
        | CellModel::Explored { .. }
        | CellModel::Edited { .. }
        | CellModel::Called { .. } => Tool,
        CellModel::Memory(_) => Memory,
        CellModel::Spawned { .. } => Delegation,
        CellModel::Waiting { .. } => Status,
        CellModel::ChildResult { .. } => Completion,
        CellModel::Sent { .. } => AgentMessage,
        CellModel::ApprovalRecord { .. } => Approval,
        CellModel::Question { .. } => Question,
        CellModel::Error { .. } => Error,
        CellModel::Notice { kind, .. } => match kind {
            NoticeKind::ClientError
            | NoticeKind::Backoff { .. }
            | NoticeKind::Fallback
            | NoticeKind::McpFailed
            | NoticeKind::BudgetWarning
            | NoticeKind::Engine
            | NoticeKind::Store => Error,
            NoticeKind::Recovered { .. }
            | NoticeKind::Reconnected
            | NoticeKind::Interrupted
            | NoticeKind::ModelChange
            | NoticeKind::Handoff
            | NoticeKind::Cancelled
            | NoticeKind::Echo
            | NoticeKind::Trigger { .. }
            | NoticeKind::Note(_) => Status,
        },
        CellModel::Compaction(_) => Compaction,
        CellModel::Separator { .. } => Usage,
    }
}

pub fn mandatory(model: &CellModel) -> bool {
    matches!(
        category(model),
        Category::Question | Category::Approval | Category::Error
    ) || match model {
        CellModel::Header { .. } => true,
        CellModel::Exec { status, .. }
        | CellModel::Called { status, .. }
        | CellModel::Edited { status, .. } => matches!(
            status,
            CallStatus::Awaiting | CallStatus::Failed | CallStatus::Denied | CallStatus::Cancelled
        ),
        CellModel::ChildResult { outcome, .. } => !matches!(outcome, ChildOutcome::Finished),
        CellModel::Compaction(CompactionState::Failed { .. }) => true,
        _ => false,
    }
}

/// A failed call that carries its own error text.
fn failed_call(model: &CellModel) -> bool {
    matches!(
        model,
        CellModel::Exec {
            status: CallStatus::Failed,
            ..
        } | CellModel::Called {
            status: CallStatus::Failed,
            ..
        }
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Policy<'a> {
    pub definition: Option<&'a Definition>,
}
impl Policy<'_> {
    pub fn shows(&self, model: &CellModel) -> bool {
        let Some(definition) = self.definition else {
            return true;
        };
        if definition.show.contains(&category(model)) {
            return true;
        }
        // A failed edit's line is its named-only call cell; the edit itself is tool detail.
        let failed_edit = matches!(
            model,
            CellModel::Edited {
                status: CallStatus::Failed,
                ..
            }
        );
        mandatory(model) && !failed_edit
    }
    pub fn shows_category(&self, kind: Category) -> bool {
        self.definition.is_none_or(|d| d.show.contains(&kind))
    }
    /// A failed call in a mode without tools: one line, never its command or output.
    pub fn failed_tool_line(&self, model: &CellModel) -> bool {
        failed_call(model)
            && self
                .definition
                .is_some_and(|d| !d.show.contains(&Category::Tool))
    }
    pub fn compact(&self, model: &CellModel) -> bool {
        self.failed_tool_line(model)
            || self
                .definition
                .is_some_and(|d| d.density == Density::Compact)
                && !mandatory(model)
                && matches!(
                    category(model),
                    Category::Delegation
                        | Category::AgentMessage
                        | Category::Completion
                        | Category::Status
                )
    }
    /// A decision or failure a named mode draws whole, as details would.
    pub fn expands(&self, model: &CellModel) -> bool {
        self.definition.is_some() && mandatory(model) && !self.failed_tool_line(model)
    }
}
