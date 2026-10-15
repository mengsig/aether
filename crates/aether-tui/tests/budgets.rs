#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! VISION §9.8 and SPEC Phase 7 measured, not asserted on faith. Run it for
//! the numbers:
//!
//!     cargo test --release -p aether-tui --test budgets -- --nocapture
//!
//! Every case prints what it measured and then holds it to the budget.
//!
//! The absolute budgets are release numbers — an unoptimised build is an
//! order of magnitude slower and scaling for it would make the gate
//! meaningless — so a debug run does not gate them. `scripts/ci.sh` runs this
//! suite in release, which is where §9.8 means anything. Ratios and draw
//! counts do not depend on the build, so those gate in both. The memory
//! budget lives in `budget_memory.rs` so it has a process to itself to measure.

use std::time::{Duration, Instant};

use aether_client::SubscriptionFrame;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, EventId, InvocationId};
use aether_core::model::{ContentBlock, MessageRole};
use aether_protocol::{Delta, Reply};
use aether_tui::app::{App, AppEvent, IDLE_PERIOD_MS, TuiOptions};
use aether_tui::motion;
use aether_tui::testing::{app_with_transcript, loaded_app};
use aether_tui::ui;
use aether_tui::ui::theme::ColorDepth;
use aether_tui::ui::theme::tokens::{Rgb, TerminalPalette, Theme, ThemeMode, ThemeSpec};
use aether_tui::ui::transcript::ViewCache;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Headroom over §9.8's numbers, the same in every build. §9.8 describes a
/// person's terminal on an idle machine; a shared runner is not that. It is
/// the only slack there is, and it is not a place to put a regression:
/// widening it takes the same argument as widening a budget.
const SLACK: u32 = 2;

/// Enough samples that the median and the p99 mean something. The max does
/// not — one scheduler hiccup owns it — so nothing asserts on it.
const SAMPLES: usize = 200;

/// Frames drawn before measuring, so the wrap cache is as warm as a running
/// TUI's.
const WARMUP: usize = 20;

/// The viewport §9.8 names.
const VIEWPORT: (u16, u16) = (160, 200);

/// One frame at 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);

fn budget(d: Duration) -> Duration {
    d * SLACK
}

struct Measured {
    median: Duration,
    p99: Duration,
}

fn report(what: &str, samples: &mut [Duration]) -> Measured {
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let p99 = samples[samples.len() * 99 / 100];
    println!(
        "{what}: median {median:?} · p99 {p99:?} · worst {:?} ({} samples)",
        samples[samples.len() - 1],
        samples.len()
    );
    Measured { median, p99 }
}

/// A terminal and the render caches the runner would keep across its frames.
struct Screen {
    terminal: Terminal<TestBackend>,
    cache: ViewCache,
}

impl Screen {
    fn new() -> Screen {
        Screen {
            terminal: Terminal::new(TestBackend::new(VIEWPORT.0, VIEWPORT.1)).expect("terminal"),
            cache: ViewCache::default(),
        }
    }

    fn draw(&mut self, app: &App) {
        let cache = &mut self.cache;
        self.terminal
            .draw(|f| ui::render(f, app, cache))
            .expect("draw");
    }
}

/// Time `step` followed by a draw, `SAMPLES` times after `WARMUP` untimed rounds.
fn time_frames(app: &mut App, mut step: impl FnMut(&mut App, usize)) -> Vec<Duration> {
    let mut screen = Screen::new();
    for i in 0..WARMUP {
        step(app, i);
        screen.draw(app);
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for i in WARMUP..WARMUP + SAMPLES {
        let start = Instant::now();
        step(app, i);
        screen.draw(app);
        samples.push(start.elapsed());
    }
    samples
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.reduce(AppEvent::Key(KeyEvent::new(code, modifiers)));
}

fn delta(app: &mut App, agent: AgentId, invocation: InvocationId, delta: Delta) {
    app.reduce(AppEvent::Frame(SubscriptionFrame::Delta {
        agent,
        invocation,
        delta,
    }));
}

/// §9.8: a full render of a 200-line viewport, under 4 ms typical and 16 ms
/// worst. "Worst" is the p99 here; the single slowest of two hundred frames
/// measures the machine, not the renderer.
fn hold_a_render_to_the_viewport_budget(what: &str, app: &mut App) {
    let mut samples = time_frames(app, |_, _| {});
    let m = report(what, &mut samples);
    assert!(
        m.median < budget(Duration::from_millis(4)),
        "{what}: typical render {:?} over the 4 ms budget",
        m.median
    );
    assert!(
        m.p99 < budget(Duration::from_millis(16)),
        "{what}: p99 render {:?} over the 16 ms budget",
        m.p99
    );
}

/// Hold the p99 of a measured interaction to one frame.
fn hold_to_one_frame(what: &str, samples: &mut [Duration]) {
    let m = report(what, samples);
    assert!(
        m.p99 < budget(FRAME),
        "{what} took {:?} at p99, over one frame",
        m.p99
    );
}

#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn a_full_render_of_a_two_hundred_line_viewport_stays_within_budget() {
    let mut app = loaded_app(64, 2_000, 200);
    hold_a_render_to_the_viewport_budget("render 200 lines · 64 agents · 2000 events", &mut app);
}

