//! The async event loop: terminal setup, input, subscription frames, replies
//! and the frame tick, with effects executed on the reducer's behalf.

use crate::image_input::ImageSource;
use crate::keyboard::KeyboardMode;
use std::io::{Stdout, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aether_client::{Client, ClientError, SubscriptionFrame};
use aether_protocol::Command;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyEventKind,
};
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    SetTitle, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::sync::mpsc;

use crate::app::{App, AppEvent, Effect, Then, TuiOptions, subscription_query};
use crate::prefs::MouseMode;
use crate::ui;
use crate::ui::theme::probe::{PROBE_DEADLINE, probe_palette};
use crate::ui::transcript::ViewCache;
use crate::viewed::Publisher;
use crate::voice::control::{self, ControlRequest, Listener};

mod frames;
use frames::Frames;

/// Back off while offline, but leave time within §9.8's second for handshake and replay.
const RECONNECT_FIRST: Duration = Duration::from_millis(100);
const RECONNECT_MAX: Duration = Duration::from_millis(500);

// Moved once from a channel into the reducer; size is irrelevant.
#[allow(clippy::large_enum_variant)]
pub(crate) enum LoopEvent {
    #[cfg(unix)]
    Suspend,
    /// Suspend the UI and run `aether <args>` attached to the terminal.
    External(Vec<String>),
    App(AppEvent),
    /// From daemon `instance`, over the loop's connection number `generation`.
    AppFrom {
        instance: String,
        generation: u64,
        event: AppEvent,
    },
    Mouse(crossterm::event::MouseEvent),
    Reconnected(Client),
    VoiceControl(anyhow::Result<ControlRequest>),
}

/// The loop's daemon client, numbered from 1 per connection so an answer can
/// tell whether it came back over the connection the view now reflects.
#[derive(Clone)]
pub(crate) struct Link {
    pub(crate) client: Client,
    generation: u64,
    /// The first generation on this daemon identity. A change of identity
    /// rebuilds the view, so nothing asked for before it has a route left.
    since: u64,
}

impl Link {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            generation: 1,
            since: 1,
        }
    }

    /// Matches `App::link` once the reducer has seen this link connect.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn reconnected(&self, client: Client) -> Self {
        let generation = self.generation + 1;
        let same_daemon = self.client.welcome().map(|welcome| &welcome.instance_id)
            == client.welcome().map(|welcome| &welcome.instance_id);
        let since = if same_daemon { self.since } else { generation };
        debug_assert!(self.since <= since && since <= generation);
        Self {
            client,
            generation,
            since,
        }
    }

    /// The reducer's event, or `None` when it came from another daemon, or from
    /// a replaced connection and could only roll the view back.
    pub(crate) fn admit(
        &self,
        instance: &str,
        generation: u64,
        event: AppEvent,
    ) -> Option<AppEvent> {
        debug_assert!(
            generation <= self.generation,
            "an event from a connection not yet made"
        );
        let same_daemon = self
            .client
            .welcome()
            .is_some_and(|welcome| welcome.instance_id == instance);
        debug_assert!(
            same_daemon || generation < self.generation,
            "a connection stamps its own daemon's identity"
        );
        // Back on a daemon after another one: the view was rebuilt in between.
        if !same_daemon || generation < self.since {
            return None;
        }
        if generation == self.generation || event.outlives_connection() {
            return Some(event);
        }
        tracing::debug!(
            generation,
            current = self.generation,
            "dropped a replaced connection's event"
        );
        None
    }
}

/// Shared by signal, panic, suspension and normal exit. Keyboard pushes are
/// balanced once, even when a panic hook is followed by guard unwinding.
#[derive(Default)]
struct TerminalState {
    keyboard: KeyboardMode,
    active: AtomicBool,
}

impl TerminalState {
    fn restore(&self) {
        if let Err(error) = self.restore_checked() {
            tracing::warn!(%error, "could not restore terminal modes");
        }
    }

    fn restore_checked(&self) -> std::io::Result<()> {
        if !self.active.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let raw = disable_raw_mode();
        let screen = write_restore(&mut stdout());
        let keyboard = self.keyboard.disable(&mut stdout());
        raw.and(screen).and(keyboard)
    }
}

#[cfg(unix)]
fn suspend_terminal(
    mouse: bool,
    state: &TerminalState,
) -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    state.restore_checked()?;
    nix::sys::signal::raise(nix::sys::signal::Signal::SIGTSTP)?;
    let mut terminal = setup_terminal(mouse, state)?;
    // Fresh fullscreen buffers already force a redraw. Terminal::clear queries
    // the cursor, which can block behind EventStream's pending input reader.
    terminal.backend_mut().clear()?;
    Ok(terminal)
}

