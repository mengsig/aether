#![cfg(test)]

//! Posted notices under a dashed header, the `N new` divider and the `N new ↓`
//! footer chip, at 100 and 40 columns on a dark and a light theme, held against
//! `snapshots/*.txt`. `AETHER_UPDATE_SNAPSHOTS=1 cargo test -p aether-tui`
//! rewrites them.

use std::path::PathBuf;

use crate::app::App;
use crate::app::approval_flow::fixtures::{at, fleet};
use crate::app::transcript::{
    CellModel, NoticeKind, NoticeSeparator, NoticeText, ScrollIntent, UserFrom,
};
use crate::testing::render_with;
use crate::ui::theme::tokens::{Theme, ThemeMode, ThemeSpec};
use crate::ui::transcript::ViewCache;

const OUTCOME: &str = "Outcome (qi_01M8C6RX156NXJH82V2FSD8KTD)";

fn themes() -> [(&'static str, ThemeMode); 2] {
    [("dark", ThemeMode::Dark), ("light", ThemeMode::Light)]
}

fn app_in(mode: ThemeMode, width: u16) -> (App, aether_core::id::AgentId) {
    let mut f = fleet();
    f.app.board.open = false;
    f.app.size = (width, 22);
    f.app.theme = Theme::resolve(ThemeSpec {
        mode,
        ..ThemeSpec::default()
    });
    f.app.transcripts.entry(f.root).or_default().loaded = true;
    (f.app, f.root)
}

fn push(app: &mut App, agent: aether_core::id::AgentId, secs: i64, model: CellModel) {
    app.transcripts
        .entry(agent)
        .or_default()
        .push(at(secs), model);
}

fn reply(text: &str) -> CellModel {
    CellModel::Assistant { text: text.into() }
}

fn operator(text: &str) -> CellModel {
    CellModel::User {
        from: UserFrom::Operator,
        text: text.into(),
        images: Vec::new(),
    }
}

fn outcome_note() -> CellModel {
    CellModel::Notice {
        kind: NoticeKind::Note(aether_core::note::NoteLevel::Info),
        text: NoticeText::joined(
            &[
                &format!(
                    "{OUTCOME}: Done: the nightly build is green and the release notes are drafted"
                ),
                "https://example.com/pull/12",
            ],
            NoticeSeparator::Dot,
        ),
    }
}

fn blocker_trigger() -> CellModel {
    CellModel::Notice {
        kind: NoticeKind::Trigger { schedule: None },
        text: NoticeText::joined(
            &[
                "Trigger from archon",
                "Blocker (qi_01M8C6RX156NXJH82V2FSD8KTD): waiting for a review before the merge",
            ],
            NoticeSeparator::Dot,
        ),
    }
}

fn screen(app: &App, cache: &mut ViewCache, width: u16) -> String {
    render_with(app, cache, width, 22)
        .lines()
        .map(str::trim_end)
        // The agents strip lists the fleet in no fixed order.
        .filter(|row| !row.starts_with('●'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The same scene at both widths on both themes, one labelled block each.
fn capture(scene: impl Fn(u16, ThemeMode) -> String) -> String {
    let mut out = String::new();
    for width in [100, 40] {
        for (name, mode) in themes() {
            out.push_str(&format!("── {name} {width}\n{}\n", scene(width, mode)));
        }
    }
    out
}

fn reply_then_outcome(width: u16, mode: ThemeMode) -> String {
    let (mut app, agent) = app_in(mode, width);
    push(
        &mut app,
        agent,
        0,
        operator("Ship the TUI change when the build is green"),
    );
    push(
        &mut app,
        agent,
        1,
        reply("The build is running; I will report when it ends."),
    );
    push(&mut app, agent, 2, outcome_note());
    push(&mut app, agent, 3, blocker_trigger());
    screen(&app, &mut ViewCache::default(), width)
}

/// Three entries arrive while the operator sits idle.
fn idle_arrivals(width: u16, mode: ThemeMode) -> String {
    let (mut app, agent) = app_in(mode, width);
    push(
        &mut app,
        agent,
        0,
        operator("Ship the TUI change when the build is green"),
    );
    push(
        &mut app,
        agent,
        1,
        reply("The build is running; I will report when it ends."),
    );
    app.track_arrivals(true);
    push(&mut app, agent, 2, reply("The build is green."));
    push(&mut app, agent, 3, outcome_note());
    push(&mut app, agent, 4, blocker_trigger());
    app.track_arrivals(false);
    screen(&app, &mut ViewCache::default(), width)
}

fn scrolled_up(width: u16, mode: ThemeMode) -> String {
    let (mut app, agent) = app_in(mode, width);
    for i in 0..30 {
        push(&mut app, agent, i, reply(&format!("answer number {i}")));
    }
    app.track_arrivals(true);
    push(&mut app, agent, 40, reply("The build is green."));
    push(&mut app, agent, 41, outcome_note());
    app.track_arrivals(false);
    app.scroll_log.push(agent, ScrollIntent::PageUp);
    screen(&app, &mut ViewCache::default(), width)
}

fn check(name: &str, got: String) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/app/arrivals/snapshots");
    let path = dir.join(format!("{name}.txt"));
    if std::env::var_os("AETHER_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(&dir).expect("the snapshot dir");
        std::fs::write(&path, &got).expect("a snapshot write");
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(got, want, "{name} differs from its snapshot:\n{got}");
}

#[test]
fn a_reply_then_an_outcome_post_match_their_snapshot() {
    check("reply_then_outcome", capture(reply_then_outcome));
}

#[test]
fn three_arrivals_while_idle_match_their_snapshot() {
    check("idle_arrivals", capture(idle_arrivals));
}

#[test]
fn arrivals_while_scrolled_up_match_their_snapshot() {
    check("scrolled_up", capture(scrolled_up));
}

fn press(
    app: &mut App,
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) {
    app.reduce(crate::app::AppEvent::Key(crossterm::event::KeyEvent::new(
        code, modifiers,
    )));
}

fn arrived(mode: ThemeMode, width: u16) -> (App, aether_core::id::AgentId) {
    let (mut app, agent) = app_in(mode, width);
    push(&mut app, agent, 0, operator("Ship it"));
    push(&mut app, agent, 1, reply("Working on it."));
    app.track_arrivals(true);
    for i in 0..3 {
        push(&mut app, agent, 2 + i, reply(&format!("news {i}")));
    }
    app.track_arrivals(false);
    (app, agent)
}

#[test]
fn the_divider_counts_what_arrived_and_the_next_key_clears_it() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let (mut app, agent) = arrived(ThemeMode::Dark, 100);
    let unseen = app.arrivals.unseen(agent).expect("three unseen cells");
    assert_eq!(unseen.count, 3);
    assert!(screen(&app, &mut ViewCache::default(), 100).contains("── 3 new ──"));

    press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(app.arrivals.unseen(agent), None);
    assert!(!screen(&app, &mut ViewCache::default(), 100).contains(" new "));

    // What arrives after the input is new again, and only that.
    push(&mut app, agent, 9, reply("one more"));
    app.track_arrivals(false);
    assert_eq!(app.arrivals.unseen(agent).map(|u| u.count), Some(1));
}

#[test]
fn sending_a_message_clears_the_divider() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let (mut app, agent) = arrived(ThemeMode::Dark, 100);
    app.input.text = "thanks".into();
    app.input.cursor = app.input.text.len();
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.arrivals.unseen(agent), None);
}

#[test]
fn the_operators_own_messages_and_turn_separators_are_not_news() {
    let (mut app, agent) = app_in(ThemeMode::Dark, 100);
    push(&mut app, agent, 0, reply("Working on it."));
    app.track_arrivals(true);
    push(&mut app, agent, 1, operator("and then?"));
    push(
        &mut app,
        agent,
        2,
        CellModel::Separator {
            elapsed_ms: 1000,
            cost: None,
            agents: 1,
            finished_at: at(2),
        },
    );
    app.track_arrivals(false);
    assert_eq!(app.arrivals.unseen(agent), None);
    push(&mut app, agent, 3, reply("Done."));
    app.track_arrivals(false);
    let unseen = app.arrivals.unseen(agent).expect("one reply");
    assert_eq!(unseen.count, 1);
}

#[test]
fn scrolled_up_arrivals_show_in_the_footer_until_the_jump_to_latest() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let (mut app, agent) = app_in(ThemeMode::Dark, 100);
    for i in 0..30 {
        push(&mut app, agent, i, reply(&format!("answer number {i}")));
    }
    app.track_arrivals(true);
    push(&mut app, agent, 40, reply("The build is green."));
    app.track_arrivals(false);
    app.scroll_log.push(agent, ScrollIntent::PageUp);
    let mut cache = ViewCache::default();
    assert!(screen(&app, &mut cache, 100).contains("1 new ↓"));

    press(&mut app, KeyCode::End, KeyModifiers::CONTROL);
    let shown = screen(&app, &mut cache, 100);
    assert!(!shown.contains("new ↓"), "{shown}");
    assert!(shown.contains("The build is green."), "{shown}");
}

