//! Slash commands a host program (Archon) adds with `aether tui --host-commands`.
//! The TUI lists them; the host daemon behind the attached agent runs them.

use std::path::Path;

use serde::Deserialize;

/// The only file version this build reads.
const VERSION: u32 = 1;
/// More than any host needs; a bound keeps a runaway file out of the popup.
const MAX_COMMANDS: usize = 64;
const MAX_NAME: usize = 32;
const MAX_TEXT: usize = 200;
const MAX_WELCOME_TITLE: usize = 40;
const MAX_WELCOME_LINES: usize = 8;
/// Shown after each host command's about text when the file names no host.
const DEFAULT_HOST: &str = "host";

/// One command the host understands, listed like a built-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    pub name: String,
    /// Argument hint, shown after the name; `<...>` means one is required.
    pub args: String,
    pub about: String,
}

/// The host's greeting for the header: its name for the session and a few
/// plain lines, already cleaned and bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    pub title: String,
    pub lines: Vec<String>,
}

/// What a host-commands file yielded. `warning` is one line for the operator
/// naming every refused entry, or why the whole file was ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCommands {
    /// Marks the commands in the popup and help (`· archon`).
    pub host: String,
    pub commands: Vec<HostCommand>,
    pub welcome: Option<Welcome>,
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    host: Option<String>,
    commands: Vec<Entry>,
    #[serde(default)]
    welcome: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawWelcome {
    title: String,
    #[serde(default)]
    lines: Vec<String>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    about: String,
}

impl HostCommands {
    pub fn find(&self, name: &str) -> Option<&HostCommand> {
        self.commands
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The help heading for these commands.
    pub fn heading(&self) -> String {
        format!("Host commands ({})", self.host)
    }

    /// `(usage, about)` per command, as `/help` lists them.
    pub fn help_rows(&self) -> impl Iterator<Item = (String, &str)> {
        self.commands.iter().map(|c| {
            let usage = if c.args.is_empty() {
                format!("/{}", c.name)
            } else {
                format!("/{} {}", c.name, c.args)
            };
            (usage, c.about.as_str())
        })
    }

    /// Drop each command `taken` claims, naming it in `warning`, so a
    /// listed host command never runs something local instead.
    pub fn refuse_taken(&mut self, taken: impl Fn(&str) -> bool, reason: &str) {
        let before = self.commands.len();
        let mut refused = Vec::new();
        self.commands.retain(|c| {
            let free = !taken(&c.name);
            if !free {
                refused.push(format!("/{} refused: {reason}", c.name));
            }
            free
        });
        debug_assert_eq!(before, self.commands.len() + refused.len());
        if refused.is_empty() {
            return;
        }
        let refused = refused.join("; ");
        self.warning = Some(match self.warning.take() {
            Some(warning) => format!("{warning}; {refused}"),
            None => format!("host commands: {refused}"),
        });
    }

    /// Read `path`. Never an error: a missing or broken file leaves no
    /// commands and a warning, so the TUI still starts.
    pub fn load(path: &Path) -> Self {
        let failed = |reason: String| HostCommands {
            host: DEFAULT_HOST.into(),
            commands: Vec::new(),
            welcome: None,
            warning: Some(format!(
                "host commands from {} ignored: {reason}",
                path.display()
            )),
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => return failed(error.to_string()),
        };
        match Self::parse(&text) {
            Ok(mut loaded) => {
                loaded.warning = loaded
                    .warning
                    .map(|w| format!("host commands from {}: {w}", path.display()));
                loaded
            }
            Err(reason) => failed(reason),
        }
    }

    /// Parse a file's text. `Err` refuses the whole file; a bad entry only
    /// drops that entry and is named in `warning`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        if file.version != VERSION {
            return Err(format!(
                "version {} is not supported (expected {VERSION})",
                file.version
            ));
        }
        if file.commands.len() > MAX_COMMANDS {
            return Err(format!("more than {MAX_COMMANDS} commands"));
        }
        let mut refused = Vec::new();
        let host = match file.host {
            None => DEFAULT_HOST.to_owned(),
            Some(host) if valid_name(&host) => host,
            Some(host) => {
                refused.push(format!(
                    "host {:?} is not a plain lowercase name",
                    sane(&host)
                ));
                DEFAULT_HOST.to_owned()
            }
        };
        let mut commands: Vec<HostCommand> = Vec::new();
        for entry in file.commands {
            let name = entry.name.clone();
            match check_entry(&entry, &commands) {
                Ok(()) => commands.push(HostCommand {
                    name: entry.name,
                    args: entry.args,
                    about: entry.about,
                }),
                Err(reason) => refused.push(format!("/{} refused: {reason}", sane(&name))),
            }
        }
        let welcome = file.welcome.and_then(|raw| read_welcome(raw, &mut refused));
        debug_assert!(commands.iter().all(|c| super::spec_for(&c.name).is_none()));
        debug_assert!(commands.len() <= MAX_COMMANDS);
        Ok(HostCommands {
            host,
            commands,
            welcome,
            warning: (!refused.is_empty()).then(|| refused.join("; ")),
        })
    }
}

/// The welcome block from its loose JSON; a wrong shape drops only the block.
fn read_welcome(value: serde_json::Value, refused: &mut Vec<String>) -> Option<Welcome> {
    match serde_json::from_value::<RawWelcome>(value) {
        Ok(raw) => clean_welcome(raw, refused),
        Err(error) => {
            refused.push(format!("welcome ignored: {error}"));
            None
        }
    }
}

/// Strip control characters and clamp to the limits, naming each cut in
/// `refused`. A title left empty drops the whole block.
fn clean_welcome(raw: RawWelcome, refused: &mut Vec<String>) -> Option<Welcome> {
    let title = clamp(&raw.title, MAX_WELCOME_TITLE);
    if title.is_empty() {
        refused.push("welcome ignored: it has no title".into());
        return None;
    }
    if title.chars().count() < raw.title.chars().count() {
        refused.push(format!(
            "welcome title cut to {MAX_WELCOME_TITLE} characters"
        ));
    }
    if raw.lines.len() > MAX_WELCOME_LINES {
        refused.push(format!("welcome keeps only {MAX_WELCOME_LINES} lines"));
    }
    let mut cut = false;
    let lines: Vec<String> = raw
        .lines
        .iter()
        .take(MAX_WELCOME_LINES)
        .map(|line| {
            let kept = clamp(line, MAX_TEXT);
            cut |= kept.chars().count() < line.chars().count();
            kept
        })
        .collect();
    if cut {
        refused.push(format!("welcome lines cut to {MAX_TEXT} characters"));
    }
    debug_assert!(lines.len() <= MAX_WELCOME_LINES);
    debug_assert!(!title.chars().any(char::is_control));
    Some(Welcome { title, lines })
}

/// `text` without control characters, at most `max` characters.
fn clamp(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

fn check_entry(entry: &Entry, accepted: &[HostCommand]) -> Result<(), &'static str> {
    if !valid_name(&entry.name) {
        return Err("a name is lowercase [a-z][a-z0-9-]* without a slash");
    }
    if super::spec_for(&entry.name).is_some() {
        return Err("shadows a built-in command");
    }
    if accepted.iter().any(|c| c.name == entry.name) {
        return Err("listed twice");
    }
    if !plain_text(&entry.args) || !plain_text(&entry.about) {
        return Err("args and about must be one line of printable text");
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn plain_text(text: &str) -> bool {
    text.chars().count() <= MAX_TEXT && !text.chars().any(char::is_control)
}

/// A name from the file as it may be shown: control characters dropped, bounded.
fn sane(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect()
}

#[cfg(test)]
mod tests;