/// End a frame cut off mid-draw before leaving the alternate screen.
fn write_restore(out: &mut impl std::io::Write) -> std::io::Result<()> {
    execute!(
        out,
        EndSynchronizedUpdate,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )
}

struct TerminalGuard(Arc<TerminalState>);

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.0.restore();
    }
}

#[cfg(unix)]
struct ExitSignals {
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ExitSignals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.terminate.recv() => {},
            _ = self.hangup.recv() => {},
            _ = self.interrupt.recv() => {},
        }
    }
}

#[cfg(not(unix))]
struct ExitSignals;

#[cfg(not(unix))]
impl ExitSignals {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) {
        std::future::pending::<()>().await;
    }
}

fn setup_terminal(
    mouse: bool,
    state: &TerminalState,
) -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    state.active.store(true, Ordering::SeqCst);
    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange,
        // Alternate scroll off: without mouse capture the wheel must not arrive as ↑/↓.
        crossterm::style::Print("\x1b[?1007l")
    )?;
    state.keyboard.enable(&mut stdout())?;
    set_mouse_capture(mouse)?;
    let terminal = Terminal::new(native_backend(stdout()))?;
    Ok(terminal)
}

fn native_backend<W: std::io::Write>(out: W) -> CrosstermBackend<W> {
    // Theme preferences already resolve NO_COLOR; Crossterm suppression emits
    // empty SGR color parameters that also erase mono's weight and underline.
    crossterm::style::force_color_output(true);
    CrosstermBackend::new(out)
}

fn set_mouse_capture(on: bool) -> anyhow::Result<()> {
    if on {
        execute!(stdout(), EnableMouseCapture)?;
    } else {
        execute!(stdout(), DisableMouseCapture)?;
    }
    Ok(())
}

/// Run the TUI until the user quits. Agents keep running in the daemon.
pub async fn run(client: Client, options: TuiOptions) -> anyhow::Result<()> {
    run_inner(
        client,
        options,
        #[cfg(test)]
        None,
    )
    .await
}