/// The same viewport with a transcript fifty times longer. §9.8's render
/// budget is about the viewport, not the transcript, so this is held to the
/// same numbers: a virtualised list that is not virtualised fails here.
#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn a_long_transcript_renders_to_the_same_budget_as_a_short_one() {
    let mut app = loaded_app(8, 15_000, 5_000);
    hold_a_render_to_the_viewport_budget("render 10 000 cells · same viewport", &mut app);
}

#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn a_key_press_is_answered_within_one_frame() {
    let mut app = loaded_app(64, 2_000, 200);
    let mut samples = time_frames(&mut app, |app, i| {
        let c = (b'a' + (i % 26) as u8) as char;
        press(app, KeyCode::Char(c), KeyModifiers::NONE);
    });
    hold_to_one_frame("key press to echo", &mut samples);
}

/// Finding is a key press too, and it is the expensive one: every character
/// re-runs the query over the whole transcript.
#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn a_key_press_in_the_find_bar_is_answered_within_one_frame() {
    let mut app = loaded_app(8, 15_000, 5_000);
    press(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert!(app.bottom.find.is_some(), "Ctrl+F opens the find bar");
    let word = "about the build";
    for c in word.chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    let matched = app.bottom.find.as_ref().unwrap().matches.len();
    assert_eq!(matched, 5_000, "every question cell matches");
    // Delete the word and type it again, over and over.
    let keys: Vec<char> = word.chars().collect();
    let mut samples = time_frames(&mut app, |app, i| match keys.get(i % (keys.len() * 2)) {
        Some(_) => press(app, KeyCode::Backspace, KeyModifiers::NONE),
        None => press(app, KeyCode::Char(keys[i % keys.len()]), KeyModifiers::NONE),
    });
    hold_to_one_frame("key press in find · 10 000 cells", &mut samples);
}

/// Markdown in the shape a model streams: prose, a list and a fenced block.
const REPLY_BLOCK: &str = "The build runs in three stages, and each one caches what it can so a \
second run only redoes the work that changed since the last one.\n\n- fetch the \
dependencies\n- compile the crates\n- run the tests\n\n```rust\nfn main() {\n    \
println!(\"hello\");\n}\n```\n\n";

/// Tokens of the reply still streaming; a paragraph break every few.
const TOKENS: [&str; 8] = [
    "Each ",
    "stage ",
    "reports ",
    "what ",
    "it ",
    "did, ",
    "then ",
    "stops.\n\n",
];

/// The viewed agent mid-reply, `chars` of markdown already streamed in.
fn streaming(chars: usize) -> (App, AgentId, InvocationId) {
    let mut app = app_with_transcript(&["how does the build work?"]);
    let agent = app.selected.expect("an agent");
    let invocation = InvocationId::new();
    delta(&mut app, agent, invocation, Delta::TurnStarted { turn: 1 });
    let mut streamed = 0;
    while streamed < chars {
        delta(
            &mut app,
            agent,
            invocation,
            Delta::Text {
                text: REPLY_BLOCK.into(),
            },
        );
        streamed += REPLY_BLOCK.len();
    }
    (app, agent, invocation)
}

/// One reply streaming at a given length, with the screen drawing it.
struct Streaming {
    app: App,
    agent: AgentId,
    invocation: InvocationId,
    screen: Screen,
    samples: Vec<Duration>,
}

impl Streaming {
    fn new(chars: usize) -> Streaming {
        let (app, agent, invocation) = streaming(chars);
        Streaming {
            app,
            agent,
            invocation,
            screen: Screen::new(),
            samples: Vec::with_capacity(SAMPLES),
        }
    }

    fn step(&mut self, i: usize) -> Duration {
        let start = Instant::now();
        let text = TOKENS[i % TOKENS.len()].to_owned();
        delta(
            &mut self.app,
            self.agent,
            self.invocation,
            Delta::Text { text },
        );
        self.screen.draw(&self.app);
        start.elapsed()
    }
}

