//! `--plain`: conversation output and prompts, as lines.
//!
//! No colour, no glyphs, no motion, no cursor addressing — nothing a screen
//! reader has to skip or a log file has to strip. It is not a second client:
//! frames go through the same [`App`] reducer the TUI uses, and this module
//! only decides what to print from the resulting state.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use aether_client::Client;
use aether_core::agent::AgentStatusKind;
use aether_core::id::{AgentId, ApprovalId};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::approval_flow::ApprovalAnswer;
use crate::app::board::BoardFocus;
use crate::app::transcript::{Cell, CellId, CellModel};
use crate::app::{App, AppEvent, Effect, IDLE_PERIOD_MS, TICKS_PER_SECOND, TuiOptions};
use crate::commands::{InputAction, SlashCommand};
use crate::format;
use crate::runner::{LoopEvent, execute_effects};
use crate::ui::cells::{ChatCell as _, plain_theme};
use crate::ui::views::ViewKind;

/// Width cells are laid out to. Fixed, because a pipe has no width and a
/// log read later has no terminal.
const PLAIN_WIDTH: u16 = 100;
const BOARD_HINT: &str =
    "the agent board needs the interactive TUI; use /agent <name> in plain mode";

/// After stdin closes, how long the reply already in flight may take before
/// plain mode stops waiting for it.
const DRAIN_MAX: Duration = Duration::from_secs(120);

/// How long nothing may happen after stdin closes before plain mode leaves.
///
/// A wall-clock quiet period, not a turn boundary: [`busy`] cannot see the
/// window where the daemon has confirmed the message but has not yet moved
/// the agent to `Running`, so a daemon that takes longer than this to start
/// the turn can be left behind. Widening it costs every script a second of
/// waiting; closing it properly needs the daemon to say which turn a message
/// started, which the protocol does not carry yet.
const DRAIN_QUIET: Duration = Duration::from_secs(1);

/// Run the plain client until stdin closes or the person types `/quit`.
pub async fn run(client: Client, options: TuiOptions) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // Tokio's stdin uses a blocking pool read that runtime shutdown waits for.
    // A detached OS thread lets /quit leave even while stdin remains open.
    std::thread::Builder::new()
        .name("plain-stdin".into())
        .spawn(move || {
            use std::io::BufRead as _;
            for line in io::stdin().lock().lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
    run_loop(client, options, rx).await
}

/// The loop, over any source of typed lines. `run` passes stdin;
/// `tests/plain_run.rs` passes a pipe, which is the only way the drain and
/// the reconnect can be driven end to end without a terminal.
pub async fn run_with_input<R>(client: Client, options: TuiOptions, input: R) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = spawn_input(input, tx);
    let result = run_loop(client, options, rx).await;
    reader.abort();
    result
}

async fn run_loop(
    client: Client,
    options: TuiOptions,
    mut input: tokio::sync::mpsc::UnboundedReceiver<io::Result<String>>,
) -> anyhow::Result<()> {
    let mut client = client;
    let mut app = plain_app(options);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LoopEvent>();

    let mut out = Printer::default();
    out.line("aether - plain mode - type a message, /help for commands, Ctrl+D or /quit to leave")?;

    let mut pending = app.reconnected(&client);
    let mut input_open = true;
    let mut reconnecting = false;
    let mut eof_at = Instant::now();
    let mut last_activity = Instant::now();
    // The TUI's clock, at its idle beat: the reducer refreshes the fleet
    // status and expires notices on a tick, and plain mode shows both.
    let mut clock = tokio::time::interval(Duration::from_millis(IDLE_PERIOD_MS));
    clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        pending = attach_images(&mut app, pending).await;
        if pending.iter().any(|e| matches!(e, Effect::Quit)) {
            out.finish_stream_line()?;
            return Ok(());
        }
        execute_effects(
            &client,
            &tx,
            std::mem::take(&mut pending),
            &mut std::io::stdout(),
        );
        if out.drain(&app)? || busy(&app) {
            last_activity = Instant::now();
        }
        // Input is over: leave once the turn it started has settled, and
        // leave regardless rather than hang on an agent that never does.
        if !input_open && (last_activity.elapsed() >= DRAIN_QUIET || eof_at.elapsed() >= DRAIN_MAX)
        {
            out.finish_stream_line()?;
            return Ok(());
        }

        pending = tokio::select! {
            line = input.recv(), if input_open => match line {
                Some(line) => submit_line(&mut app, &mut out, &line?)?,
                None => {
                    input_open = false;
                    eof_at = Instant::now();
                    Vec::new()
                }
            },
            event = rx.recv() => match event {
                Some(LoopEvent::Mouse(_)) => continue,
                None => return Ok(()),
                Some(LoopEvent::AppFrom { instance, event }) => {
                    if !client.welcome().is_some_and(|welcome| welcome.instance_id == instance) { continue; }
                    let lost = matches!(event, AppEvent::Disconnected { .. } | AppEvent::HistoryExpired);
                    let effects = app.reduce(event);
                    if lost && !reconnecting { reconnecting = true; crate::runner::spawn_reconnect(&app, tx.clone()); }
                    effects
                }
                Some(LoopEvent::App(event)) => {
                    let lost = matches!(event, AppEvent::Disconnected { .. } | AppEvent::HistoryExpired);
                    let effects = match event {
                        AppEvent::Paste(line) => submit_line(&mut app, &mut out, &line)?,
                        event => app.reduce(event),
                    };
                    out.close_views(&mut app)?;
                    // The status line says "reconnecting", so something has
                    // to be reconnecting. Same backoff as the TUI's.
                    if lost && !reconnecting {
                        reconnecting = true;
                        crate::runner::spawn_reconnect(&app, tx.clone());
                    }
                    effects
                }
                #[cfg(unix)]
                Some(LoopEvent::Suspend) => Vec::new(),
                // Nothing in plain mode hands the terminal over to a child;
                // that one is a TUI-only path.
                Some(LoopEvent::External(args)) => {
                    out.line(&format!(
                        "run `aether {}` in another terminal, then come back",
                        args.join(" ")
                    ))?;
                    Vec::new()
                }
                Some(LoopEvent::VoiceControl(request)) => {
                    match request {
                        Ok(request) => {
                            if let Err(error) = request.ack(false, "voice replies require the interactive TUI") {
                                tracing::warn!(%error, "could not reject voice control in plain mode");
                            }
                        }
                        Err(error) => tracing::warn!(%error, "voice control unavailable in plain mode"),
                    }
                    Vec::new()
                }
                Some(LoopEvent::Reconnected(new_client)) => {
                    let effects = app.reconnected(&new_client);
                    client = new_client;
                    reconnecting = false;
                    effects
                }
            },
            _ = clock.tick() => app.reduce(AppEvent::Tick {
                frames: (IDLE_PERIOD_MS * TICKS_PER_SECOND / 1_000).max(1),
            }),
        };
    }
}