async fn run_inner(
    client: Client,
    options: TuiOptions,
    #[cfg(test)] mut observer: Option<&mut process_tests::Observer>,
) -> anyhow::Result<()> {
    let mut exit_signals = ExitSignals::new()?;
    let state = Arc::new(TerminalState::default());
    let previous = std::panic::take_hook();
    let panic_state = state.clone();
    std::panic::set_hook(Box::new(move |info| {
        panic_state.restore();
        previous(info);
    }));
    let guard = TerminalGuard(state.clone());
    let mut terminal = setup_terminal(options.mouse != MouseMode::Off, &state)?;
    let mut app = App::new(options);
    // Probe before reading keys; dispatch also filters OSC replies that arrive late.
    app.set_palette(probe_palette(PROBE_DEADLINE));
    let size = terminal.size()?;
    app.start_at(size.width, size.height);
    app.bottom.approval.note_key(app.now);
    let mut cache = ViewCache::default();
    app.voice.available = cfg!(any(target_os = "linux", target_os = "macos"));
    // After `available`, so a platform without local voice never queues a
    // restoration, and before the first `Connected`, which is what applies it.
    app.load_voice_prefs();
    let mut voice = crate::voice::Service::default();
    let mut voice_control = None;
    let mut viewed = app
        .options
        .socket_path
        .as_deref()
        .map(|socket| Publisher::new(crate::viewed::viewed_dir(socket)));
    let mut link = Link::new(client);
    let (tx, mut rx) = mpsc::unbounded_channel::<LoopEvent>();
    let mut term_events = EventStream::new();
    let mut frames = Frames::new(Instant::now());
    let mut reconnecting = false;
    let mut title = String::new();
    let mut auto_scroll_due = None;

    let mut pending = app.reconnected(&link.client);
    debug_assert_eq!(
        app.link, link.generation,
        "effects carry the link they go out on"
    );
    let result: anyhow::Result<()> = async {
        loop {
            pending.extend(frames.advance_clock(&mut app, Instant::now()));
            pending.extend(reconcile_voice_control(&mut app, &mut voice_control).await);
            publish_viewed(&mut app, viewed.as_mut());
            let mut regular = Vec::new();
            for effect in std::mem::take(&mut pending) {
                match effect {
                    Effect::Voice(effect) => voice.dispatch(effect, tx.clone()),
                    effect => regular.push(effect),
                }
            }
            execute_effects(&link, &tx, regular, terminal.backend_mut());
            let now = Instant::now();
            if frames.should_draw(&app, now, !rx.is_empty()) {
                draw(&mut terminal, &app, &mut cache)?;
                for agent in std::mem::take(&mut cache.transcript.older_requests) {
                    let effects = app.older_history_request(agent).into_iter().collect();
                    execute_effects(&link, &tx, effects, terminal.backend_mut());
                }
                frames.drew(&mut app, now);
                let next = app.terminal_title();
                if next != title {
                    execute!(terminal.backend_mut(), SetTitle(&next))?;
                    title = next;
                }
            }
            let drag = cache.mouse.selection.as_ref()
                .filter(|s| {
                    s.dragging
                        && s.moved
                        && s.pane == ui::selection::Pane::Transcript
                        && (s.pointer.row <= s.rect.y || s.pointer.row >= s.rect.bottom())
                })
                .map(|s| s.pointer);
            if drag.is_some() {
                auto_scroll_due.get_or_insert_with(|| Instant::now() + Duration::from_millis(100));
            } else {
                auto_scroll_due = None;
            }
            let wake = frames.next_wake(&app);
            // Biased: keys first, so typing never queues behind a burst of frames.
            let event = tokio::select! {
                biased;
                _ = exit_signals.recv() => break,
                term = term_events.next() => match term {
                    Some(Ok(Event::Mouse(mouse))) => LoopEvent::Mouse(mouse),
                    Some(Ok(ev)) => match map_terminal_event(ev) {
                        Some(ev) => LoopEvent::App(ev),
                        None => continue,
                    },
                    Some(Err(e)) => anyhow::bail!("terminal input failed: {e}"),
                    None => break,
                },
                request = next_voice_control(&mut voice_control) => LoopEvent::VoiceControl(request),
                loop_event = rx.recv() => match loop_event {
                    Some(ev) => ev,
                    None => break,
                },
                () = sleep_until(wake) => continue,
                () = sleep_until(auto_scroll_due) => {
                    auto_scroll_due = Some(Instant::now() + Duration::from_millis(100));
                    match drag {
                        Some(mouse) => LoopEvent::Mouse(mouse),
                        None => continue,
                    }
                },
            };
            let event = match event {
                LoopEvent::AppFrom { instance, generation, event } => match link.admit(&instance, generation, event) {
                    Some(event) => LoopEvent::App(event),
                    None => continue,
                },
                event => event,
            };
            #[cfg(test)]
            if let Some(observer) = observer.as_deref_mut() {
                observer.before(&event);
            }
            // The loop may have slept for minutes: deadlines in the reducer must
            // see the tick the event arrived at, not the one it fell asleep on.
            pending = frames.advance_clock(&mut app, Instant::now());
            match event {
                LoopEvent::AppFrom { .. } => unreachable!("protocol events were normalized"),
                #[cfg(unix)]
                LoopEvent::Suspend => {
                    let replies_enabled = app.voice_replies_enabled();
                    voice_control = None;
                    for effect in app.voice_suspend() {
                        if let Effect::Voice(effect) = effect {
                            voice.dispatch(effect, tx.clone());
                        }
                    }
                    voice.shutdown().await?;
                    terminal = suspend_terminal(app.mouse != MouseMode::Off, &state)?;
                    app.voice_resume(replies_enabled);
                    let size = terminal.size()?;
                    pending.extend(dispatch_app_event(
                        &mut app,
                        &mut cache,
                        AppEvent::Resize(size.width, size.height),
                    ));
                    app.dirty = true;
                    title.clear();
                }
                LoopEvent::Reconnected(new_client) => {
                    pending.extend(app.reconnected(&new_client));
                    link = link.reconnected(new_client);
                    debug_assert_eq!(app.link, link.generation, "effects carry the link they go out on");
                    reconnecting = false;
                }
                LoopEvent::VoiceControl(request) => {
                    pending.extend(handle_voice_control(&mut app, request, &mut voice_control));
                }
                LoopEvent::External(args) => {
                    voice_control = None;
                    for effect in app.voice_suspend() {
                        if let Effect::Voice(effect) = effect {
                            voice.dispatch(effect, tx.clone());
                        }
                    }
                    voice.shutdown().await?;
                    let label = format!("aether {}", args.join(" "));
                    // Give the terminal to the child: raw mode off, main screen back.
                    state.restore();
                    let error = match std::env::current_exe() {
                        Ok(exe) => match tokio::process::Command::new(exe)
                            .args(&args)
                            .stdin(std::process::Stdio::inherit())
                            .stdout(std::process::Stdio::inherit())
                            .stderr(std::process::Stdio::inherit())
                            .status()
                            .await
                        {
                            Ok(status) if status.success() => None,
                            Ok(status) => Some(format!("exited with {status}")),
                            Err(e) => Some(format!("could not run it: {e}")),
                        },
                        Err(e) => Some(format!("could not find its own executable: {e}")),
                    };
                    terminal = setup_terminal(app.mouse != MouseMode::Off, &state)?;
                    // Fresh buffers force a redraw; avoid querying the cursor with active input.
                    terminal.backend_mut().clear()?;
                    app.dirty = true;
                    // The child may have set its own title.
                    title.clear();
                    pending.extend(app.reduce(AppEvent::ExternalFinished { label, error }));
                }
                LoopEvent::Mouse(mouse) => {
                    if cache.mouse.extend_after_scroll {
                        draw(&mut terminal, &app, &mut cache)?;
                        frames.drew(&mut app, Instant::now());
                    }
                    pending.extend(app.dispatch_mouse(mouse, &mut cache));
                    if matches!(mouse.kind, crossterm::event::MouseEventKind::Down(_)) {
                        draw(&mut terminal, &app, &mut cache)?;
                        frames.drew(&mut app, Instant::now());
                    }
                }
                LoopEvent::App(ev) => {
                    let disconnected = matches!(
                        ev,
                        AppEvent::Disconnected { .. }
                            | AppEvent::HistoryExpired
                            | AppEvent::Frame(SubscriptionFrame::Lagged { .. })
                    );
                    pending.extend(dispatch_app_event(&mut app, &mut cache, ev));
                    if disconnected && !reconnecting {
                        reconnecting = true;
                        spawn_reconnect(&app, tx.clone());
                    }
                }
            }
            #[cfg(test)]
            if let Some(observer) = observer.as_deref_mut() {
                observer.after(&app)?;
            }
            if app.quit_requested {
                break;
            }
        }
        Ok(())
    }
    .await;
    state.restore();
    drop(voice_control);
    let cleanup = match (voice.shutdown().await, viewed.map(Publisher::finish)) {
        (Err(voice), Some(Err(viewed))) => Err(voice.context(format!("{viewed:#}"))),
        (voice, viewed) => voice.and(viewed.unwrap_or(Ok(()))),
    };
    drop(terminal);
    drop(guard);
    if let Some(message) = app.exit_message.take() {
        println!("{message}");
    }
    match (result, cleanup) {
        (Err(cause), Err(cleanup)) => {
            Err(cause.context(format!("voice shutdown also failed: {cleanup:#}")))
        }
        (Err(cause), Ok(())) => Err(cause),
        (Ok(()), result) => result,
    }
}