/// Median cost of one delta + frame at 50k chars streamed over that at 2k.
/// Samples alternate so both lengths see the same machine load; medians absorb
/// the rest, and the floor keeps a near-zero baseline out of the ratio.
fn delta_cost_ratio() -> (f64, Duration, Duration) {
    const FLOOR: Duration = Duration::from_micros(200);
    let mut short = Streaming::new(2_000);
    let mut long = Streaming::new(50_000);
    for i in 0..WARMUP {
        short.step(i);
        long.step(i);
    }
    for i in WARMUP..WARMUP + SAMPLES {
        let at = short.step(i);
        short.samples.push(at);
        let at = long.step(i);
        long.samples.push(at);
    }
    let short = report("one delta + frame · 2k chars streamed", &mut short.samples);
    let long = report("one delta + frame · 50k chars streamed", &mut long.samples);
    let ratio = long.median.as_secs_f64() / short.median.max(FLOOR).as_secs_f64();
    println!("per-delta cost ratio, 50k / 2k chars: {ratio:.2}");
    (ratio, long.median, short.median)
}

/// SPEC Phase 7: the cost of one more delta must not grow with the reply.
/// The reply is 25x longer, so re-rendering it would cost ~25x; flat is ~1x.
#[test]
fn a_streamed_delta_costs_the_same_at_any_reply_length() {
    const MAX_RATIO: f64 = 8.0;
    let (ratio, long, short) = delta_cost_ratio();
    assert!(
        ratio < MAX_RATIO,
        "a delta at 50k chars costs {long:?}, {ratio:.1}x one at 2k ({short:?}): streaming re-renders the reply",
    );
}

/// The release bound on the same ratio: settled rows are shared, never copied
/// per delta or per frame, so a longer reply adds next to nothing (~1.15x).
#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn a_streamed_delta_stays_flat_in_release() {
    const MAX_RATIO: f64 = 1.6;
    let (ratio, long, short) = delta_cost_ratio();
    assert!(
        ratio < MAX_RATIO,
        "a delta at 50k chars costs {long:?}, {ratio:.2}x one at 2k ({short:?}): per-delta work grows with the reply",
    );
}

/// SPEC Phase 7: Ctrl+B shows the docked board, or hides it, within a frame.
#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn the_board_toggles_within_one_frame() {
    let mut app = loaded_app(64, 2_000, 200);
    app.start_at(VIEWPORT.0, VIEWPORT.1);
    assert!(
        app.board.open,
        "the wide terminal starts with the board open"
    );
    press(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(!app.board.open, "one press closes even an unfocused board");
    press(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
    let mut was_open = app.board.open;
    assert!(was_open);
    let mut samples = time_frames(&mut app, |app, _| {
        press(app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_ne!(
            app.board.open, was_open,
            "every Ctrl+B here toggles the board"
        );
        was_open = app.board.open;
    });
    hold_to_one_frame("Ctrl+B board toggle · 64 agents", &mut samples);
}

/// `count` assistant replies on `agent`, after its (empty) backfill.
fn fill(app: &mut App, agent: AgentId, count: usize) {
    app.reduce(AppEvent::Reply {
        then: aether_tui::testing::backfill_then(app, agent),
        result: Ok(Reply::Events(vec![])),
    });
    for i in 0..count {
        let seq = app.transcripts.get(&agent).map_or(0, |t| t.max_seq) + 1;
        let kind = EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(format!(
                "Answer {i}.\n\nSome **markdown** with a list:\n\n- one\n- two"
            ))],
            invocation: None,
        };
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(
            Event::from_draft(
                EventDraft::new(Actor::System, kind).agent(agent),
                EventId::new(),
                seq,
                aether_core::time::now(),
            ),
        ))));
    }
}

/// SPEC Phase 7: viewing another agent whose transcript is 5 000 cells long
/// costs one frame, not a re-layout of everything it ever said.
#[cfg_attr(
    debug_assertions,
    ignore = "a release number: scripts/ci.sh gates it in release"
)]
#[test]
fn switching_to_an_agent_with_five_thousand_cells_takes_one_frame() {
    const CELLS: usize = 5_000;
    let mut app = loaded_app(2, 0, 0);
    let root = app.selected.expect("the root is viewed");
    let child = *app.tree.nodes[&root].children.first().expect("a child");
    for agent in [root, child] {
        fill(&mut app, agent, CELLS);
        assert!(app.transcripts[&agent].cells.len() >= CELLS);
    }
    let agents = [child, root];
    let mut samples = time_frames(&mut app, |app, i| {
        app.select(agents[i % 2]);
    });
    hold_to_one_frame("switch agents · 5 000 cells each", &mut samples);
}