/// Read each image a line asked for before the next line is taken: a pipe
/// delivers the message after `/image` sooner than a file is read.
async fn attach_images(app: &mut App, effects: Vec<Effect>) -> Vec<Effect> {
    let mut rest = Vec::with_capacity(effects.len());
    for effect in effects {
        let Effect::AttachImage(source) = effect else {
            rest.push(effect);
            continue;
        };
        let image = crate::image_input::read(source).await;
        rest.extend(app.reduce(AppEvent::ImageRead(image)));
    }
    rest
}

/// Whether the session still owes the person something: an agent yet to
/// exist, a turn running, or a message the daemon has not confirmed.
fn plain_app(options: TuiOptions) -> App {
    // Interactive decoration must not change the plain renderer's established output.
    App::new(TuiOptions {
        ascii: false,
        ..options
    })
}

fn busy(app: &App) -> bool {
    if app.usage_loading {
        return true;
    }
    let Some(agent) = app.selected else {
        return true;
    };
    if app
        .selected_agent()
        .is_some_and(|a| a.status.kind() == AgentStatusKind::Running)
    {
        return true;
    }
    app.transcripts
        .get(&agent)
        .is_some_and(|t| !t.steering.is_empty() || !t.queue.is_empty())
}

/// One typed line: a message or command, or the answer to the approval on
/// screen. While one waits, other text is refused rather than lost silently.
fn submit_line<W: Write>(
    app: &mut App,
    out: &mut Printer<W>,
    line: &str,
) -> io::Result<Vec<Effect>> {
    out.close_views(app)?;
    if let InputAction::Command(command) = app.parse_input(line) {
        out.cancel = None;
        match command {
            SlashCommand::Agent { name: None } => {
                out.line(BOARD_HINT)?;
                return Ok(Vec::new());
            }
            SlashCommand::ViewMode(None) => {
                for line in crate::ui::views::modes::body(app, &plain_theme()) {
                    out.line(&line.to_string())?;
                }
                return Ok(Vec::new());
            }
            SlashCommand::Cancel => {
                if let Some(agent) = app.selected {
                    out.line(&format!(
                        "End {} ({agent})? Type yes to end it, or no to keep it running.",
                        app.agent_name(agent)
                    ))?;
                    out.cancel = Some(agent);
                } else {
                    out.line("no agent selected to cancel")?;
                }
                return Ok(Vec::new());
            }
            command => {
                let effects = app.run_slash(command);
                out.close_views(app)?;
                return Ok(effects);
            }
        }
    }
    if let Some(agent) = out.cancel.take() {
        match line.trim() {
            "yes" => {
                return Ok(vec![Effect::Request {
                    command: aether_protocol::Command::Cancel {
                        agent,
                        reason: Some("stopped from plain mode".into()),
                        idempotency_key: None,
                    },
                    then: crate::app::Then::Ignore,
                }]);
            }
            "no" => {
                out.line("cancellation declined")?;
                return Ok(Vec::new());
            }
            _ => {
                out.line("cancellation declined; type /cancel again to request confirmation")?;
            }
        }
    }
    if let Some(id) = out.approval {
        if !app.approvals.iter().any(|a| a.id == id) || app.resolving.contains(&id) {
            out.line("not sent: the printed approval is no longer pending; wait for the next prompt and enter a new answer")?;
            out.approval = None;
            return Ok(Vec::new());
        }
        // A typed line is deliberate, so it answers directly: the TUI's
        // minimum-visible wait guards against stray keys, not lines.
        let answer = match line.trim() {
            "y" => Some(ApprovalAnswer::Once),
            "a" => Some(ApprovalAnswer::Subtree),
            "p" => Some(ApprovalAnswer::Persist),
            "n" => Some(ApprovalAnswer::Deny),
            _ => None,
        };
        return match answer {
            Some(answer) => {
                out.approval = None;
                Ok(app.answer_approval(id, answer))
            }
            None => {
                out.line("not sent: an approval is waiting; answer y (once), a (saved for the agent and its subagents), p (always, everywhere) or n (deny)")?;
                Ok(Vec::new())
            }
        };
    }
    // An approval can arrive before the next drain. Enter must never reach
    // the TUI's approval dispatch for a request plain mode has not printed.
    if app.approvals.iter().any(|a| !app.resolving.contains(&a.id)) {
        out.line("not sent: an approval is waiting; wait for its prompt before answering")?;
        return Ok(Vec::new());
    }
    let mut effects = app.reduce(AppEvent::Paste(line.to_owned()));
    effects.extend(app.reduce(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    ))));
    out.close_views(app)?;
    Ok(effects)
}

/// Deliver lines in order, and surface read errors instead of treating them as EOF.
fn spawn_input<R>(
    input: R,
    tx: tokio::sync::mpsc::UnboundedSender<io::Result<String>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt as _;
        let mut lines = tokio::io::BufReader::new(input).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    })
}

/// What has already been said, so each turn of the loop prints only what is
/// new. The TUI redraws the whole screen; plain mode cannot take anything back.
struct Printer<W = io::Stdout> {
    output: W,
    presentation: Option<(u32, bool)>,
    diagnostics: Vec<crate::view_modes::Diagnostic>,
    epochs: std::collections::HashMap<AgentId, crate::app::transcript::TranscriptEpoch>,
    revisions: std::collections::HashMap<(AgentId, CellId), crate::app::transcript::CellRevision>,
    /// What was last said about each cell, so a cell that changes — a call
    /// going from running to done — is reported again instead of being lost
    /// the way an in-place redraw would hide it.
    printed: std::collections::HashMap<(AgentId, CellId), Vec<String>>,
    /// The agent already named, so the session says which agent it is
    /// addressing once per adoption. A log read later, or a step that failed
    /// in CI, has no other way to know.
    named: Option<AgentId>,
    last_notice: Option<String>,
    disk_held: bool,
    usage_pending: bool,
    last_status: Option<String>,
    /// Keep the prompt until a line answers or acknowledges it, even if the
    /// daemon resolves it meanwhile. A partially typed answer belongs to it.
    approval: Option<ApprovalId>,
    cancel: Option<AgentId>,
    streams: std::collections::HashMap<AgentId, PrintedStream>,
    stream_line_open: bool,
}