/// One frame inside a synchronized update: the terminal shows it whole
/// rather than tearing partway through a large diff.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
    cache: &mut ViewCache,
) -> std::io::Result<()> {
    queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(|f| ui::render(f, app, cache)).map(drop);
    // End even after a failed draw: the terminal would hold the screen until its own timeout.
    let ended = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    drawn.and(ended)
}

async fn sleep_until(wake: Option<Instant>) {
    match wake {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// Keep `@viewed` naming this TUI's agent; a failure is shown, not fatal.
fn publish_viewed(app: &mut App, viewed: Option<&mut Publisher>) {
    let Some(viewed) = viewed else {
        return;
    };
    if let Err(error) = viewed.sync(app.selected) {
        app.flash_error(format!("`@viewed` will not name this view: {error:#}"));
    }
}

async fn reconcile_voice_control(app: &mut App, listener: &mut Option<Listener>) -> Vec<Effect> {
    if !app.voice_replies_enabled() {
        *listener = None;
        return Vec::new();
    }
    if listener.is_some() {
        return Vec::new();
    }
    let Some(socket) = app.options.socket_path.as_deref() else {
        return disable_voice_control(app, "the TUI has no local daemon socket".into());
    };
    match Listener::start(control::control_socket_path(socket)).await {
        Ok(started) => {
            *listener = Some(started);
            Vec::new()
        }
        Err(error) => disable_voice_control(app, format!("{error:#}")),
    }
}

async fn next_voice_control(listener: &mut Option<Listener>) -> anyhow::Result<ControlRequest> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

/// Losing the local control socket, or never getting it, disarms replies for
/// this run only: it is not the user opting out, so the saved setting stands.
fn disable_voice_control(app: &mut App, error: String) -> Vec<Effect> {
    let effects = app.disarm_voice_replies_transiently();
    app.flash_error(format!(
        "global voice replies disabled for this run: {error}; your saved setting is unchanged"
    ));
    effects
}

fn handle_voice_control(
    app: &mut App,
    request: anyhow::Result<ControlRequest>,
    listener: &mut Option<Listener>,
) -> Vec<Effect> {
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            *listener = None;
            return disable_voice_control(app, format!("{error:#}"));
        }
    };
    if !request.is_live() {
        app.flash("expired voice shortcut ignored; nothing recorded or sent");
        return Vec::new();
    }
    let (effects, ok, message) = match app.handle_voice_control(request.action()) {
        Ok(effects) => (effects, true, app.voice_control_ack(request.action())),
        Err(error) => {
            app.flash_error(format!("global voice reply: {error}"));
            (Vec::new(), false, error)
        }
    };
    if let Err(error) = request.ack(ok, message) {
        app.flash_error(format!(
            "voice command processed, but shortcut acknowledgement failed: {error:#}"
        ));
    }
    effects
}

