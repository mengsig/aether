//! The redesign's slash-command path: views open in the bottom pane (W7),
//! everything without a view keeps `run_command`'s behaviour. Owner: W6.

use super::confirm::ConfirmCancel;
use crate::app::{App, Effect};
use crate::commands::SlashCommand;
use crate::prefs::MouseMode;
use crate::ui::theme::ThemeChoice;
use crate::ui::theme::files::{self, ThemeError};
use crate::ui::theme::tokens::{Theme, ThemeSpec};
use crate::ui::views::ViewKind;
use crate::voice::Action;

impl App {
    pub(crate) fn run_slash(&mut self, command: SlashCommand) -> Vec<Effect> {
        let view = match command {
            SlashCommand::ViewMode(None) => ViewKind::Modes,
            SlashCommand::Models | SlashCommand::Efforts => ViewKind::Model,
            SlashCommand::Usage | SlashCommand::Providers => ViewKind::Usage,
            SlashCommand::Rules => ViewKind::Approvals,
            SlashCommand::Memory => ViewKind::Memory,
            SlashCommand::Schedules => ViewKind::Schedules,
            SlashCommand::Mcp => ViewKind::Mcp,
            SlashCommand::Outbox => ViewKind::Outbox,
            SlashCommand::Doctor => ViewKind::Doctor,
            SlashCommand::Report => ViewKind::Report,
            SlashCommand::Artifacts => ViewKind::Artifacts,
            SlashCommand::Settings => ViewKind::Settings,
            SlashCommand::Theme => ViewKind::Theme,
            SlashCommand::Workspace(None) => ViewKind::Workspace,
            SlashCommand::Voice(Action::Setup) => ViewKind::Voice,
            SlashCommand::Login { provider: None } => ViewKind::Login,
            other => return self.run_slash_action(other),
        };
        self.open_view(view)
    }

    fn run_slash_action(&mut self, command: SlashCommand) -> Vec<Effect> {
        match command {
            SlashCommand::ViewMode(Some(name)) if name == "reload" => self.reload_modes(),
            SlashCommand::ViewMode(Some(name)) => {
                self.set_view_mode(&name);
                Vec::new()
            }
            SlashCommand::Agent { name: None } => {
                self.open_and_focus_board();
                Vec::new()
            }
            SlashCommand::Agent { name: Some(name) } => self.view_agent_named(&name),
            SlashCommand::Cancel => {
                if let Some(agent) = self.need_agent() {
                    let name = self.agent_name(agent);
                    let title = format!("End {name}?");
                    self.bottom.push_view(Box::new(ConfirmCancel::action(
                        agent,
                        name,
                        title,
                        "Ends this agent and all live descendants permanently.".to_owned(),
                        false,
                        false,
                    )));
                }
                Vec::new()
            }
            SlashCommand::Mouse(mode) => {
                let toggled = if self.mouse == MouseMode::Off {
                    MouseMode::Scroll
                } else {
                    MouseMode::Off
                };
                self.set_mouse_mode(mode.unwrap_or(toggled))
            }
            SlashCommand::SetTheme(choice) => {
                self.set_theme(choice);
                Vec::new()
            }
            SlashCommand::Help => {
                self.toggle_shortcuts();
                Vec::new()
            }
            SlashCommand::Quit => self.detach(),
            other => self.run_command(other),
        }
    }

    /// `/agent <name>`: the board's name, any case, so `worker 2` is the second
    /// `worker`. A name still shared (across parents) takes the oldest.
    pub(crate) fn view_agent_named(&mut self, name: &str) -> Vec<Effect> {
        let names = crate::app::sibling_names(&self.tree);
        let found = self
            .tree
            .nodes
            .values()
            .filter(|node| {
                let agent = &node.agent;
                names
                    .get(&agent.id)
                    .unwrap_or(&agent.name)
                    .eq_ignore_ascii_case(name)
            })
            .min_by_key(|node| (node.agent.created_at, node.agent.id))
            .map(|node| node.agent.id);
        match found {
            Some(agent) => self.select(agent),
            None => {
                self.flash(format!("no agent named {name}"));
                Vec::new()
            }
        }
    }

    pub(crate) fn set_mouse_mode(&mut self, mode: MouseMode) -> Vec<Effect> {
        self.mouse = mode;
        let dot = self.theme.glyphs().dot;
        let label = match mode {
            MouseMode::Off => format!("mouse off {dot} the terminal selects text"),
            MouseMode::Scroll | MouseMode::Full => {
                format!("mouse on {dot} drag to select and copy; shift+drag selects in terminal")
            }
        };
        match self.save_prefs(|prefs| prefs.mouse = Some(mode)) {
            Ok(()) => self.flash(label),
            Err(reason) => self.flash_error(format!("{label} {dot} not saved: {reason}")),
        }
        vec![Effect::SetMouse(mode != MouseMode::Off)]
    }

    /// Paint with `choice` and remember it. A theme file is read now: a
    /// missing one changes nothing, a broken one paints its base, both said.
    fn set_theme(&mut self, choice: ThemeChoice) {
        let (scheme, problem) =
            match files::scheme(&choice, self.options.definitions_dir.as_deref()) {
                Ok(scheme) => (scheme, None),
                Err(ThemeError {
                    message,
                    fallback: Some(base),
                }) => (base.scheme(), Some(message)),
                Err(ThemeError {
                    message,
                    fallback: None,
                }) => {
                    self.flash_error(message);
                    return;
                }
            };
        self.refresh_ui_theme(ThemeSpec {
            mode: choice.mode,
            scheme,
            ..self.theme.spec()
        });
        // Auto is "no saved theme": the next launch probes the terminal again.
        let stored = (choice != ThemeChoice::default()).then(|| choice.clone());
        let label = format!("theme {choice}");
        self.theme_choice = choice;
        let saved = self.save_prefs(|prefs| prefs.theme = stored);
        let dot = self.theme.glyphs().dot;
        match (problem, saved) {
            (Some(problem), Ok(())) => self.flash_error(problem),
            (Some(problem), Err(reason)) => {
                self.flash_error(format!("{problem} {dot} not saved: {reason}"))
            }
            (None, Ok(())) => self.flash(label),
            (None, Err(reason)) => self.flash_error(format!("{label} {dot} not saved: {reason}")),
        }
    }

    /// What the startup probe read from the terminal; `None` keeps the fallbacks.
    pub fn set_palette(&mut self, palette: Option<crate::ui::theme::tokens::TerminalPalette>) {
        self.refresh_ui_theme(ThemeSpec {
            palette,
            ..self.theme.spec()
        });
    }

    /// Every layout cache keys on `theme_epoch`, so any token change bumps it.
    pub(crate) fn refresh_ui_theme(&mut self, spec: ThemeSpec) {
        self.theme = Theme::resolve(spec);
        self.theme_epoch = self.theme_epoch.wrapping_add(1);
        self.dirty = true;
    }
}