struct PrintedStream {
    generation: u64,
    invocation: Option<aether_core::id::InvocationId>,
    text: String,
    after: CellId,
}

impl Default for Printer {
    fn default() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write> Printer<W> {
    fn new(output: W) -> Self {
        Self {
            output,
            presentation: None,
            diagnostics: Vec::new(),
            epochs: Default::default(),
            revisions: Default::default(),
            printed: Default::default(),
            named: None,
            last_notice: None,
            disk_held: false,
            usage_pending: false,
            last_status: None,
            approval: None,
            cancel: None,
            streams: Default::default(),
            stream_line_open: false,
        }
    }

    fn line(&mut self, text: &str) -> io::Result<()> {
        self.finish_stream_line()?;
        writeln!(self.output, "{}", sanitise(text))?;
        self.output.flush()
    }

    fn finish_stream_line(&mut self) -> io::Result<()> {
        if self.stream_line_open {
            writeln!(self.output)?;
            self.stream_line_open = false;
            self.output.flush()?;
        }
        Ok(())
    }

    fn assistant_chunk(&mut self, text: &str, first: bool) -> io::Result<()> {
        for (index, line) in text.split_inclusive('\n').enumerate() {
            if !self.stream_line_open {
                let prefix = if first && index == 0 { "* " } else { "  " };
                write!(self.output, "{prefix}")?;
            }
            write!(self.output, "{}", sanitise(line))?;
            if line.ends_with('\n') {
                writeln!(self.output)?;
            }
            self.stream_line_open = !line.ends_with('\n');
        }
        self.output.flush()
    }

    /// Views are interactive: `/usage` is printed once its data lands, and
    /// any other view is closed with a word on where it works.
    fn close_views(&mut self, app: &mut App) -> io::Result<()> {
        for view in std::mem::take(&mut app.bottom.view_stack) {
            match view.kind() {
                Some(ViewKind::Usage) => self.usage_pending = true,
                _ => {
                    self.line("that view needs the interactive TUI; run aether without --plain")?
                }
            }
        }
        if app.board.open {
            self.line(BOARD_HINT)?;
        }
        app.board.open = false;
        app.board.focus = BoardFocus::BottomPane;
        app.bottom.popup = None;
        app.bottom.find = None;
        if app.bottom.shortcuts {
            for (command, description) in crate::commands::help_commands() {
                self.line(&format!("{command} - {description}"))?;
            }
            self.line("View modes")?;
            for mode in app.view_modes.ordered() {
                self.line(&format!(
                    "/{} - {}",
                    mode.name,
                    mode.definition
                        .description
                        .as_deref()
                        .unwrap_or("view mode")
                ))?;
            }
        }
        app.bottom.shortcuts = false;
        Ok(())
    }

    /// Say whatever changed since the last call; `true` if anything was said.
    fn drain(&mut self, app: &App) -> io::Result<bool> {
        let mut said_anything = false;
        let presentation = (app.presentation_revision, app.details);
        if self.presentation.is_some_and(|old| old != presentation) {
            self.line(&format!(
                "- view mode {} (subsequent updates)",
                app.view_mode
            ))?;
            for (agent, transcript) in &app.transcripts {
                for cell in transcript
                    .all_cells()
                    .chain(transcript.pending_messages.values())
                {
                    self.revisions.insert((*agent, cell.id), cell.revision);
                }
                if !transcript.stream.text.is_empty() {
                    self.streams.insert(
                        *agent,
                        PrintedStream {
                            generation: transcript.stream.generation,
                            invocation: transcript.stream.invocation,
                            text: transcript.stream.text.trim_start().into(),
                            after: transcript
                                .all_cells()
                                .map(|c| c.id)
                                .max()
                                .unwrap_or_default(),
                        },
                    );
                }
            }
        }
        self.presentation = Some(presentation);
        if self.diagnostics != app.view_modes.diagnostics {
            for problem in &app.view_modes.diagnostics {
                self.line(&format!("- view mode notice: {problem}"))?;
            }
            self.diagnostics.clone_from(&app.view_modes.diagnostics);
        }
        if let Some(status) = &app.status {
            let held = status.low_disk.is_some();
            if held != self.disk_held {
                match &status.low_disk {
                    Some(disk) => self.line(&format!("- {}", format::low_disk(disk)))?,
                    None => self.line("- Disk recovered: new turns released")?,
                }
                self.disk_held = held;
                said_anything = true;
            }
        }
        if self.usage_pending && !app.usage_loading {
            self.line("account usage")?;
            for line in crate::ui::views::usage::body(app, &plain_theme()) {
                self.line(&line.to_string())?;
            }
            self.usage_pending = false;
            said_anything = true;
        }
        if self.approval.is_none()
            && let Some(approval) = app
                .approvals
                .iter()
                .filter(|a| !app.resolving.contains(&a.id))
                .min_by_key(|a| a.requested_at)
        {
            self.line(&format!(
                "approval needed: {} {} - {} - {} - {} - {:?} - type y (once), a (saved for the agent and its subagents), p (always, everywhere) or n (deny)",
                app.agent_name(approval.agent_id), approval.agent_id, approval.id,
                approval.tool, approval.description, approval.side_effect
            ))?;
            // Only a successfully written and flushed prompt may be answered.
            self.approval = Some(approval.id);
            said_anything = true;
        }
        let Some(agent) = app.selected else {
            return Ok(said_anything);
        };
        if let Some(transcript) = app.transcripts.get(&agent) {
            if self
                .epochs
                .insert(agent, transcript.epoch)
                .is_some_and(|epoch| epoch != transcript.epoch)
            {
                self.printed.retain(|(owner, _), _| *owner != agent);
                self.revisions.retain(|(owner, _), _| *owner != agent);
                self.streams.remove(&agent);
            }
            let mut said: Vec<(CellId, Vec<String>)> = Vec::new();
            let policy = app.policy(Some(agent));
            let named = policy.definition.is_some() || app.details && app.view_mode != "standard";
            let mut after_failed = false;
            for cell in transcript.presentation_cells(named) {
                let visible = policy.shows(&cell.model);
                // Only the first of a run of failed tool lines is said.
                let failed = visible && policy.failed_tool_line(&cell.model);
                let folded = failed && after_failed;
                if visible {
                    after_failed = failed;
                }
                if self.revisions.get(&(agent, cell.id)) == Some(&cell.revision) {
                    continue;
                }
                self.revisions.insert((agent, cell.id), cell.revision);
                let lines = if !visible || folded {
                    Vec::new()
                } else if policy.compact(&cell.model) {
                    crate::ui::cells::compact::line(&cell.model, PLAIN_WIDTH, &plain_theme())
                        .map(|line| vec![sanitise(&line.to_string())])
                        .unwrap_or_else(|| describe(cell))
                } else if policy.expands(&cell.model)
                    && !matches!(cell.model, CellModel::Question { .. })
                {
                    cell.model
                        .detail_lines(PLAIN_WIDTH, &plain_theme())
                        .into_iter()
                        .map(|line| sanitise(&line.to_string()))
                        .collect()
                } else {
                    describe(cell)
                };
                if !self.printed.contains_key(&(agent, cell.id))
                    && let CellModel::Assistant { text } = &cell.model
                    && let Some(stream) = self.streams.get(&agent)
                    && cell.id > stream.after
                    && let Some(tail) = text
                        .strip_prefix(&stream.text)
                        .or_else(|| (text == stream.text.trim_end()).then_some(""))
                {
                    if visible && !tail.is_empty() {
                        self.assistant_chunk(tail, false)?;
                    }
                    self.streams.remove(&agent);
                    self.finish_stream_line()?;
                    self.printed.insert((agent, cell.id), lines);
                    continue;
                }
                if self.printed.get(&(agent, cell.id)) != Some(&lines) {
                    said.push((cell.id, lines));
                }
            }
            for (id, lines) in said {
                said_anything |= !lines.is_empty();
                for line in &lines {
                    self.line(line)?;
                }
                self.printed.insert((agent, id), lines);
            }
            let text = transcript.stream.text.trim_start();
            if !text.is_empty() {
                let after = transcript
                    .all_cells()
                    .map(|cell| cell.id)
                    .max()
                    .unwrap_or_default();
                if self.streams.get(&agent).is_some_and(|printed| {
                    printed.generation != transcript.stream.generation
                        || printed.invocation != transcript.stream.invocation
                }) {
                    self.streams.remove(&agent);
                }
                let stream = self.streams.entry(agent).or_insert_with(|| PrintedStream {
                    generation: transcript.stream.generation,
                    invocation: transcript.stream.invocation,
                    text: String::new(),
                    after,
                });
                let first = stream.text.is_empty();
                let tail = text.strip_prefix(&stream.text).unwrap_or(text).to_owned();
                if !tail.is_empty() {
                    if policy.shows_category(crate::view_modes::Category::AssistantText) {
                        self.assistant_chunk(&tail, first)?;
                    }
                    self.streams.insert(
                        agent,
                        PrintedStream {
                            generation: transcript.stream.generation,
                            invocation: transcript.stream.invocation,
                            text: text.to_owned(),
                            after,
                        },
                    );
                    said_anything = true;
                }
            }
            // A cell the transcript dropped is not coming back.
            self.printed
                .retain(|(owner, id), _| *owner != agent || transcript.cell(*id).is_some());
            self.revisions
                .retain(|(owner, id), _| *owner != agent || transcript.cell(*id).is_some());
        }

        // Until the tree arrives the agent has no name, only an id; saying
        // "agt_01M…: created" helps nobody.
        if app.selected_agent().is_none() {
            return Ok(said_anything);
        }
        if self.named != Some(agent) {
            self.named = Some(agent);
            said_anything = true;
            self.line(&format!(
                "- session agent: {} {agent}",
                app.agent_name(agent)
            ))?;
        }
        // The status line's own information, once per change.
        let status = summary(app, agent);
        if self.last_status.as_deref() != Some(status.as_str()) {
            said_anything = true;
            self.line(&status)?;
            self.last_status = Some(status);
        }
        let notice = app.live_flash().map(str::to_owned);
        if let Some(text) = &notice
            && notice != self.last_notice
        {
            said_anything = true;
            self.line(&format!("- {text}"))?;
        }
        self.last_notice = notice;
        Ok(said_anything)
    }
}

/// Model and tool bytes reach the terminal through here, so nothing they
/// contain can move the cursor, set a colour or ring a bell, and the glyphs
/// the screen draws come out as punctuation instead.
fn sanitise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(' '),
            c if c.is_control() || matches!(c, '\u{80}'..='\u{9f}') => {}
            c => match undraw(c) {
                Some(ascii) => out.push_str(ascii),
                None => out.push(c),
            },
        }
    }
    out
}