#[cfg(test)]
mod frames_tests;
#[cfg(all(test, target_os = "linux"))]
mod paste_tests;
#[cfg(test)]
mod process_tests;
#[cfg(all(test, target_os = "linux"))]
mod suspend_tests;
#[cfg(test)]
mod voice_control_tests;

fn map_terminal_event(ev: Event) -> Option<AppEvent> {
    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(AppEvent::Key(key)),
        Event::Key(_) => None,
        Event::Mouse(_) => None,
        Event::Paste(text) => Some(paste_event(text)),
        Event::Resize(w, h) => Some(AppEvent::Resize(w, h)),
        Event::FocusGained => Some(AppEvent::Focus(true)),
        Event::FocusLost => Some(AppEvent::Focus(false)),
    }
}

/// A terminal paste: empty image signal, text, or the image file the text names.
pub(crate) fn paste_event(text: String) -> AppEvent {
    if text.is_empty() {
        return AppEvent::TerminalImagePaste;
    }
    let home = crate::workspace::home_dir();
    let Some(path) = crate::image_input::pasted_image_path(&text, home.as_deref()) else {
        return AppEvent::Paste(text);
    };
    let image = aether_client::images::read_image_file(&path).map_err(|e| e.to_string());
    debug_assert!(!text.trim().is_empty());
    debug_assert!(path.extension().is_some());
    AppEvent::ImagePathPasted { text, image }
}

fn dispatch_app_event(app: &mut App, cache: &mut ViewCache, event: AppEvent) -> Vec<Effect> {
    if matches!(event, AppEvent::Resize(..))
        || matches!(event, AppEvent::Key(key) if key.code == crossterm::event::KeyCode::Esc)
    {
        let cleared = cache.mouse.selection.is_some();
        cache.mouse.clear_selection();
        app.dirty |= cleared;
    }
    app.reduce(event)
}

/// Read an image off the UI loop; the helper behind a clipboard can take seconds.
fn spawn_image_read(tx: mpsc::UnboundedSender<LoopEvent>, source: ImageSource) {
    tokio::spawn(async move {
        let image = crate::image_input::read(source).await;
        let _ = tx.send(LoopEvent::App(AppEvent::ImageRead(image)));
    });
}

fn spawn_clipboard_paste(tx: mpsc::UnboundedSender<LoopEvent>) {
    tokio::spawn(async move {
        let content = crate::image_input::paste_clipboard().await;
        let _ = tx.send(LoopEvent::App(AppEvent::ClipboardRead(content)));
    });
}

