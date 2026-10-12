//! Slash commands typed into the chat input.

use aether_core::model::{Effort, ModelChain};

use crate::prefs::MouseMode;
use crate::ui::theme::ThemeChoice;

#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    ViewMode(Option<String>),
    Voice(crate::voice::Action),
    Rename(String),
    OperatorRemember {
        text: String,
        scope: RememberScope,
    },
    New {
        objective: Option<String>,
    },
    /// `/workspace` opens the picker; with a path, switch there directly.
    Workspace(Option<std::path::PathBuf>),
    /// Set the current agent's model for its next turn, or - with `default_only`, from
    /// `/model --default <ref>` - set the session default without touching
    /// the agent in front of you.
    Model {
        chain: ModelChain,
        default_only: bool,
    },
    Effort(Effort),
    /// Open the effort picker.
    Efforts,
    /// Cancel the agent for good.
    Cancel,
    /// Interrupt the running turn; the agent stays.
    Stop,
    Compact,
    /// Log in to a provider from inside the app (hands off to the login flow).
    Login {
        provider: Option<String>,
    },
    /// Pause the selected agent; with `subtree`, everything beneath it too.
    Pause {
        subtree: bool,
    },
    /// Lift a pause, or clear a wait or an inspection.
    Resume {
        subtree: bool,
    },
    /// Recreate the selected agent from its last checkpoint after it failed.
    Restart,
    /// Wake the selected agent in place after it failed, history and all.
    Wake,
    /// Clear finished agents and runs out of the tree, chat and timeline.
    /// They stay in the store, in reports and in replay.
    Clear,
    /// `/approvals`: remembered decisions, revoke, the auto-approve toggle.
    Rules,
    Schedules,
    /// Configured MCP servers, their health and their tools.
    Mcp,
    /// Remote operations whose delivery the host could not confirm.
    Outbox,
    /// Durable memories, their provenance and recent curation activity.
    Memory,
    /// `/agent` opens the board; `/agent <name>` views that agent.
    Agent {
        name: Option<String>,
    },
    Models,
    /// Every provider, its auth and its live health.
    Providers,
    /// Account quota windows reported by each configured provider.
    Usage,
    /// Theme, motion, mouse and ascii in one view.
    Settings,
    /// Set mouse capture; `None` toggles off and wheel scrolling.
    Mouse(Option<MouseMode>),
    /// Toggle approvals off/on for the selected agent.
    Yolo,
    /// Copy the last reply to the clipboard.
    Copy,
    /// Attach an image to the draft, or drop the ones attached.
    Image(ImageArg),
    /// `/theme` with no argument: the theme picker.
    Theme,
    /// `/theme <name>`: a built-in (`auto`, `nord-light`, …) or a theme file.
    SetTheme(ThemeChoice),
    /// Full or reduced motion. `None` toggles.
    Motion(Option<bool>),
    /// Replace the selected agent's standing instructions.
    Instructions(String),
    /// Remove stored credentials for a provider.
    Logout(String),
    /// What is and is not working: connection, sign-in, paths, versions.
    Doctor,
    /// What the selected agent's run did: agents, tools, cost, open loops.
    Report,
    /// The selected agent's artifacts; Enter opens one in the pager.
    Artifacts,
    Help,
    /// Leave the client; agents keep running in the daemon.
    Quit,
    Unknown(String),
    /// Well-formed command with a bad argument; carries the message to show.
    Invalid(String),
}

/// What `/image` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageArg {
    /// No argument: the image on the system clipboard.
    Clipboard,
    /// A path as typed; `~` and a relative path are resolved when it runs.
    File(String),
    /// `--clear`: remove every attached image.
    Clear,
}

/// Whether archived work is shown (the board's `.`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowFilter {
    /// Bring cleared work back into the view.
    Archived,
    /// Hide it again.
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberScope {
    Agent,
    Global,
    Repository,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    Empty,
    Message(String),
    Command(SlashCommand),
}