/// The glyphs the UI itself draws, as ASCII: the theme's glyph table
/// (`ASCII_GLYPHS`) plus markdown's own. The model's characters are its words
/// and pass through, accents and all.
fn undraw(c: char) -> Option<&'static str> {
    Some(match c {
        '›' => ">",
        '•' | '●' => "*",
        '└' => "\\",
        '│' | '‖' | '▍' | '▎' => "|",
        '→' => "->",
        '≥' => ">=",
        '≈' => "~",
        '├' => "|-",
        '━' => "=",
        '─' => "-",
        '◌' => "~",
        '○' => "o",
        '◆' => "!",
        '✓' => "+",
        '✗' => "x",
        '⊘' | '·' => "-",
        '↪' => ">",
        '…' => "...",
        '☑' => "[x]",
        '☐' => "[ ]",
        _ => return None,
    })
}

/// One cell as lines: the screen's own grammar, without styles, glyphs or
/// anything the model's text could do to a terminal.
pub fn describe(cell: &Cell) -> Vec<String> {
    let mut out: Vec<String> = cell
        .model
        .plain_lines(PLAIN_WIDTH)
        .iter()
        .map(|line| sanitise(line.trim_end()))
        .collect();
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    // The card's colour says whether it still waits; plain mode says it in words.
    if let CellModel::Question { state, .. } = cell.model {
        out.push(
            match state {
                crate::app::transcript::QuestionState::Answered => "answered",
                crate::app::transcript::QuestionState::Pending => "reply to this agent to answer",
                crate::app::transcript::QuestionState::AskerEnded => {
                    "unanswered; this agent has ended"
                }
            }
            .to_owned(),
        );
    }
    out
}

/// The status line, as a sentence.
pub fn summary(app: &App, agent: AgentId) -> String {
    let name = app.agent_name(agent);
    let kind = app
        .selected_agent()
        .map(|a| a.status.kind())
        .unwrap_or(AgentStatusKind::Created);
    let mut out = format!("-- {name}: {}", status_word(kind));
    if let Some(status) = &app.status {
        out.push_str(&format!(
            " - fleet {} live, {} running, {} waiting, {} blocked",
            status.agents_live, status.agents_running, status.agents_waiting, status.agents_blocked,
        ));
        if status.agents_suspended > 0 {
            out.push_str(&format!(", {} suspended", status.agents_suspended));
        }
        out.push_str(&format!(" - {}", format::cost(&status.usage_total)));
    }
    if !app.connected {
        out.push_str(" - not connected, reconnecting");
    }
    sanitise(&out)
}