pub(crate) fn execute_effects(
    link: &Link,
    tx: &mpsc::UnboundedSender<LoopEvent>,
    effects: Vec<Effect>,
    out: &mut impl std::io::Write,
) {
    for effect in effects {
        match effect {
            #[cfg(unix)]
            Effect::Suspend => {
                let _ = tx.send(LoopEvent::Suspend);
            }
            Effect::Voice(_) => {
                let _ = tx.send(LoopEvent::App(AppEvent::EffectFailed {
                    action: "local voice".into(),
                    error: "requires the interactive TUI".into(),
                }));
            }
            Effect::ReloadViewModes(dir) => {
                let tx = tx.clone();
                tokio::task::spawn_blocking(move || {
                    let catalog = crate::view_modes::discover(&dir);
                    let _ = tx.send(LoopEvent::App(AppEvent::ViewModesLoaded(catalog)));
                });
            }
            Effect::Quit => {}
            Effect::Request { command, then } => {
                spawn_request(link.clone(), tx.clone(), command, then)
            }
            Effect::Subscribe { after } => spawn_subscription(link.clone(), tx.clone(), after),
            Effect::Outbox { settle } => spawn_outbox(link.clone(), tx.clone(), settle),
            Effect::RunExternal { args } => {
                let _ = tx.send(LoopEvent::External(args));
            }
            Effect::SetMouse(on) => {
                if let Err(e) = set_mouse_capture(on) {
                    tracing::warn!("mouse capture change failed: {e}");
                    let _ = tx.send(LoopEvent::App(AppEvent::EffectFailed {
                        action: "mouse capture change".into(),
                        error: e.to_string(),
                    }));
                }
            }
            Effect::Copy(text) => {
                spawn_copy(tx.clone(), text, out);
            }
            Effect::AttachImage(source) => spawn_image_read(tx.clone(), source),
            Effect::PasteClipboard => spawn_clipboard_paste(tx.clone()),
            Effect::Bell => {
                use std::io::Write as _;
                let mut out = stdout();
                let _ = out.write_all(b"\x07").and_then(|()| out.flush());
            }
            // OSC 777 where it is understood (foot, WezTerm, some others),
            // OSC 9 elsewhere. Both are ignored by terminals that do not
            // implement them, so no capability check is needed.
            Effect::Notify { title, body } => {
                use std::io::Write as _;
                let clean = |s: &str| s.replace(['\x1b', '\x07', ';'], " ");
                let mut out = stdout();
                let _ = write!(
                    out,
                    "\x1b]777;notify;{};{}\x1b\\\x1b]9;{}: {}\x07",
                    clean(&title),
                    clean(&body),
                    clean(&title),
                    clean(&body)
                )
                .and_then(|()| out.flush());
            }
        }
    }
}

fn spawn_copy(tx: mpsc::UnboundedSender<LoopEvent>, text: String, out: &mut impl std::io::Write) {
    let terminal = crate::clipboard::send_osc52(&text, out);
    tokio::spawn(async move {
        let event = match crate::clipboard::copy(text, terminal).await {
            Ok((lines, confirmed)) => AppEvent::Copied { lines, confirmed },
            Err(error) => AppEvent::EffectFailed {
                action: "clipboard copy".into(),
                error,
            },
        };
        let _ = tx.send(LoopEvent::App(event));
    });
}

pub(crate) fn protocol_event(link: &Link, event: AppEvent) -> LoopEvent {
    match link.client.welcome() {
        Some(welcome) => LoopEvent::AppFrom {
            instance: welcome.instance_id.clone(),
            generation: link.generation,
            event,
        },
        None => LoopEvent::App(event),
    }
}

fn spawn_request(link: Link, tx: mpsc::UnboundedSender<LoopEvent>, command: Command, then: Then) {
    tokio::spawn(async move {
        let result = link.client.request(command).await;
        let disconnected = matches!(result, Err(ClientError::Disconnected | ClientError::Ssh(_)));
        let because = disconnected
            .then(|| link.client.disconnect_reason())
            .flatten();
        let _ = tx.send(protocol_event(&link, AppEvent::Reply { then, result }));
        if disconnected {
            let _ = tx.send(protocol_event(
                &link,
                AppEvent::Disconnected {
                    because: because.map(|notice| notice.message),
                },
            ));
        }
    });
}

fn spawn_outbox(
    link: Link,
    tx: mpsc::UnboundedSender<LoopEvent>,
    settle: Option<(String, aether_client::Settlement)>,
) {
    tokio::spawn(async move {
        let settled = match settle {
            Some((key, choice)) => Some((choice, link.client.settle(&key, choice).await)),
            None => None,
        };
        let undecided = link.client.undecided().await;
        let event = AppEvent::Outbox { settled, undecided };
        let _ = tx.send(protocol_event(&link, event));
    });
}

