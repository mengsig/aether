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
            SlashCommand::Notifications => ViewKind::Notifications,
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
        let (spec, problem) = self.resolve_theme_choice(&choice);
        if let Some(error) = &problem
            && error.fallback.is_none()
        {
            self.preview_theme(None);
            self.flash_error(error.message.clone());
            return;
        }
        self.committed_theme = None;
        self.refresh_ui_theme(spec);
        let problem = problem.map(|error| error.message);
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

    fn resolve_theme_choice(&self, choice: &ThemeChoice) -> (ThemeSpec, Option<ThemeError>) {
        let (scheme, problem) = match files::scheme(choice, self.options.definitions_dir.as_deref())
        {
            Ok(scheme) => (scheme, None),
            Err(error) => (
                error.fallback.map(|base| base.scheme()).unwrap_or_default(),
                Some(error),
            ),
        };
        let base = self.committed_theme.unwrap_or_else(|| self.theme.spec());
        let spec = ThemeSpec {
            mode: choice.mode,
            scheme,
            ..base
        };
        debug_assert_eq!(spec.palette, base.palette);
        debug_assert_eq!(spec.depth, base.depth);
        (spec, problem)
    }

    pub(crate) fn preview_theme(&mut self, choice: Option<ThemeChoice>) {
        let Some(choice) = choice else {
            if let Some(spec) = self.committed_theme.take() {
                self.refresh_ui_theme(spec);
            }
            return;
        };
        let (spec, _) = self.resolve_theme_choice(&choice);
        let saved = *self.committed_theme.get_or_insert(self.theme.spec());
        debug_assert_eq!(spec.ascii, saved.ascii);
        debug_assert_eq!(spec.reduced_motion, saved.reduced_motion);
        self.refresh_ui_theme(spec);
    }

    /// A picker closed without Enter or Esc (a click, a disconnect) ends its preview.
    pub(crate) fn end_orphaned_theme_preview(&mut self) {
        if self.committed_theme.is_some() && !self.bottom.showing(ViewKind::Theme) {
            self.preview_theme(None);
            debug_assert!(self.committed_theme.is_none());
        }
    }

    pub fn apply_terminal_probe(&mut self, probe: crate::ui::theme::probe::TerminalProbe) {
        use crate::ui::theme::ColorDepth;
        let before = self.theme.spec();
        let depth = match (before.depth, probe.truecolor) {
            (ColorDepth::Indexed256, true) => ColorDepth::Truecolor,
            _ => before.depth,
        };
        self.refresh_ui_theme(ThemeSpec {
            depth,
            palette: probe.palette,
            ..before
        });
        debug_assert_eq!(self.theme.spec().palette, probe.palette);
        debug_assert!(before.depth != ColorDepth::Basic16 || depth == before.depth);
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