#[test]
fn nothing_new_shows_while_following_the_latest_row_without_arrivals() {
    let (mut app, agent) = app_in(ThemeMode::Dark, 100);
    push(&mut app, agent, 0, reply("hello"));
    app.track_arrivals(true);
    app.track_arrivals(false);
    assert_eq!(app.arrivals.unseen(agent), None);
    assert!(!screen(&app, &mut ViewCache::default(), 100).contains("new"));
}

#[test]
fn the_divider_and_the_post_header_differ_in_glyph_and_colour() {
    for (_, mode) in themes() {
        let theme = Theme::resolve(ThemeSpec {
            mode,
            ..ThemeSpec::default()
        });
        let divider = crate::ui::cells::separator::new_divider(3, 40, &theme);
        let header = crate::ui::cells::ChatCell::display_lines(&outcome_note(), 40, &theme);
        let header = &header[0];
        assert!(divider.to_string().starts_with("── 3 new "));
        assert!(header.to_string().starts_with("╌╌ "));
        assert_ne!(divider.spans[1].style.fg, header.spans[0].style.fg);
    }
}

#[test]
fn ascii_posts_and_dividers_use_only_ascii() {
    let theme = Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    });
    let divider = crate::ui::cells::separator::new_divider(2, 30, &theme).to_string();
    let rows = crate::ui::cells::ChatCell::display_lines(&outcome_note(), 60, &theme);
    let shown = rows.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert!(
        divider.is_ascii() && divider.starts_with("-- 2 new "),
        "{divider}"
    );
    assert!(shown.iter().all(|row| row.is_ascii()), "{shown:?}");
    assert!(shown[0].starts_with(".. Outcome | qi_01"), "{shown:?}");
    assert!(shown[1].starts_with("| Done:"), "{shown:?}");
}