fn spawn_subscription(link: Link, tx: mpsc::UnboundedSender<LoopEvent>, after: Option<u64>) {
    tokio::spawn(async move {
        match link
            .client
            .subscribe(after, subscription_query(), true)
            .await
        {
            Ok(mut sub) => {
                let _ = tx.send(protocol_event(
                    &link,
                    AppEvent::Subscribed {
                        id: sub.id(),
                        last_seq: sub.last_seq(),
                    },
                ));
                while let Some(frame) = sub.next().await {
                    let lagged = matches!(frame, SubscriptionFrame::Lagged { .. });
                    if tx
                        .send(protocol_event(&link, AppEvent::Frame(frame)))
                        .is_err()
                    {
                        return;
                    }
                    if lagged {
                        return;
                    }
                }
                let _ = tx.send(protocol_event(
                    &link,
                    if link.client.history_expired() {
                        AppEvent::HistoryExpired
                    } else {
                        AppEvent::Disconnected {
                            because: link.client.disconnect_reason().map(|notice| notice.message),
                        }
                    },
                ));
            }
            Err(ClientError::Protocol(error))
                if after.is_some() && error.code == aether_protocol::ErrorCode::NotFound =>
            {
                let _ = tx.send(protocol_event(&link, AppEvent::HistoryExpired));
            }
            Err(_) if link.client.history_expired() => {
                let _ = tx.send(protocol_event(&link, AppEvent::HistoryExpired));
            }
            Err(e) => {
                tracing::warn!("subscribe failed: {e}");
                let _ = tx.send(protocol_event(
                    &link,
                    AppEvent::Disconnected { because: None },
                ));
            }
        }
    });
}

