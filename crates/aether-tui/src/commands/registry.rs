/// One entry of the `/` popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    /// Optional argument, shown after the name.
    pub usage: &'static str,
    pub about: &'static str,
    /// Fixed popup position, 1-based; [`COMMANDS`] is in this order.
    pub rank: u8,
    /// Older names [`super::parse`] still accepts; never listed.
    pub hidden_aliases: &'static [&'static str],
    /// The chord that does the same thing, if there is one; `tests/registry`
    /// keeps it pointing at a row of [`super::KEYS`].
    pub key: &'static str,
}

/// The command `name` belongs to, by its own name or a hidden alias.
/// [`super::parse`] resolves through this, so the two can never disagree.
pub fn spec_for(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| {
        c.name.eq_ignore_ascii_case(name)
            || c.hidden_aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(name))
    })
}

/// Every listed slash command, in rank order (SPEC §6.2). Each works without arguments.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "model",
        usage: "[--default] [provider/model]",
        about: "choose the model and effort; --default sets new agents",
        rank: 1,
        hidden_aliases: &["models", "effort"],
        key: "",
    },
    CommandSpec {
        name: "new",
        usage: "[objective]",
        about: "start a new root agent",
        rank: 2,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "workspace",
        usage: "[path]",
        about: "switch workspace: view its agent, or start one there",
        rank: 3,
        hidden_aliases: &["cd", "ws", "workspaces"],
        key: "Ctrl+O",
    },
    CommandSpec {
        name: "agent",
        usage: "[name]",
        about: "open the agents board, or view the named agent",
        rank: 4,
        hidden_aliases: &["agents"],
        key: "Ctrl+G",
    },
    CommandSpec {
        name: "compact",
        usage: "",
        about: "fold older context into a checkpoint",
        rank: 5,
        hidden_aliases: &["checkpoint"],
        key: "",
    },
    CommandSpec {
        name: "copy",
        usage: "",
        about: "copy the last reply to the clipboard",
        rank: 6,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "memory",
        usage: "",
        about: "browse durable memories; pin or forget one",
        rank: 7,
        hidden_aliases: &["memories"],
        key: "",
    },
    CommandSpec {
        name: "approvals",
        usage: "",
        about: "remembered approval rules and the auto-approve toggle",
        rank: 8,
        hidden_aliases: &["rules", "yolo"],
        key: "",
    },
    CommandSpec {
        name: "usage",
        usage: "",
        about: "providers, sign-in, health and account limits",
        rank: 9,
        hidden_aliases: &["providers"],
        key: "",
    },
    CommandSpec {
        name: "settings",
        usage: "",
        about: "theme, motion, mouse and ascii",
        rank: 10,
        hidden_aliases: &["theme", "motion", "mouse"],
        key: "",
    },
    CommandSpec {
        name: "remember",
        usage: "[--agent|--global|--repository] <durable fact>",
        about: "record an operator fact; default: the viewed agent only",
        rank: 11,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "stop",
        usage: "",
        about: "interrupt the current turn; the agent keeps its context",
        rank: 12,
        hidden_aliases: &["interrupt"],
        key: "Esc",
    },
    CommandSpec {
        name: "cancel",
        usage: "",
        about: "end the viewed agent for good (asks first)",
        rank: 13,
        hidden_aliases: &["kill"],
        key: "",
    },
    CommandSpec {
        name: "pause",
        usage: "[subtree]",
        about: "pause the viewed agent, or everything under it, after its turn",
        rank: 14,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "resume",
        usage: "[subtree]",
        about: "resume a paused agent, or everything paused under it",
        rank: 15,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "wake",
        usage: "",
        about: "wake a failed agent in place, keeping its history",
        rank: 16,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "restart",
        usage: "",
        about: "recreate a failed agent from its last checkpoint",
        rank: 17,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "rename",
        usage: "<new name>",
        about: "rename the viewed agent",
        rank: 18,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "instructions",
        usage: "<text>",
        about: "replace the viewed agent's standing instructions",
        rank: 19,
        hidden_aliases: &["standing"],
        key: "",
    },
    CommandSpec {
        name: "artifacts",
        usage: "",
        about: "the viewed agent's artifacts; Enter opens one",
        rank: 20,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "mcp",
        usage: "",
        about: "MCP servers, their health and their tools; r reconnects",
        rank: 21,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "schedules",
        usage: "",
        about: "recurring timers and their history; pause, resume or cancel",
        rank: 22,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "report",
        usage: "",
        about: "what this agent's run did: agents, tools, cost, open loops",
        rank: 23,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "doctor",
        usage: "",
        about: "what is and is not working: connection, sign-in, paths",
        rank: 24,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "voice",
        usage: "[status|setup|check|on|off|mute|replies on|replies off|start|stop|cancel|insert]",
        about: "local dictation and spoken replies",
        rank: 25,
        hidden_aliases: &[],
        key: "F4",
    },
    CommandSpec {
        name: "login",
        usage: "[provider]",
        about: "sign in: Claude or ChatGPT subscription, or an API key",
        rank: 26,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "logout",
        usage: "<provider>",
        about: "remove stored credentials for a provider",
        rank: 27,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "clear",
        usage: "",
        about: "clear finished work out of the view; it stays in the store",
        rank: 28,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "help",
        usage: "",
        about: "keys and commands",
        rank: 29,
        hidden_aliases: &["?"],
        key: "?",
    },
    CommandSpec {
        name: "quit",
        usage: "",
        about: "leave the client; agents keep running in the daemon",
        rank: 30,
        hidden_aliases: &["exit", "q"],
        key: "Ctrl+C",
    },
    CommandSpec {
        name: "mode",
        usage: "[name|reload]",
        about: "choose a transcript view mode",
        rank: 31,
        hidden_aliases: &[],
        key: "",
    },
    CommandSpec {
        name: "outbox",
        usage: "",
        about: "remote operations the host cannot confirm; deliver, resend or discard",
        rank: 32,
        hidden_aliases: &[],
        key: "",
    },
];

/// Palette entries matching what is typed so far: a `/` followed by part of
/// a command name and nothing else. Empty once an argument is being typed.
pub fn palette_matches(input: &str) -> Vec<&'static CommandSpec> {
    let Some(rest) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if rest.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let rest = rest.to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&rest))
        .collect()
}

/// Every command with its palette text, for the help overlay and the docs.
pub fn help_commands() -> impl Iterator<Item = (String, &'static str)> {
    help_commands_ascii(false)
}

pub fn help_commands_ascii(ascii: bool) -> impl Iterator<Item = (String, &'static str)> {
    COMMANDS.iter().map(move |c| {
        let name = if c.usage.is_empty() {
            format!("/{}", c.name)
        } else {
            format!("/{} {}", c.name, c.usage)
        };
        (name, c.display_about(ascii))
    })
}

impl CommandSpec {
    /// Every `about` is plain ASCII, so both modes read the same.
    pub fn display_about(&self, ascii: bool) -> &'static str {
        let _ = ascii;
        self.about
    }
}