/// SPEC Phase 7: agents streaming out of sight draw nothing. Ten other roots
/// stream for a simulated second while the viewed agent idles with the board
/// closed; a frame is drawn whenever the runner would draw one (`dirty` or
/// `background_dirty`), so the count is exact, not timed.
#[test]
fn ten_agents_streaming_in_the_background_draw_no_frames() {
    use aether_core::agent::AgentStatus;
    let mut app = loaded_app(11, 0, 0);
    let viewed = app.selected.expect("an agent");
    let others: Vec<AgentId> = app.tree.nodes[&viewed].children.clone();
    assert_eq!(others.len(), 10);
    // The children become roots of their own, out of the viewed agent's strip.
    for id in &others {
        let node = app.tree.nodes.get_mut(id).unwrap();
        node.agent.parent = None;
    }
    app.tree.nodes.get_mut(&viewed).unwrap().children.clear();
    app.tree.nodes.get_mut(&viewed).unwrap().agent.status =
        AgentStatus::Completed { summary: None };
    app.tree.roots.extend(others.iter().copied());
    app.board.open = false;
    app.flash = None;
    let invocations: Vec<InvocationId> = others.iter().map(|_| InvocationId::new()).collect();
    for (agent, invocation) in others.iter().zip(&invocations) {
        delta(
            &mut app,
            *agent,
            *invocation,
            Delta::TurnStarted { turn: 1 },
        );
    }
    app.dirty = false;
    app.background_dirty = false;

    let mut draws = 0;
    for frame in 0..motion::FRAME_HZ {
        for (n, (agent, invocation)) in others.iter().zip(&invocations).enumerate() {
            let text = TOKENS[(frame as usize + n) % TOKENS.len()].to_owned();
            delta(&mut app, *agent, *invocation, Delta::Text { text });
            delta(
                &mut app,
                *agent,
                *invocation,
                Delta::Thinking { text: "hm ".into() },
            );
        }
        app.reduce(AppEvent::Tick { frames: 1 });
        if app.dirty || app.background_dirty {
            draws += 1;
            app.dirty = false;
            app.background_dirty = false;
        }
    }
    assert_eq!(
        draws, 0,
        "background streaming drew {draws} frames in a second"
    );
    for agent in &others {
        assert!(
            !app.transcripts[agent].stream.is_empty(),
            "the deltas must have reached the hidden transcripts"
        );
    }
    let wake = app.next_tick_in();
    assert!(
        wake.is_none_or(|frames| frames >= motion::frames(IDLE_PERIOD_MS)),
        "an idle screen must not keep an animation clock: next tick in {wake:?} frames"
    );
}

/// Wake scheduling, not a CPU measurement or a no-timers assertion.
/// Process measurements live in runner::process_tests.
#[test]
fn the_frame_clock_sleeps_when_nothing_moves() {
    let mut idle = App::new(TuiOptions::default());
    assert_eq!(
        idle.next_tick_in(),
        None,
        "with nothing to animate, nothing should wake the loop"
    );
    // Connected, the wakes left are the status poll and the flash's expiry,
    // and neither is an animation clock.
    idle.reduce(AppEvent::Connected);
    let poll = idle
        .next_tick_in()
        .expect("a connected TUI polls the status");
    assert!(
        poll >= motion::frames(IDLE_PERIOD_MS),
        "an idle wake every {poll} frames is an animation clock"
    );
    // A running child in the viewed agent's strip moves: its clock ticks
    // each second, and where the palette is known it pulses (§2.1).
    let mut busy = loaded_app(2, 8, 1);
    assert!(
        busy.next_tick_in()
            .is_some_and(|frames| frames <= motion::frames(1_000)),
        "a running agent must drive its clock"
    );
    busy.theme = Theme::resolve(ThemeSpec {
        mode: ThemeMode::Dark,
        depth: ColorDepth::Truecolor,
        palette: Some(TerminalPalette {
            fg: Rgb(0xe6, 0xed, 0xf3),
            bg: Rgb(0x0d, 0x11, 0x17),
        }),
        ..ThemeSpec::default()
    });
    assert!(
        busy.next_tick_in()
            .is_some_and(|frames| frames <= motion::frames(ui::cells::PULSE_FRAME_MS)),
        "a running agent must drive the pulse"
    );
}