/// Plain mode has no glyphs, so the state becomes its name. Off the status
/// itself, not off the glyph, so a new glyph cannot silently read wrong.
fn status_word(kind: AgentStatusKind) -> &'static str {
    match kind {
        AgentStatusKind::Created => "created",
        AgentStatusKind::Ready => "ready",
        AgentStatusKind::Running => "running",
        AgentStatusKind::Waiting => "waiting",
        AgentStatusKind::Blocked => "blocked",
        AgentStatusKind::Completed => "completed",
        AgentStatusKind::Failed => "failed",
        AgentStatusKind::Cancelled => "cancelled",
        AgentStatusKind::Imported => "imported",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use aether_core::approval::RuleScope;
    use aether_core::id::ToolCallId;
    use aether_core::tool::{ApprovalDecision, ApprovalRequest, ApprovalStatus, SideEffectClass};
    use aether_protocol::Command;

    use crate::testing::{app_with_transcript, populated_app};

    fn request(app: &App, agent: AgentId, description: &str, seconds_ago: i64) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalId::new(),
            agent_id: agent,
            task_id: None,
            call_id: ToolCallId::new(),
            tool: "bash".into(),
            description: description.into(),
            argument: Some(description.into()),
            resources: Vec::new(),
            side_effect: SideEffectClass::Mutation,
            requested_at: app.now - chrono::Duration::seconds(seconds_ago),
            status: ApprovalStatus::Pending,
        }
    }

    fn resolved_id(effects: &[Effect]) -> Option<ApprovalId> {
        effects.iter().find_map(|effect| match effect {
            Effect::Request {
                command: Command::ResolveApproval { id, .. },
                ..
            } => Some(*id),
            _ => None,
        })
    }

    fn output(out: &Printer<Vec<u8>>) -> &str {
        std::str::from_utf8(&out.output).expect("plain output is UTF-8")
    }

    #[test]
    fn mode_switch_does_not_replay_suppressed_cells_or_stream_prefixes() {
        use aether_client::SubscriptionFrame;
        use aether_core::event::{Actor, Event, EventDraft, EventKind};
        use aether_core::id::InvocationId;
        use aether_core::model::{ContentBlock, MessageRole};
        let mut app = app_with_transcript(&[]);
        let agent = app.selected.unwrap();
        let definition = crate::view_modes::Definition::parse("version=1\nshow=[]").unwrap();
        app.view_modes.modes.insert(
            "quiet".into(),
            crate::view_modes::Mode {
                name: "quiet".into(),
                source: "fixture".into(),
                definition,
            },
        );
        app.view_mode = "quiet".into();
        let mut out = Printer::new(Vec::new());
        out.drain(&app).unwrap();
        out.output.clear();
        let invocation = InvocationId::new();
        app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
            agent,
            invocation,
            delta: aether_protocol::Delta::Text {
                text: "suppressed prefix".into(),
            },
        }));
        out.drain(&app).unwrap();
        assert!(!output(&out).contains("suppressed prefix"));
        submit_line(&mut app, &mut out, "/mode standard").unwrap();
        out.drain(&app).unwrap();
        app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
            agent,
            invocation,
            delta: aether_protocol::Delta::Text {
                text: " visible suffix".into(),
            },
        }));
        out.drain(&app).unwrap();
        assert!(output(&out).contains("visible suffix"));
        assert!(!output(&out).contains("suppressed prefix"));
        let event = Event::from_draft(
            EventDraft::new(
                Actor::System,
                EventKind::AgentMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::text("suppressed prefix visible suffix")],
                    invocation: Some(invocation),
                },
            )
            .agent(agent),
            aether_core::id::EventId::new(),
            1,
            app.now,
        );
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
        out.drain(&app).unwrap();
        assert_eq!(output(&out).matches("visible suffix").count(), 1);
        assert!(!output(&out).contains("suppressed prefix"));
        submit_line(&mut app, &mut out, "/focus").unwrap();
        out.drain(&app).unwrap();
        assert_eq!(output(&out).matches("visible suffix").count(), 1);
    }

    #[test]
    fn disk_holds_and_releases_print_once_even_without_a_selected_agent() {
        let mut app = populated_app();
        app.selected = None;
        app.approvals.clear();
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("initial status");
        assert!(output(&out).is_empty());
        app.status.as_mut().expect("status").low_disk = Some(aether_core::disk::LowDisk {
            path: "/isolated/store".into(),
            available_bytes: 1 << 30,
            floor_bytes: 5 << 30,
            since: app.now,
        });
        assert!(out.drain(&app).expect("hold"));
        assert!(!out.drain(&app).expect("same hold"));
        app.status
            .as_mut()
            .expect("status")
            .low_disk
            .as_mut()
            .expect("disk")
            .available_bytes = 2 << 30;
        assert!(!out.drain(&app).expect("new measurement"));
        let status = app.status.take().expect("status");
        assert!(!out.drain(&app).expect("no status is not recovery"));
        app.status = Some(status);
        app.status.as_mut().expect("status").low_disk = None;
        assert!(out.drain(&app).expect("release"));
        assert!(!out.drain(&app).expect("same release"));
        assert_eq!(
            output(&out),
            "- Disk low: new turns held - 1.0 GiB free / 5.0 GiB floor\n- Disk recovered: new turns released\n"
        );
        assert!(output(&out).is_ascii());
    }

    #[test]
    fn ascii_option_does_not_change_plain_notices() {
        let options = TuiOptions {
            ascii: true,
            reduced_motion: true,
            ..Default::default()
        };
        let mut app = plain_app(options);
        assert!(!app.options.ascii);
        assert!(!app.theme.ascii());
        assert!(app.options.reduced_motion);
        let mut baseline = App::new(TuiOptions::default());
        for state in [&mut app, &mut baseline] {
            state.reduce(AppEvent::Paste("/mouse off".into()));
            state.reduce(AppEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )));
        }
        let output = sanitise(app.live_flash().expect("mouse notice"));
        assert_eq!(
            output,
            sanitise(baseline.live_flash().expect("baseline notice"))
        );
        assert!(output.contains("mouse off -"), "{output}");
    }

    /// A typed approval letter is deliberate: it answers at once, without the
    /// card's read delay, and each letter reaches the daemon as its own decision.
    #[test]
    fn plain_slash_commands_work_while_an_approval_waits_or_expires() {
        for printed in [false, true] {
            for command in ["/quit", "/stop", "/agent assistant", "/help", "/cancel"] {
                let mut app = populated_app();
                let mut out = Printer::new(Vec::new());
                if printed {
                    out.drain(&app).expect("prompt");
                }
                let effects = submit_line(&mut app, &mut out, command).expect("command");
                assert_eq!(resolved_id(&effects), None, "{command}");
                assert!(app.resolving.is_empty());
                match command {
                    "/quit" => assert!(effects.contains(&Effect::Quit)),
                    "/stop" => assert!(effects.iter().any(|e| matches!(
                        e,
                        Effect::Request {
                            command: Command::Interrupt { .. },
                            ..
                        }
                    ))),
                    "/help" => assert!(output(&out).contains("/quit")),
                    "/cancel" => assert!(out.cancel.is_some()),
                    _ => assert_eq!(app.agent_name(app.selected.expect("agent")), "assistant"),
                }
            }
        }
        let mut app = populated_app();
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("prompt");
        app.approvals.clear();
        assert!(
            submit_line(&mut app, &mut out, "/quit")
                .expect("quit")
                .contains(&Effect::Quit)
        );
    }

    #[test]
    fn plain_cancel_confirmation_is_explicit_and_bound_to_the_printed_agent() {
        for answer in ["yes", "no", "y", "/help", "/quit"] {
            let mut app = populated_app();
            let agent = app.selected.expect("selected");
            let mut out = Printer::new(Vec::new());
            out.drain(&app).expect("approval");
            assert!(
                submit_line(&mut app, &mut out, "/cancel")
                    .expect("ask")
                    .is_empty()
            );
            assert!(output(&out).contains("Type yes"));
            app.selected = Some(AgentId::new());
            let effects = submit_line(&mut app, &mut out, answer).expect("confirmation");
            let cancelled = effects.iter().find_map(|e| match e {
                Effect::Request {
                    command: Command::Cancel { agent, .. },
                    ..
                } => Some(*agent),
                _ => None,
            });
            assert_eq!(cancelled, (answer == "yes").then_some(agent));
            if answer == "y" {
                assert!(resolved_id(&effects).is_some());
            } else {
                assert_eq!(resolved_id(&effects), None);
            }
            assert!(out.cancel.is_none());
        }
    }

    #[test]
    fn plain_chunks_preserve_words_and_newlines_before_a_status_record() {
        let mut out = Printer::new(Vec::new());
        out.assistant_chunk("mo", true).expect("first delta");
        assert_eq!(output(&out), "* mo");
        out.assistant_chunk("ck model\nnext\n", false)
            .expect("multiline delta");
        out.assistant_chunk("caf", false).expect("partial word");
        out.assistant_chunk("é", false).expect("unicode suffix");
        out.line("- status").expect("status closes stream line");
        assert_eq!(output(&out), "* mock model\n  next\n  café\n- status\n");
    }

    #[test]
    fn plain_streams_deltas_before_completion_without_reprinting_the_reply() {
        use aether_client::SubscriptionFrame;
        use aether_core::event::{Actor, Event, EventDraft, EventKind};
        use aether_core::id::{EventId, InvocationId};
        use aether_core::model::{ContentBlock, MessageRole};
        for interrupted in [false, true] {
            let mut app = app_with_transcript(&[]);
            let agent = app.selected.expect("agent");
            let invocation = InvocationId::new();
            let mut out = Printer::new(Vec::new());
            out.drain(&app).expect("baseline");
            out.output.clear();
            for chunk in ["Count", "ing café: ", "1, ", "2, ", "3"] {
                app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
                    agent,
                    invocation,
                    delta: aether_protocol::Delta::Text { text: chunk.into() },
                }));
                assert!(out.drain(&app).expect("delta printed"));
                assert!(output(&out).contains(chunk), "{}", output(&out));
                let bytes = out.output.len();
                out.drain(&app).expect("unchanged drain");
                assert_eq!(out.output.len(), bytes);
            }
            let before = out.output.clone();
            assert!(output(&out).contains("Counting café: 1, 2, 3"));
            let kind = if interrupted {
                EventKind::AgentInterrupted { turn: 1 }
            } else {
                EventKind::AgentMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::text("Counting café: 1, 2, 3")],
                    invocation: Some(invocation),
                }
            };
            let event = Event::from_draft(
                EventDraft::new(Actor::System, kind).agent(agent),
                EventId::new(),
                1,
                app.now,
            );
            app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
            out.drain(&app).expect("committed");
            assert!(out.output.starts_with(&before));
            assert_eq!(output(&out).matches("Counting café:").count(), 1);
            assert_eq!(output(&out).matches("1, ").count(), 1);
            assert_eq!(output(&out).matches("2, ").count(), 1);
            if interrupted {
                assert!(output(&out).contains("Interrupted"));
            }
        }
    }

    #[test]
    fn plain_approval_letters_map_to_the_four_answers() {
        for (letter, want, scope) in [
            ('y', ApprovalDecision::Approved, None),
            ('a', ApprovalDecision::Approved, Some("subtree")),
            ('p', ApprovalDecision::Approved, Some("everywhere")),
            ('n', ApprovalDecision::Denied, None),
        ] {
            let mut app = populated_app();
            let pending = app.due_approval().expect("the fixture's approval").id;
            let mut out = Printer::new(Vec::new());
            out.drain(&app).expect("print approval");
            let effects = submit_line(&mut app, &mut out, &format!("{letter}\n"))
                .expect("submit approval answer");
            let resolved = effects.iter().find_map(|effect| match effect {
                Effect::Request {
                    command:
                        Command::ResolveApproval {
                            id,
                            decision,
                            remember,
                            ..
                        },
                    ..
                } => Some((
                    *id,
                    *decision,
                    remember.as_ref().and_then(|r| r.scope.clone()),
                )),
                _ => None,
            });
            let (id, decision, rule) = resolved.unwrap_or_else(|| panic!("{letter}: {effects:?}"));
            assert_eq!(id, pending, "{letter}");
            assert_eq!(decision, want, "{letter}");
            match (scope, rule) {
                (None, None) => {}
                (Some("subtree"), Some(RuleScope::Subtree { .. })) => {}
                (Some("everywhere"), Some(RuleScope::Everywhere)) => {}
                (scope, rule) => panic!("{letter}: wanted {scope:?}, got {rule:?}"),
            }
            assert!(app.due_approval().is_none(), "{letter}: still due");
        }
    }

    #[test]
    fn plain_answers_only_the_printed_request_with_requester_attribution() {
        let mut app = populated_app();
        let selected = app.selected.expect("selected agent");
        let child = AgentId::new();
        let mut node = app.tree.nodes[&selected].clone();
        node.agent.id = child;
        node.agent.parent = Some(selected);
        node.agent.name = "child".into();
        app.tree.nodes.insert(child, node);
        app.tree
            .nodes
            .get_mut(&selected)
            .expect("root")
            .children
            .push(child);
        let older = request(&app, child, "delete important files", 2);
        let newer = request(&app, selected, "run tests", 1);
        app.approvals = vec![newer.clone(), older.clone()];
        let mut out = Printer::new(Vec::new());

        out.drain(&app).expect("print approval");
        let text = output(&out);
        assert!(text.contains("saved for the agent and its subagents"));
        assert!(text.contains("always, everywhere"));
        assert!(!text.contains("this session"));
        assert!(
            text.contains(&format!(
                "approval needed: {} {} - {} - bash - delete important files",
                app.agent_name(child),
                child,
                older.id
            )),
            "{text}"
        );
        assert!(!text.contains("run tests"), "{text}");
        assert_eq!(text.matches("approval needed:").count(), 1);
        let effects = submit_line(&mut app, &mut out, "y").expect("answer");
        assert_eq!(resolved_id(&effects), Some(older.id));

        out.drain(&app).expect("print next approval");
        assert!(output(&out).contains("run tests"));
        let effects = submit_line(&mut app, &mut out, "n").expect("deny");
        assert_eq!(resolved_id(&effects), Some(newer.id));
    }

    #[test]
    fn plain_does_not_answer_an_approval_before_printing_it() {
        let mut app = populated_app();
        let mut out = Printer::new(Vec::new());
        let effects = submit_line(&mut app, &mut out, "y").expect("refuse unseen answer");
        assert!(effects.is_empty());
        assert!(app.resolving.is_empty());
        assert!(app.input.text.is_empty());
        assert!(output(&out).contains("wait for its prompt"));
    }

    #[test]
    fn plain_keeps_the_printed_id_when_fifo_changes_while_typing() {
        let mut app = populated_app();
        let shown = app.approvals.first().expect("approval").clone();
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("print approval");

        let older = request(&app, AgentId::new(), "new FIFO head", 3600);
        app.approvals.push(older.clone());
        assert_eq!(app.due_approval().expect("new head").id, older.id);
        out.drain(&app).expect("drain while answer is being typed");
        assert!(!output(&out).contains("new FIFO head"));
        let effects = submit_line(&mut app, &mut out, "y").expect("answer printed request");
        assert_eq!(resolved_id(&effects), Some(shown.id));
        assert!(!app.resolving.contains(&older.id));
    }

    #[tokio::test]
    async fn plain_partial_input_stays_bound_to_the_prompt_across_approval_refreshes() {
        use crate::app::Then;
        use aether_protocol::Reply;
        use tokio::io::AsyncWriteExt as _;

        for expires in [false, true] {
            let mut app = populated_app();
            let shown = app.approvals.first().expect("approval").clone();
            let mut out = Printer::new(Vec::new());
            out.drain(&app).expect("print approval before typing");
            let (mut writer, reader) = tokio::io::duplex(64);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let task = spawn_input(reader, tx);

            writer.write_all(b"y").await.expect("start an answer");
            assert!(rx.try_recv().is_err(), "a partial line must wait for Enter");
            let older = request(&app, AgentId::new(), "refreshed FIFO head", 3600);
            let mut refreshed = vec![older.clone()];
            if !expires {
                refreshed.push(shown.clone());
            }
            app.reduce(AppEvent::Reply {
                then: Then::Approvals {
                    revision: app.approvals_revision,
                },
                result: Ok(Reply::Approvals(refreshed)),
            });
            out.drain(&app).expect("drain refresh during partial input");
            assert!(!output(&out).contains("refreshed FIFO head"));

            writer.write_all(b"\n").await.expect("press Enter");
            let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("input reader should finish the line")
                .expect("input reader event");
            let line = event.expect("completed input line");
            assert_eq!(line, "y");
            let effects = submit_line(&mut app, &mut out, &line).expect("submit completed line");
            assert_eq!(resolved_id(&effects), (!expires).then_some(shown.id));
            assert!(!app.resolving.contains(&older.id));
            drop(writer);
            task.await.expect("input reader task");
        }
    }

    #[test]
    fn plain_rejects_a_stale_answer_before_printing_the_replacement() {
        let mut app = populated_app();
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("print approval before typing");

        let replacement = request(&app, AgentId::new(), "replacement request", 1);
        app.approvals = vec![replacement.clone()];
        out.drain(&app).expect("drain while answer is being typed");
        assert!(!output(&out).contains("replacement request"));
        let effects = submit_line(&mut app, &mut out, "y").expect("reject stale answer");
        assert!(effects.is_empty());
        assert!(app.resolving.is_empty());
        assert!(output(&out).contains("printed approval is no longer pending"));

        out.drain(&app)
            .expect("print replacement after stale answer");
        assert!(output(&out).contains("replacement request"));
        let effects = submit_line(&mut app, &mut out, "y").expect("fresh answer");
        assert_eq!(resolved_id(&effects), Some(replacement.id));
    }

    #[test]
    fn plain_refuses_messages_while_a_printed_approval_waits() {
        let mut app = populated_app();
        let id = app.approvals.first().expect("approval").id;
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("print approval");
        let effects = submit_line(&mut app, &mut out, "a message").expect("refuse message");
        assert!(effects.is_empty());
        assert_eq!(out.approval, Some(id));
        assert!(app.input.text.is_empty());
    }

    #[test]
    fn plain_reprints_a_pending_approval_after_its_answer_fails() {
        let mut app = populated_app();
        let id = app.approvals.first().expect("approval").id;
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("print approval");
        let effects = submit_line(&mut app, &mut out, "y").expect("answer");
        assert_eq!(resolved_id(&effects), Some(id));
        out.drain(&app).expect("drain while answer is in flight");
        assert_eq!(output(&out).matches("approval needed:").count(), 1);

        app.reduce(AppEvent::Reply {
            then: crate::app::Then::ApprovalResolved(id),
            result: Err(aether_client::ClientError::Disconnected),
        });
        out.drain(&app).expect("print retry prompt");
        assert_eq!(output(&out).matches("approval needed:").count(), 2);
        let effects = submit_line(&mut app, &mut out, "n").expect("retry answer");
        assert_eq!(resolved_id(&effects), Some(id));
    }

    #[test]
    fn plain_prints_approvals_even_without_a_selected_agent() {
        let mut app = populated_app();
        let id = app.approvals.first().expect("approval").id;
        app.selected = None;
        let mut out = Printer::new(Vec::new());
        out.drain(&app).expect("print approval");
        assert!(output(&out).contains(&id.to_string()));
        let effects = submit_line(&mut app, &mut out, "y").expect("answer");
        assert_eq!(resolved_id(&effects), Some(id));
    }

    #[test]
    fn plain_does_not_pin_an_approval_when_output_cannot_be_flushed() {
        struct FlushFailure;
        impl Write for FlushFailure {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
            }
        }

        let app = populated_app();
        let mut out = Printer::new(FlushFailure);
        let error = out.drain(&app).expect_err("failed output must be visible");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(out.approval, None);
    }

    #[test]
    fn plain_board_and_help_commands_leave_following_messages_and_quit_usable() {
        for command in ["/agent", "/help", "/models", "/cancel"] {
            let mut app = app_with_transcript(&["hello", "answer"]);
            let mut out = Printer::new(Vec::new());
            submit_line(&mut app, &mut out, command).expect("plain command");
            assert!(!app.board.open, "{command}");
            assert_eq!(app.board.focus, BoardFocus::BottomPane, "{command}");
            assert!(!app.bottom_covered(), "{command}");
            assert!(app.input.text.is_empty(), "{command}");

            for text in ["message after command", "second separate message"] {
                let effects = submit_line(&mut app, &mut out, text).expect("send message");
                assert!(
                    effects.iter().any(|effect| matches!(effect,
                        Effect::Request { command: Command::SendMessage { content: sent, .. }, .. }
                            if sent == text
                    )),
                    "{command}: {effects:?}; draft {:?}",
                    app.input.text
                );
                assert!(app.input.text.is_empty(), "{command}");
            }
            let effects = submit_line(&mut app, &mut out, "/quit").expect("quit");
            assert_eq!(effects, vec![Effect::Quit], "{command}");
            assert!(app.input.text.is_empty(), "{command}");
            if command == "/agent" {
                assert!(output(&out).contains("use /agent <name>"));
            }
            if command == "/help" {
                assert!(output(&out).contains("/quit"));
            }
        }
    }

    #[test]
    fn plain_clears_invisible_find_popup_and_board_state_before_input() {
        let mut app = app_with_transcript(&["hello", "answer"]);
        app.board.open = true;
        app.board.focus = BoardFocus::Board;
        app.bottom.find = Some(Default::default());
        app.open_popup(crate::ui::bottom::popup::PopupKind::History);
        let mut out = Printer::new(Vec::new());
        let effects = submit_line(&mut app, &mut out, "fresh message").expect("send message");
        assert!(
            effects.iter().any(|effect| matches!(effect,
                Effect::Request { command: Command::SendMessage { content: text, .. }, .. }
                    if text == "fresh message"
            )),
            "{effects:?}"
        );
        assert!(!app.bottom_covered());
        assert!(!app.board.open);
        assert!(app.input.text.is_empty());
    }

    #[test]
    fn plain_rejects_the_board_without_saving_a_board_preference() {
        let mut app = app_with_transcript(&["hello", "answer"]);
        let directory = tempfile::tempdir().expect("isolated preferences");
        app.options.prefs_dir = Some(directory.path().to_owned());
        let mut out = Printer::new(Vec::new());
        let effects = submit_line(&mut app, &mut out, "/agent").expect("reject board");
        assert!(effects.is_empty());
        assert!(!directory.path().join("ui.toml").exists());
        assert!(app.input.text.is_empty());
    }
    #[test]
    fn window_eviction_does_not_reprint_retained_cells() {
        use aether_client::SubscriptionFrame;
        use aether_core::event::{Actor, Event, EventDraft, EventKind};
        use aether_core::id::EventId;
        use aether_core::model::{ContentBlock, MessageRole};

        let mut app = app_with_transcript(&[]);
        let agent = app.selected.unwrap();
        let mut out = Printer::new(Vec::new());
        for seq in 1..=5_001 {
            let event = Event::from_draft(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentMessage {
                        role: MessageRole::Assistant,
                        content: vec![ContentBlock::text(format!("reply {seq}"))],
                        invocation: None,
                    },
                )
                .agent(agent),
                EventId::new(),
                seq,
                app.now,
            );
            app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
            if seq >= 5_000 {
                out.drain(&app).unwrap();
            }
        }
        let mut counts = std::collections::HashMap::new();
        for line in output(&out).lines() {
            *counts.entry(line).or_insert(0) += 1;
        }
        for seq in 1..=5_001 {
            assert_eq!(
                counts.get(format!("* reply {seq}").as_str()),
                Some(&1),
                "reply {seq} must print once"
            );
        }
        assert!(!out.drain(&app).unwrap());
    }

    #[test]
    fn a_run_of_failed_calls_is_one_plain_line_in_a_mode_without_tools() {
        use crate::app::transcript::CallStatus;
        let mut app = app_with_transcript(&[]);
        let agent = app.selected.unwrap();
        let definition = crate::view_modes::Definition::parse("version=1\nshow=[]").unwrap();
        app.view_modes.modes.insert(
            "quiet".into(),
            crate::view_modes::Mode {
                name: "quiet".into(),
                source: "fixture".into(),
                definition,
            },
        );
        app.view_mode = "quiet".into();
        let mut out = Printer::new(Vec::new());
        for n in 0..3 {
            let failed = CellModel::Exec {
                call: None,
                command: format!("probe {n}"),
                status: CallStatus::Failed,
                output: format!("refused {n}\nlater line"),
                exit: Some(1),
                duration_ms: None,
                is_search: false,
                artifacts: vec![],
            };
            let transcript = app.transcripts.get_mut(&agent).unwrap();
            transcript.push_for_test(app.now, failed);
            out.drain(&app).unwrap();
        }
        let failures: Vec<&str> = output(&out)
            .lines()
            .filter(|line| line.contains("failed"))
            .collect();
        assert_eq!(failures, ["* Command failed: refused 0"]);
        assert!(!output(&out).contains("probe"), "{}", output(&out));
        assert!(!output(&out).contains("later line"), "{}", output(&out));
    }
}