#[test]
fn a_header_cuts_its_id_rather_than_wrapping() {
    let theme = Theme::resolve(ThemeSpec::default());
    for width in [60, 40, 30, 24, 16, 12, 8] {
        let rows = crate::ui::cells::ChatCell::display_lines(&outcome_note(), width, &theme);
        let header = rows[0].to_string();
        assert!(
            unicode_width::UnicodeWidthStr::width(header.as_str()) <= usize::from(width),
            "{width}: {header}"
        );
        assert!(
            header.contains("Outcome") || width < 12,
            "{width}: {header}"
        );
    }
    let rows = crate::ui::cells::ChatCell::display_lines(&outcome_note(), 40, &theme);
    assert!(rows[0].to_string().contains("qi_01M8C6RX156NXJH82V2F…"));
}

/// One frame as the escape sequences a terminal paints, so a person can see the colours.
fn ansi(buffer: &ratatui::buffer::Buffer, paper: Option<(u8, u8, u8)>) -> String {
    use ratatui::style::{Color, Modifier};
    let sgr = |color: Color, fg: bool| -> String {
        let base = if fg { 30 } else { 40 };
        match color {
            Color::Reset => String::new(),
            Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
            Color::Indexed(n) => format!("{};5;{n}", base + 8),
            Color::Black => format!("{base}"),
            Color::Red => format!("{}", base + 1),
            Color::Green => format!("{}", base + 2),
            Color::Yellow => format!("{}", base + 3),
            Color::Blue => format!("{}", base + 4),
            Color::Magenta => format!("{}", base + 5),
            Color::Cyan => format!("{}", base + 6),
            Color::Gray => format!("{}", base + 7),
            Color::DarkGray => format!("{}", base + 60),
            Color::LightRed => format!("{}", base + 61),
            Color::LightGreen => format!("{}", base + 62),
            Color::LightYellow => format!("{}", base + 63),
            Color::LightBlue => format!("{}", base + 64),
            Color::LightMagenta => format!("{}", base + 65),
            Color::LightCyan => format!("{}", base + 66),
            Color::White => format!("{}", base + 67),
        }
    };
    let width = usize::from(buffer.area.width);
    let mut out = String::new();
    for row in buffer.content().chunks(width) {
        for cell in row {
            let mut codes = vec!["0".to_owned()];
            if cell.modifier.contains(Modifier::BOLD) {
                codes.push("1".into());
            }
            if cell.modifier.contains(Modifier::DIM) {
                codes.push("2".into());
            }
            if cell.modifier.contains(Modifier::ITALIC) {
                codes.push("3".into());
            }
            codes.push(sgr(cell.fg, true));
            let bg = match (cell.bg, paper) {
                (Color::Reset, Some((r, g, b))) => Color::Rgb(r, g, b),
                (bg, _) => bg,
            };
            codes.push(sgr(bg, false));
            codes.retain(|code| !code.is_empty());
            out.push_str(&format!("\x1b[{}m{}", codes.join(";"), cell.symbol()));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// `cargo test -p aether-tui --lib preview_frames -- --ignored --nocapture`
/// paints the three scenes in colour, dark then light, at 100 and 40 columns.
#[test]
#[ignore = "prints coloured frames for a person to look at"]
fn preview_frames() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    type Scene = fn(u16, ThemeMode) -> App;
    let scenes: [(&str, Scene); 3] = [
        ("A: a reply, then posts", |w, m| {
            let (mut app, agent) = app_in(m, w);
            push(
                &mut app,
                agent,
                1,
                reply("The build is running; I will report when it ends."),
            );
            push(&mut app, agent, 2, outcome_note());
            push(&mut app, agent, 3, blocker_trigger());
            app
        }),
        ("B: three entries arrived while idle", |w, m| {
            arrived(m, w).0
        }),
        ("B: scrolled up, entries arrived", |w, m| {
            let (mut app, agent) = app_in(m, w);
            for i in 0..30 {
                push(&mut app, agent, i, reply(&format!("answer number {i}")));
            }
            app.track_arrivals(true);
            push(&mut app, agent, 40, reply("The build is green."));
            push(&mut app, agent, 41, outcome_note());
            app.track_arrivals(false);
            app.scroll_log.push(agent, ScrollIntent::PageUp);
            app
        }),
    ];
    for (title, scene) in scenes {
        for width in [100, 40] {
            for (name, mode) in themes() {
                let app = scene(width, mode);
                let mut terminal = Terminal::new(TestBackend::new(width, 22)).expect("terminal");
                let mut cache = ViewCache::default();
                terminal
                    .draw(|frame| crate::ui::render(frame, &app, &mut cache))
                    .expect("draw");
                let paper = (mode == ThemeMode::Light).then_some((255, 255, 255));
                println!("\n=== {title} · {name} · {width} columns ===");
                print!("{}", ansi(terminal.backend().buffer(), paper));
            }
        }
    }
}

#[test]
fn a_hidden_first_arrival_neither_carries_the_divider_nor_counts() {
    let (mut app, agent) = app_in(ThemeMode::Dark, 100);
    app.view_mode = "focus".into();
    assert!(
        app.view_modes.modes.contains_key("focus"),
        "the focus mode is built in"
    );
    push(&mut app, agent, 0, operator("Ship it"));
    push(&mut app, agent, 1, reply("Working on it."));
    app.track_arrivals(true);
    push(
        &mut app,
        agent,
        2,
        CellModel::Exec {
            call: None,
            command: "cargo build".into(),
            status: exec_status(),
            output: String::new(),
            exit: Some(0),
            duration_ms: Some(10),
            is_search: false,
            artifacts: Vec::new(),
        },
    );
    push(&mut app, agent, 3, reply("news one"));
    push(&mut app, agent, 4, reply("news two"));
    app.track_arrivals(false);
    let shown = app
        .arrivals
        .shown(agent, &app.transcripts[&agent], app.policy(Some(agent)))
        .expect("two shown arrivals");
    assert_eq!(shown.count, 2);
    let text = screen(&app, &mut ViewCache::default(), 100);
    assert!(text.contains("── 2 new ──"), "{text}");
}

fn exec_status() -> crate::app::transcript::CallStatus {
    crate::app::transcript::CallStatus::Succeeded
}