/// Reconnect in the background, telling the loop when a client is up. Both
/// clients use it: a person in `--plain` waits for the daemon exactly as long
/// as a person in the TUI does (§9.8).
pub(crate) fn spawn_reconnect(app: &App, tx: mpsc::UnboundedSender<LoopEvent>) {
    let endpoint = app.options.endpoint.clone().unwrap_or_else(|| {
        aether_client::Endpoint::Socket(
            app.options
                .socket_path
                .clone()
                .unwrap_or_else(aether_protocol::paths::socket_path),
        )
    });
    tokio::spawn(async move {
        let mut wait = if endpoint.is_remote() {
            Duration::from_secs(1)
        } else {
            RECONNECT_FIRST
        };
        let maximum = if endpoint.is_remote() {
            Duration::from_secs(30)
        } else {
            RECONNECT_MAX
        };
        loop {
            let jitter =
                Duration::from_millis(rand::random::<u64>() % (wait.as_millis() as u64 / 2 + 1));
            tokio::select! { _ = tx.closed() => return, _ = tokio::time::sleep((wait + jitter).min(maximum)) => {} }
            let connected = tokio::select! { _ = tx.closed() => return, result = Client::connect_endpoint(&endpoint) => result };
            match connected {
                Ok(client) => {
                    let _ = tx.send(LoopEvent::Reconnected(client));
                    return;
                }
                Err(e) => {
                    tracing::debug!("reconnect attempt failed: {e}");
                    if endpoint.is_remote() {
                        let _ = tx.send(LoopEvent::App(AppEvent::EffectFailed {
                            action: "SSH reconnect".into(),
                            error: e.to_string(),
                        }));
                    }
                }
            }
            wait = (wait * 2).min(maximum);
        }
    });
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "current_thread")]
    async fn clipboard_terminal_write_finishes_before_dispatch_returns() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut output = Vec::new();
        super::spawn_copy(tx, "copied text".into(), &mut output);
        assert_eq!(
            output,
            crate::clipboard::osc52("copied text").unwrap().as_bytes()
        );
    }

    #[test]
    fn escape_after_copy_clears_selection_and_interrupts_in_one_press() {
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        let mut app = crate::testing::app_with_transcript(&["prompt", "reply"]);
        app.size = (160, 44);
        app.reduce(crate::app::AppEvent::Paste("copy me".into()));
        let mut cache = crate::ui::transcript::ViewCache::default();
        crate::testing::render_with(&app, &mut cache, 160, 44);
        let r = cache.regions.composer;
        let mouse = |kind, x| MouseEvent {
            kind,
            column: x,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        };
        app.dispatch_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), r.x),
            &mut cache,
        );
        let copied = app.dispatch_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), r.x + 6),
            &mut cache,
        );
        assert!(
            copied
                .iter()
                .any(|effect| matches!(effect, crate::app::Effect::Copy(_)))
        );
        let effects = super::dispatch_app_event(
            &mut app,
            &mut cache,
            crate::app::AppEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        assert!(cache.mouse.selection.is_none());
        assert!(effects.iter().any(|effect| matches!(
            effect,
            crate::app::Effect::Request {
                command: aether_protocol::Command::Interrupt { .. },
                ..
            }
        )));
    }

    #[test]
    #[cfg(unix)]
    fn terminal_restore_child() {
        let Ok(mode) = std::env::var("AETHER_TERMINAL_RESTORE_TEST") else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut signals = super::ExitSignals::new().expect("signal handlers");
            let state = std::sync::Arc::new(super::TerminalState::default());
            let previous = std::panic::take_hook();
            let panic_state = state.clone();
            std::panic::set_hook(Box::new(move |info| {
                panic_state.restore();
                previous(info);
            }));
            let _guard = super::TerminalGuard(state.clone());
            let mut terminal = super::setup_terminal(true, &state).expect("terminal");
            crossterm::execute!(
                terminal.backend_mut(),
                crossterm::terminal::BeginSynchronizedUpdate,
                crossterm::cursor::Hide,
                crossterm::style::Print("RESTORE_READY")
            )
            .expect("ready");
            if mode == "SUSPEND" {
                terminal = super::suspend_terminal(true, &state).expect("suspend and resume");
                crossterm::execute!(
                    terminal.backend_mut(),
                    crossterm::style::Print("RESUME_READY")
                )
                .expect("resume marker");
                use std::io::Read as _;
                std::io::stdin().read_exact(&mut [0]).expect("exit key");
            } else if mode == "TERM" || mode == "HUP" {
                signals.recv().await;
            } else {
                use std::io::Read as _;
                std::io::stdin().read_exact(&mut [0]).expect("exit key");
                assert_ne!(mode, "panic", "intentional terminal restore panic");
            }
            state.restore();
            state.restore();
        });
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn normal_panic_and_exit_signals_restore_all_terminal_modes() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/test_tui_terminal_restore.py");
        let status = std::process::Command::new("python3")
            .arg(script)
            .arg(std::env::current_exe().expect("test executable"))
            .status()
            .expect("PTY controller");
        assert!(status.success(), "terminal restore controller: {status}");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn suspend_restores_terminal_then_resume_reenters_it() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/test_tui_terminal_restore.py");
        let status = std::process::Command::new("python3")
            .arg(script)
            .arg(std::env::current_exe().expect("test executable"))
            .arg("SUSPEND")
            .status()
            .expect("PTY controller");
        assert!(status.success(), "suspend/resume controller: {status}");
    }

    #[test]
    fn native_backend_preserves_theme_styles_with_no_color() {
        use crate::ui::theme::tokens::{Theme, ThemeMode, ThemeSpec};
        use ratatui::backend::Backend;
        use ratatui::buffer::Cell;

        const CHILD: &str = "AETHER_TEST_NATIVE_COLORS";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "runner::tests::native_backend_preserves_theme_styles_with_no_color",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("NO_COLOR", "1")
                .output()
                .expect("isolated color policy test");
            assert!(output.status.success(), "{output:?}");
            return;
        }
        for mode in [ThemeMode::Dark, ThemeMode::Mono] {
            let theme = Theme::resolve(ThemeSpec {
                mode,
                ..ThemeSpec::default()
            });
            let mut failed = Cell::default();
            failed
                .set_symbol("•")
                .set_style(theme.failure().patch(theme.strong()));
            let mut text = Cell::default();
            text.set_symbol("x").set_style(theme.text());
            let mut output = Vec::new();
            super::native_backend(&mut output)
                .draw([(0, 0, &failed), (1, 0, &text)].into_iter())
                .expect("draw");
            let output = String::from_utf8(output).expect("ANSI text");
            assert!(output.contains("\x1b[1m"), "{mode:?}: {output:?}");
            assert!(!output.contains("\x1b[;m"), "{mode:?}: {output:?}");
            let colored = output.contains("38;2;") || output.contains("38;5;");
            assert_eq!(colored, mode != ThemeMode::Mono, "{output:?}");
        }
    }

    #[test]
    fn restoring_the_terminal_ends_a_synchronized_update_first() {
        let mut out = Vec::new();
        super::write_restore(&mut out).expect("in-memory write");
        let out = String::from_utf8(out).expect("escape sequences are ASCII");
        assert!(
            out.starts_with("\x1b[?2026l"),
            "a frame cut off by a panic leaves mode 2026 on: {out:?}"
        );
        assert!(
            out.contains("\x1b[?1049l"),
            "and leaves the alternate screen"
        );
    }
}

#[cfg(test)]
#[path = "runner/history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "runner/reconnect_tests.rs"]
mod reconnect_tests;
