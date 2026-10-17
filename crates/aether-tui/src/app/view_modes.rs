use super::{App, Effect};
use crate::commands::{InputAction, SlashCommand};
use crate::view_modes::{Catalog, Policy};

impl App {
    pub fn policy(&self, agent: Option<super::AgentId>) -> Policy<'_> {
        let owner = agent
            .and_then(|id| self.tree.nodes.get(&id))
            .map(|n| &n.agent);
        let definition = self
            .view_modes
            .modes
            .get(&self.view_mode)
            .filter(|mode| {
                mode.name != "standard"
                    && !self.details
                    && mode.definition.matches(
                        owner.map(|a| a.name.as_str()),
                        owner.map(|a| a.role.0.as_str()),
                    )
            })
            .map(|mode| &mode.definition);
        Policy { definition }
    }

    pub fn parse_input(&self, text: &str) -> InputAction {
        let parsed = crate::commands::parse(text);
        let InputAction::Command(SlashCommand::Unknown(name)) = &parsed else {
            return parsed;
        };
        let live = self.selected.is_some_and(|id| !self.is_terminal(id));
        // A listed host command always goes to the host, even past a view mode
        // reloaded under its name.
        let host = &self.options.host_commands;
        if host.find(name).is_some() && !live {
            return InputAction::Command(SlashCommand::Invalid(format!(
                "/{name} is for {}; view a live agent to send it",
                host.host
            )));
        }
        if host.find(name).is_none() && self.view_modes.modes.contains_key(name) {
            if text.split_whitespace().count() != 1 {
                return InputAction::Command(SlashCommand::Invalid(format!("usage: /{name}")));
            }
            return InputAction::Command(SlashCommand::ViewMode(Some(
                if self.view_mode == *name {
                    "standard".into()
                } else {
                    name.clone()
                },
            )));
        }
        // Another program's vocabulary (Archon's `/answer 3 yes`) reaches a live
        // agent verbatim; with none to receive it, it stays an unknown command.
        if live {
            return InputAction::Message(text.trim().to_owned());
        }
        parsed
    }

    pub(crate) fn set_view_mode(&mut self, name: &str) {
        if !self.view_modes.modes.contains_key(name) {
            self.flash_error(format!(
                "unavailable view mode {name:?}; /mode lists definitions"
            ));
            return;
        }
        self.view_mode = name.into();
        self.presentation_changed();
        match self.save_prefs(|prefs| prefs.view_mode = Some(name.into())) {
            Ok(()) => self.flash(format!("view mode {name}")),
            Err(reason) => self.flash_error(format!(
                "view mode {name} {} not saved: {reason}",
                self.theme.glyphs().dot
            )),
        }
    }

    pub(crate) fn presentation_changed(&mut self) {
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
        self.dirty = true;
    }

    pub(crate) fn reload_modes(&mut self) -> Vec<Effect> {
        if self.modes_reloading {
            self.flash("view mode reload already in progress");
            return Vec::new();
        }
        match self.options.definitions_dir.clone() {
            Some(dir) => {
                self.modes_reloading = true;
                vec![Effect::ReloadViewModes(dir)]
            }
            None => {
                self.flash_error("no config directory for /mode reload");
                Vec::new()
            }
        }
    }

    pub(crate) fn modes_loaded(&mut self, catalog: Catalog) {
        self.modes_reloading = false;
        self.view_modes = catalog;
        if !self.view_modes.modes.contains_key(&self.view_mode) {
            let removed = std::mem::replace(&mut self.view_mode, "standard".into());
            self.view_modes.diagnostics.insert(
                0,
                crate::view_modes::Diagnostic {
                    source: "reload".into(),
                    reason: format!(
                        "active mode {removed} not found; using standard (saved choice retained)"
                    ),
                },
            );
        }
        self.presentation_changed();
        self.report_mode_diagnostics();
        if self.view_modes.diagnostics.is_empty() {
            self.flash("view modes reloaded");
        }
    }

    pub(crate) fn report_mode_diagnostics(&mut self) {
        if !self.view_modes.diagnostics.is_empty() {
            let mut diagnostics: Vec<_> = self.view_modes.diagnostics.iter().collect();
            diagnostics.sort_by_key(|notice| {
                !matches!(notice.source.as_str(), "reload" | "ui.toml view_mode")
            });
            let summaries = diagnostics
                .iter()
                .map(|notice| {
                    let source = std::path::Path::new(&notice.source)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let reason = notice
                        .reason
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("");
                    let summary =
                        if matches!(notice.source.as_str(), "reload" | "ui.toml view_mode") {
                            reason.to_owned()
                        } else {
                            format!("{source}: {reason}")
                        };
                    crate::format::one_line_with_marker(
                        &aether_core::text::strip_controls(&summary),
                        100,
                        self.theme.glyphs().ellipsis.chars().next().unwrap_or('~'),
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            self.flash_error(format!(
                "{summaries}; {} view mode notice(s); /mode shows details",
                diagnostics.len(),
            ));
        }
    }

    pub(crate) fn help_modes(&self) -> Option<&Catalog> {
        (self.view_mode != "standard" || self.view_modes.modes.len() > 2)
            .then_some(&self.view_modes)
    }

    pub fn mode_label(&self) -> Option<String> {
        (self.view_mode != "standard").then(|| {
            let state = if self.details {
                " details"
            } else if self.policy(self.selected).definition.is_none() {
                " -> standard"
            } else {
                ""
            };
            format!("{}{state}", self.view_mode)
        })
    }
}
