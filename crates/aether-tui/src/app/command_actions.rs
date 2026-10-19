//! Application command actions.
use super::{AgentPatch, App, ArchiveTarget, Command, Effect, SlashCommand, Then};

impl App {
    /// Commands without a view of their own. `run_slash` opens the views and
    /// handles the rest of the redesign's commands; they are routed back there.
    pub(super) fn run_command(&mut self, cmd: SlashCommand) -> Vec<Effect> {
        if self.options.is_remote()
            && matches!(
                cmd,
                SlashCommand::Login { .. } | SlashCommand::Logout(_) | SlashCommand::Voice(_)
            )
        {
            self.flash_error("provider login/logout and voice are local-only; run aether --local on the host instead");
            return Vec::new();
        }
        match cmd {
            SlashCommand::ViewMode(_) => self.run_slash(cmd),
            SlashCommand::Voice(action) => self.voice_action(action),
            SlashCommand::New { objective } => {
                let here = self.active_workspace().map(std::path::Path::to_path_buf);
                if self.options.is_remote() && here.is_none() {
                    self.flash_error("choose a host workspace with /workspace first");
                    return Vec::new();
                }
                vec![self.start_new_root(objective.map(Into::into), here)]
            }
            SlashCommand::Workspace(Some(path)) => self.open_workspace(&path),
            SlashCommand::Model {
                chain,
                default_only,
            } => self.use_model(chain, default_only),
            SlashCommand::Effort(effort) => self.use_effort(effort),
            SlashCommand::Image(arg) => self.run_image_command(arg),
            SlashCommand::Copy => match self.last_reply() {
                Some(text) => vec![Effect::Copy(text)],
                None => {
                    self.flash("nothing to copy yet");
                    Vec::new()
                }
            },
            SlashCommand::Instructions(text) => self.patch_selected(
                AgentPatch {
                    instructions: Some(text),
                    ..Default::default()
                },
                "standing instructions replaced".to_owned(),
            ),
            SlashCommand::Logout(provider) => vec![Effect::RunExternal {
                args: vec!["logout".into(), provider],
            }],
            SlashCommand::Motion(reduced) => {
                self.set_motion(reduced.unwrap_or(!self.motion.reduced()));
                Vec::new()
            }
            SlashCommand::Login {
                provider: Some(provider),
            } => {
                self.flash(crate::ui_format!(
                    self.theme,
                    "logging in to {provider}…",
                    "logging in to {provider}~"
                ));
                vec![Effect::RunExternal {
                    args: vec!["login".into(), provider],
                }]
            }
            SlashCommand::Rename(name) => match self.need_agent() {
                Some(id) => vec![Effect::Request {
                    command: Command::UpdateAgent {
                        id,
                        patch: AgentPatch {
                            name: Some(name),
                            ..Default::default()
                        },
                        idempotency_key: None,
                    },
                    then: Then::AgentRenamed(id),
                }],
                None => Vec::new(),
            },
            SlashCommand::OperatorRemember { text, scope } => {
                self.create_operator_memory(text, scope)
            }
            SlashCommand::Compact => match self.need_agent() {
                Some(agent) => vec![Effect::Request {
                    command: Command::Compact { agent },
                    then: Then::CompactionAccepted(agent),
                }],
                None => Vec::new(),
            },
            SlashCommand::Stop => match self.need_agent() {
                Some(agent) => self.interrupt_agent(agent),
                None => Vec::new(),
            },
            SlashCommand::Yolo => self.toggle_yolo(),
            SlashCommand::Restart => match self.need_agent() {
                Some(agent) => vec![Effect::Request {
                    command: Command::Restart {
                        agent,
                        idempotency_key: None,
                    },
                    then: Then::Restarted { from: agent },
                }],
                None => Vec::new(),
            },
            SlashCommand::Wake => match self.need_agent() {
                Some(agent) => vec![Effect::Request {
                    command: Command::Wake { agent },
                    then: Then::Woken,
                }],
                None => Vec::new(),
            },
            SlashCommand::Pause { subtree } => match self.need_agent() {
                Some(agent) => vec![Effect::Request {
                    command: Command::Pause {
                        agent,
                        subtree,
                        idempotency_key: None,
                    },
                    then: Then::Paused(agent),
                }],
                None => Vec::new(),
            },
            SlashCommand::Resume { subtree } => match self.need_agent() {
                Some(agent) => vec![Effect::Request {
                    command: Command::Resume {
                        agent,
                        subtree,
                        idempotency_key: None,
                    },
                    then: Then::Resumed(agent),
                }],
                None => Vec::new(),
            },
            SlashCommand::Clear => {
                self.flash(self.theme.decoration(
                    "clearing finished work out of the view…",
                    "clearing finished work out of the view~",
                ));
                vec![Effect::Request {
                    command: Command::Archive {
                        target: ArchiveTarget::AllFinished,
                        archived: true,
                        idempotency_key: None,
                    },
                    then: Then::Archived,
                }]
            }
            SlashCommand::Models
            | SlashCommand::Efforts
            | SlashCommand::Usage
            | SlashCommand::Providers
            | SlashCommand::Rules
            | SlashCommand::Memory
            | SlashCommand::Notifications
            | SlashCommand::Schedules
            | SlashCommand::Mcp
            | SlashCommand::Outbox
            | SlashCommand::Doctor
            | SlashCommand::Report
            | SlashCommand::Artifacts
            | SlashCommand::Settings
            | SlashCommand::Theme
            | SlashCommand::Workspace(None)
            | SlashCommand::SetTheme(_)
            | SlashCommand::Mouse(_)
            | SlashCommand::Login { provider: None }
            | SlashCommand::Agent { .. }
            | SlashCommand::Cancel
            | SlashCommand::Help
            | SlashCommand::Quit => self.run_slash(cmd),
            SlashCommand::Unknown(name) => {
                self.flash(crate::ui_format!(
                    self.theme,
                    "unknown command /{name} · try /help",
                    "unknown command /{name} | try /help"
                ));
                Vec::new()
            }
            SlashCommand::Invalid(msg) => {
                self.flash(msg);
                Vec::new()
            }
        }
    }
}