/// Interpret submitted input text.
pub fn parse(text: &str) -> InputAction {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return InputAction::Empty;
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return InputAction::Message(trimmed.to_owned());
    };
    let (name, arg) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    let arg_opt = (!arg.is_empty()).then(|| arg.to_owned());
    let name = name.to_ascii_lowercase();
    // The registry is the vocabulary. A name it does not carry never reaches
    // the arms below, so the palette cannot fall behind the parser.
    if spec_for(&name).is_none() {
        return InputAction::Command(SlashCommand::Unknown(name));
    }
    let command = match name.as_str() {
        "voice" => crate::voice::Action::parse(arg)
            .map(SlashCommand::Voice)
            .unwrap_or_else(|| {
                SlashCommand::Invalid(
                    "usage: /voice [status|setup|check|on|off|mute|replies on|replies off|start|stop|cancel|insert]"
                        .into(),
                )
            }),
        "new" => SlashCommand::New { objective: arg_opt },
        "workspace" | "cd" | "ws" | "workspaces" => {
            SlashCommand::Workspace(arg_opt.map(std::path::PathBuf::from))
        }
        "rename" => match arg_opt {
            Some(name) => SlashCommand::Rename(name),
            None => SlashCommand::Invalid("usage: /rename <new name> (selected agent)".into()),
        },
        "remember" => parse_remember(arg),
        "model" => {
            let bad = || {
                SlashCommand::Invalid(
                    "usage: /model [--default] [provider/model[,provider/model...]]".into(),
                )
            };
            // `--defaultfoo` is not `--default foo`: only a bare flag or one
            // followed by whitespace counts, so a model ref that happens to
            // start with "default" (or a typo) is not silently mistaken for it.
            let (default_only, rest) = match arg.strip_prefix("--default") {
                Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
                    (true, rest.trim())
                }
                _ => (false, arg),
            };
            match (rest.is_empty(), ModelChain::parse(rest)) {
                // Bare `/model` opens the picker; `--default` alone is a typo.
                (true, _) if default_only => bad(),
                (true, _) => SlashCommand::Models,
                (_, Some(chain)) => SlashCommand::Model {
                    chain,
                    default_only,
                },
                (_, None) => bad(),
            }
        }
        "effort" => match arg_opt.as_deref() {
            None => SlashCommand::Efforts,
            Some(arg) => match arg.parse::<Effort>() {
                Ok(effort) => SlashCommand::Effort(effort),
                Err(_) => {
                    SlashCommand::Invalid("usage: /effort [low|medium|high|xhigh|max]".into())
                }
            },
        },
        "cancel" | "kill" => SlashCommand::Cancel,
        "stop" | "interrupt" => SlashCommand::Stop,
        "compact" | "checkpoint" => SlashCommand::Compact,
        "restart" => SlashCommand::Restart,
        "wake" => SlashCommand::Wake,
        "pause" | "resume" => match subtree_arg(arg) {
            Some(subtree) if name.eq_ignore_ascii_case("pause") => SlashCommand::Pause { subtree },
            Some(subtree) => SlashCommand::Resume { subtree },
            None => SlashCommand::Invalid(format!("usage: /{name} [subtree]")),
        },
        "clear" => SlashCommand::Clear,
        "approvals" | "rules" => SlashCommand::Rules,
        "mcp" => SlashCommand::Mcp,
        "outbox" => SlashCommand::Outbox,
        "memory" | "memories" => SlashCommand::Memory,
        "agent" | "agents" => SlashCommand::Agent { name: arg_opt },
        "models" => SlashCommand::Models,
        "schedules" => SlashCommand::Schedules,
        "providers" => SlashCommand::Providers,
        "usage" if arg.is_empty() => SlashCommand::Usage,
        "usage" => SlashCommand::Invalid("usage: /usage".into()),
        "mode" => SlashCommand::ViewMode(arg_opt),
        "settings" => SlashCommand::Settings,
        "mouse" => match arg.to_ascii_lowercase().as_str() {
            "" => SlashCommand::Mouse(None),
            "off" => SlashCommand::Mouse(Some(MouseMode::Off)),
            "scroll" => SlashCommand::Mouse(Some(MouseMode::Scroll)),
            "full" => SlashCommand::Mouse(Some(MouseMode::Full)),
            _ => SlashCommand::Invalid("usage: /mouse [off|scroll|full]".into()),
        },
        "yolo" => SlashCommand::Yolo,
        "copy" => SlashCommand::Copy,
        "image" | "img" | "attach" => SlashCommand::Image(match arg_opt {
            None => ImageArg::Clipboard,
            Some(flag) if flag == "--clear" => ImageArg::Clear,
            Some(path) => ImageArg::File(path),
        }),
        "instructions" | "standing" => match arg_opt {
            Some(text) => SlashCommand::Instructions(text),
            None => SlashCommand::Invalid("usage: /instructions <standing instructions>".into()),
        },
        "logout" => match arg_opt {
            Some(provider) => SlashCommand::Logout(provider),
            None => SlashCommand::Invalid("usage: /logout <provider>".into()),
        },
        "doctor" => SlashCommand::Doctor,
        "report" => SlashCommand::Report,
        "artifacts" => SlashCommand::Artifacts,
        "motion" => match arg_opt.as_deref() {
            None => SlashCommand::Motion(None),
            Some(a) if a.eq_ignore_ascii_case("reduced") || a.eq_ignore_ascii_case("off") => {
                SlashCommand::Motion(Some(true))
            }
            Some(a) if a.eq_ignore_ascii_case("full") || a.eq_ignore_ascii_case("on") => {
                SlashCommand::Motion(Some(false))
            }
            Some(_) => SlashCommand::Invalid("usage: /motion [full|reduced]".into()),
        },
        "theme" => match arg_opt.as_deref() {
            None => SlashCommand::Theme,
            Some(a) => match a.parse::<ThemeChoice>() {
                Ok(choice) => SlashCommand::SetTheme(choice),
                Err(reason) => SlashCommand::Invalid(reason),
            },
        },
        "login" => SlashCommand::Login { provider: arg_opt },
        "help" | "?" => SlashCommand::Help,
        "quit" | "exit" | "q" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_owned()),
    };
    InputAction::Command(command)
}

