//! What the wizard says and asks, behind a trait so a test can answer.

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use anyhow::{Context as _, bail};

/// A person's answer to "press Enter when done".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wait {
    Done,
    Quit,
}

pub(super) trait Ui {
    /// Whether a person is at the terminal to be asked.
    fn can_ask(&self) -> bool;

    fn say(&mut self, text: &str);

    /// One line the person typed, trimmed.
    fn ask(&mut self, prompt: &str) -> anyhow::Result<String>;

    /// A yes or no question. An empty answer takes `default`.
    fn confirm(&mut self, question: &str, default: bool) -> anyhow::Result<bool> {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        for _ in 0..3 {
            let answer = self
                .ask(&format!("{question} {hint} "))?
                .to_ascii_lowercase();
            match answer.as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.say("Please answer y or n."),
            }
        }
        bail!("no clear answer to \"{question}\"")
    }

    /// Waits for a person to finish something outside the terminal.
    fn wait(&mut self, what: &str) -> anyhow::Result<Wait> {
        let answer = self.ask(&format!("{what}Press Enter when done, or type q to stop: "))?;
        let quit = matches!(answer.to_ascii_lowercase().as_str(), "q" | "quit");
        Ok(if quit { Wait::Quit } else { Wait::Done })
    }
}

/// The real terminal. Text from other programs is cleaned before it is shown.
#[derive(Debug)]
pub(super) struct Terminal {
    tty: bool,
}

impl Terminal {
    pub(super) fn new() -> Self {
        Self {
            tty: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        }
    }
}

impl Ui for Terminal {
    fn can_ask(&self) -> bool {
        self.tty
    }

    fn say(&mut self, text: &str) {
        println!("{}", crate::cli::sane(text));
    }

    fn ask(&mut self, prompt: &str) -> anyhow::Result<String> {
        if !self.tty {
            bail!("a question was asked but no terminal is attached");
        }
        print!("{prompt}");
        std::io::stdout().flush().context("writing the prompt")?;
        let mut line = String::new();
        let read = std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("reading your answer")?;
        if read == 0 {
            bail!("the input closed before you answered");
        }
        Ok(line.trim().to_owned())
    }
}

/// `--json`: nothing is printed until the report, and nobody is asked.
#[derive(Debug)]
pub(super) struct Silent;

impl Ui for Silent {
    fn can_ask(&self) -> bool {
        false
    }

    fn say(&mut self, _text: &str) {}

    fn ask(&mut self, prompt: &str) -> anyhow::Result<String> {
        bail!(
            "`{}` needs an answer but this run cannot ask",
            prompt.trim()
        )
    }
}
