//! Application preferences.
use super::{AgentId, AgentPatch, App, Command, Effect, Motion, Then, UiPrefs};

impl App {
    /// Change one thing about the selected agent and say what changed.
    pub(super) fn patch_selected(&mut self, patch: AgentPatch, what: String) -> Vec<Effect> {
        match self.need_agent() {
            Some(id) => {
                self.flash(what);
                vec![Effect::Request {
                    command: Command::UpdateAgent {
                        id,
                        patch,
                        subtree: false,
                        idempotency_key: None,
                    },
                    then: Then::AgentPatched,
                }]
            }
            None => Vec::new(),
        }
    }

    /// `/motion`: still the spinner and make scrolling instant, or restore it.
    pub(super) fn set_motion(&mut self, reduced: bool) {
        self.motion = Motion::new(reduced);
        self.refresh_ui_theme(crate::ui::theme::ThemeSpec {
            reduced_motion: reduced,
            ..self.theme.spec()
        });
        let label = self.motion.label();
        match self.save_prefs(|prefs| prefs.reduced_motion = Some(reduced)) {
            Ok(()) => self.flash(format!("{label} motion")),
            Err(reason) => self.flash_error(crate::ui_format!(
                self.theme,
                "{label} motion · not saved: {reason}",
                "{label} motion | not saved: {reason}"
            )),
        }
    }

    /// Change what is remembered between runs. A save that fails is shown,
    /// never swallowed: the preference still applies to this run.
    pub(crate) fn save_prefs(&self, change: impl FnOnce(&mut UiPrefs)) -> Result<(), String> {
        let Some(dir) = self.options.prefs_dir.as_deref() else {
            return Ok(());
        };
        UiPrefs::update(dir, change).map_err(|e| e.reason)
    }

    /// The selected agent for commands that act on one; explains when none.
    pub(super) fn need_agent(&mut self) -> Option<AgentId> {
        if self.selected.is_none() {
            self.flash(self.theme.decoration(
                "no agent yet · send a message or /new to start one",
                "no agent yet | send a message or /new to start one",
            ));
        }
        self.selected
    }
}

impl App {
    pub(crate) fn set_key(
        &mut self,
        action: crate::keys::KeyAction,
        chord: Option<crate::keys::KeyChord>,
    ) {
        let result = match chord {
            Some(chord) => self.keys.assign(action, chord),
            None => self.keys.reset(action),
        };
        if let Err(error) = result {
            self.flash_error(error);
            return;
        }
        self.theme_epoch = self.theme_epoch.wrapping_add(1);
        let override_value = self.keys.overrides().remove(action.name());
        let saved = self.save_prefs(|prefs| {
            prefs.keys.remove(action.name());
            if let Some(value) = override_value {
                prefs.keys.insert(action.name().into(), value);
            }
        });
        let notice = format!(
            "{}: {}",
            action.label(),
            self.keys.display(action, self.theme.ascii())
        );
        match saved {
            Ok(()) => self.flash(notice),
            Err(error) => self.flash_error(format!("{notice}; not saved: {error}")),
        }
    }

    pub(crate) fn reset_keys(&mut self) {
        self.keys = Default::default();
        self.theme_epoch = self.theme_epoch.wrapping_add(1);
        match self.save_prefs(|prefs| prefs.keys.clear()) {
            Ok(()) => self.flash("All key bindings reset to defaults"),
            Err(error) => self.flash_error(format!("Key bindings reset; not saved: {error}")),
        }
    }
}