fn parse_remember(arg: &str) -> SlashCommand {
    let (first, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
    let (scope, text) = match first {
        "--agent" | "--" => (RememberScope::Agent, rest.trim()),
        "--global" => (RememberScope::Global, rest.trim()),
        "--repository" => (RememberScope::Repository, rest.trim()),
        flag if flag.starts_with("--") => (RememberScope::Agent, ""),
        _ => (RememberScope::Agent, arg),
    };
    if text.is_empty() {
        return SlashCommand::Invalid(
            "usage: /remember [--agent|--global|--repository] <durable fact>; default: selected agent; -- escapes a fact starting with --".into(),
        );
    }
    SlashCommand::OperatorRemember {
        text: text.into(),
        scope,
    }
}

/// The only argument `/pause` and `/resume` take. `None` is a typo, not a
/// silent whole-subtree pause.
fn subtree_arg(arg: &str) -> Option<bool> {
    match arg {
        "" => Some(false),
        a if a.eq_ignore_ascii_case("subtree") || a.eq_ignore_ascii_case("--subtree") => Some(true),
        _ => None,
    }
}

mod keys;
mod registry;

pub use keys::{KEYS, KeyGroup, KeyScope, KeySpec, MAX_LABEL_WIDTH};
pub use registry::{
    COMMANDS, CommandSpec, help_commands, help_commands_ascii, palette_matches, spec_for,
};

#[cfg(test)]
mod tests;
