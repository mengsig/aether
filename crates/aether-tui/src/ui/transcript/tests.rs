#![cfg(test)]

//! The transcript view: only visible cells are laid out, scroll intents move
//! an anchor that survives new cells and Ctrl+T, the prompt pins, finds show.

use aether_core::id::{AgentId, InvocationId};
use aether_core::time::now;
use aether_protocol::Delta;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

use super::*;
use crate::app::TuiOptions;
use crate::app::transcript::UserFrom;
use crate::ui::bottom::find::FindBar;
use crate::ui::cells::text::{Layout, RowSource};
use crate::ui::theme::tokens::ThemeSpec;

const W: u16 = 60;
const H: u16 = 12;

struct View {
    app: App,
    agent: AgentId,
    cache: ViewCache,
    theme: Theme,
    terminal: Terminal<TestBackend>,
}

impl View {
    fn new() -> Self {
        let mut app = App::new(TuiOptions::default());
        let agent = AgentId::new();
        app.selected = Some(agent);
        app.transcripts.insert(agent, AgentTranscript::default());
        Self {
            app,
            agent,
            cache: ViewCache::default(),
            theme: Theme::resolve(ThemeSpec::default()),
            terminal: Terminal::new(TestBackend::new(W, H)).expect("a test terminal"),
        }
    }

    fn transcript(&mut self) -> &mut AgentTranscript {
        self.app
            .transcripts
            .get_mut(&self.agent)
            .expect("the viewed transcript")
    }

    fn push(&mut self, model: CellModel) -> CellId {
        self.transcript().push_for_test(now(), model)
    }

    /// `n` one-row replies, `reply 1` … `reply n`.
    fn replies(&mut self, from: usize, n: usize) {
        for i in from..from + n {
            self.push(CellModel::Assistant {
                text: format!("reply {i}"),
            });
        }
    }

    fn scroll(&mut self, intent: ScrollIntent) {
        self.app.scroll_log.push(self.agent, intent);
    }

    /// Draw a frame; the screen's rows, trailing blanks trimmed.
    fn frame(&mut self) -> Vec<String> {
        let Self {
            app,
            cache,
            theme,
            terminal,
            ..
        } = self;
        terminal
            .draw(|frame| render(frame, frame.area(), app, cache, theme))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(W))
            .map(|row| {
                let text: String = row.iter().map(|c| c.symbol()).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    fn anchor(&self) -> Anchor {
        self.cache.transcript.anchor(self.agent)
    }
}

/// Each one-row reply takes two rows (gap and text), so `H` rows show the
/// last `H / 2` replies.
#[test]
fn the_first_frame_of_5000_cells_lays_out_only_the_visible_ones() {
    let mut view = View::new();
    view.replies(1, 5_000);
    let rows = view.frame();
    assert_eq!(rows.last().map(String::as_str), Some("• reply 5000"));
    assert_eq!(rows[1], "• reply 4995");
    let inserts = view.cache.layout.inserts();
    assert!(
        inserts <= u64::from(H),
        "laid out {inserts} cells for {H} rows"
    );
    assert_eq!(view.anchor(), Anchor::Latest);
}

#[test]
fn scrolling_through_5000_cells_measures_only_what_it_crosses() {
    let mut view = View::new();
    view.replies(1, 5_000);
    view.frame();
    for _ in 0..3 {
        view.scroll(ScrollIntent::PageUp);
    }
    let rows = view.frame();
    // Three pages of 11 rows: 33 rows, 16½ replies up from the bottom-aligned top.
    assert!(rows.iter().any(|r| r == "• reply 4979"), "{rows:#?}");
    assert!(
        view.cache.layout.inserts() < 60,
        "{}",
        view.cache.layout.inserts()
    );
    view.scroll(ScrollIntent::Top);
    let rows = view.frame();
    assert_eq!(rows[0], "• reply 1");
    assert_eq!(rows[2], "• reply 2");
    assert!(
        view.cache.layout.inserts() < 80,
        "{}",
        view.cache.layout.inserts()
    );
}

#[test]
fn reading_holds_its_place_as_cells_arrive_and_latest_resumes_following() {
    let mut view = View::new();
    view.replies(1, 40);
    view.frame();
    view.scroll(ScrollIntent::LineUp(4));
    let before = view.frame();
    assert!(matches!(view.anchor(), Anchor::Reading { .. }));
    view.replies(41, 5);
    assert_eq!(
        view.frame(),
        before,
        "new cells below do not move the reader"
    );
    view.scroll(ScrollIntent::Latest);
    let rows = view.frame();
    assert_eq!(rows.last().map(String::as_str), Some("• reply 45"));
    assert_eq!(view.anchor(), Anchor::Latest);
    view.replies(46, 1);
    let rows = view.frame();
    assert_eq!(
        rows.last().map(String::as_str),
        Some("• reply 46"),
        "following"
    );
}

#[test]
fn scrolling_down_to_the_bottom_turns_back_into_following() {
    let mut view = View::new();
    view.replies(1, 40);
    view.frame();
    view.scroll(ScrollIntent::LineUp(3));
    view.frame();
    view.scroll(ScrollIntent::LineDown(2));
    view.frame();
    assert!(matches!(view.anchor(), Anchor::Reading { .. }));
    view.scroll(ScrollIntent::LineDown(1));
    view.frame();
    assert_eq!(view.anchor(), Anchor::Latest);
    view.scroll(ScrollIntent::PageDown);
    view.frame();
    assert_eq!(
        view.anchor(),
        Anchor::Latest,
        "past the bottom stays following"
    );
}

#[test]
fn ctrl_t_keeps_the_cell_at_the_top_of_the_view() {
    let mut view = View::new();
    for i in 1..=30 {
        view.push(CellModel::Exec {
            call: None,
            command: format!("step {i}"),
            status: crate::app::transcript::CallStatus::Succeeded,
            output: "ok".into(),
            exit: Some(0),
            duration_ms: Some(10),
            is_search: false,
            artifacts: Vec::new(),
        });
    }
    view.frame();
    view.scroll(ScrollIntent::Top);
    view.frame();
    view.scroll(ScrollIntent::LineDown(9));
    let rows = view.frame();
    let top = rows[0].clone();
    assert!(top.contains("step 4"), "{rows:#?}");
    view.app.details = true;
    let rows = view.frame();
    assert!(
        rows[0].contains("step 4"),
        "same cell on top in details: {rows:#?}"
    );
    assert!(
        rows[2].contains("exit 0"),
        "details show the outcome: {rows:#?}"
    );
    view.app.details = false;
    assert_eq!(view.frame()[0], top);
}

/// The top rows stay for context, since a pinned prompt may cover them.
#[test]
fn reveal_brings_a_cell_to_the_third_row() {
    let mut view = View::new();
    view.replies(1, 10);
    let target = view.push(CellModel::Assistant {
        text: "the one".into(),
    });
    view.replies(11, 30);
    view.frame();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    assert_eq!(rows[2], "• the one", "{rows:#?}");
}

/// A hit deep in a long tool output scrolls to its row, not the cell's top.
#[test]
fn reveal_shows_a_find_hit_below_the_viewport() {
    let mut view = View::new();
    view.replies(1, 3);
    let output = (1..=40)
        .map(|i| format!("ux-file-{i:02}.txt"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = view.push(CellModel::Exec {
        call: None,
        command: "ls".into(),
        status: crate::app::transcript::CallStatus::Succeeded,
        output,
        exit: Some(0),
        duration_ms: Some(10),
        is_search: false,
        artifacts: Vec::new(),
    });
    view.replies(4, 3);
    view.app.details = true;
    view.app.bottom.find = Some(FindBar {
        query: "ux-file-30".into(),
        matches: vec![target],
        current: Some(0),
        ..FindBar::default()
    });
    view.frame();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    assert!(
        rows[2].contains("ux-file-30.txt"),
        "the hit sits below the pinned rows: {rows:#?}"
    );

    view.app.bottom.find.as_mut().unwrap().query = "Ran ls".into();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    assert!(
        rows[2].contains("Ran ls"),
        "a hit near the top keeps the cell's top: {rows:#?}"
    );
}

/// The wrap splits `MATCH-PHRASE` over two rows: reveal still lands on it, both halves marked.
#[test]
fn reveal_shows_a_find_hit_the_wrap_split() {
    let mut view = View::new();
    let output = (1..=150)
        .map(|i| match i {
            101 => format!("{}MATCH-PHRASE tail", "a".repeat(50)),
            _ => format!("line {i}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let target = view.push(CellModel::Exec {
        call: None,
        command: "cat big".into(),
        status: crate::app::transcript::CallStatus::Succeeded,
        output,
        exit: Some(0),
        duration_ms: Some(10),
        is_search: false,
        artifacts: Vec::new(),
    });
    view.replies(1, 3);
    view.app.details = true;
    view.app.bottom.find = Some(FindBar {
        query: "match-phrase".into(),
        matches: vec![target],
        current: Some(0),
        ..FindBar::default()
    });
    view.frame();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    let start = format!("    {}MATCH-", "a".repeat(50));
    assert_eq!(
        rows[2], start,
        "the hit sits below the pinned rows: {rows:#?}"
    );
    assert_eq!(rows[3], "    PHRASE tail");

    let buffer = view.terminal.backend().buffer();
    let loud = Modifier::BOLD | Modifier::UNDERLINED;
    let marked = |x: u16, y: u16| buffer[(x, y)].style().add_modifier.contains(loud);
    assert!((54..60).all(|x| marked(x, 2)), "MATCH- is marked");
    assert!(!marked(53, 2) && !marked(3, 3));
    assert!((4..10).all(|x| marked(x, 3)), "PHRASE is marked");
    assert!(!marked(10, 3), "the rest of the row is not");
}

/// An exec cell running `command` that printed `output`.
fn ran(command: &str, output: String) -> CellModel {
    CellModel::Exec {
        call: None,
        command: command.into(),
        status: crate::app::transcript::CallStatus::Succeeded,
        output,
        exit: Some(0),
        duration_ms: Some(10),
        is_search: false,
        artifacts: Vec::new(),
    }
}

/// 150 output lines, `line i` except where `line` says otherwise.
fn output(line: impl Fn(usize) -> Option<String>) -> String {
    (1..=150)
        .map(|i| line(i).unwrap_or_else(|| format!("line {i}")))
        .collect::<Vec<_>>()
        .join("\n")
}

impl View {
    /// Find `query` with `cell` its only and current match.
    fn find(&mut self, query: &str, cell: CellId) {
        self.app.bottom.find = Some(FindBar {
            query: query.into(),
            matches: vec![cell],
            current: Some(0),
            ..FindBar::default()
        });
    }

    /// Whether the cell at `(x, y)` carries the find highlight.
    fn marked(&self, x: u16, y: u16) -> bool {
        let loud = Modifier::BOLD | Modifier::UNDERLINED;
        let buffer = self.terminal.backend().buffer();
        buffer[(x, y)].style().add_modifier.contains(loud)
    }
}

/// `match` and `phrase` end and start two output lines: no hit joins them.
#[test]
fn a_hard_break_never_joins_a_find_hit() {
    let mut view = View::new();
    let target = view.push(ran(
        "cat big",
        output(|i| match i {
            20 => Some("match".into()),
            21 => Some("phrase".into()),
            101 => Some("the match phrase".into()),
            _ => None,
        }),
    ));
    view.replies(1, 3);
    view.app.details = true;
    view.find("match phrase", target);
    view.frame();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    assert_eq!(rows[2], "    the match phrase", "the real hit: {rows:#?}");
    assert!((8..20).all(|x| view.marked(x, 2)));

    view.scroll(ScrollIntent::Top);
    view.scroll(ScrollIntent::LineDown(19));
    let rows = view.frame();
    let y = rows.iter().position(|r| r == "    match").expect("line 20") as u16;
    assert_eq!(rows[usize::from(y) + 1], "    phrase");
    assert!(!(0..W).any(|x| view.marked(x, y) || view.marked(x, y + 1)));
}

/// The command wraps `MATCH-` onto a `  │ ` continuation: both halves marked, the gutter not.
#[test]
fn a_hit_split_behind_a_continuation_gutter_is_marked() {
    let mut view = View::new();
    let command = format!("{}MATCH-PHRASE", "x".repeat(48));
    let target = view.push(ran(&command, "ok".into()));
    view.find("match-phrase", target);
    let rows = view.frame();
    let pipe = view.theme.glyphs().pipe;
    assert_eq!(rows[1], format!("  {pipe} PHRASE"), "{rows:#?}");
    assert!(rows[0].ends_with("xMATCH-"));
    assert!((54..60).all(|x| view.marked(x, 0)), "MATCH- is marked");
    assert!(!view.marked(53, 0));
    assert!((4..10).all(|x| view.marked(x, 1)), "PHRASE is marked");
    assert!(!(0..4).any(|x| view.marked(x, 1)), "the gutter is not");
}

/// An 80-column query the wrap spreads over three rows: revealed, every part marked.
#[test]
fn reveal_shows_a_find_hit_over_three_rows() {
    let mut view = View::new();
    let query = format!("START{}END", "-".repeat(72));
    let target = view.push(ran(
        "cat big",
        output(|i| (i == 101).then(|| format!("{}{query} tail", "a".repeat(50)))),
    ));
    view.replies(1, 3);
    view.app.details = true;
    view.find(&query, target);
    view.frame();
    view.scroll(ScrollIntent::Reveal(target));
    let rows = view.frame();
    assert_eq!(
        rows[2],
        format!("    {}START-", "a".repeat(50)),
        "{rows:#?}"
    );
    assert_eq!(rows[3], format!("    {}", "-".repeat(56)));
    assert_eq!(rows[4], format!("    {}END tail", "-".repeat(15)));
    assert!((54..60).all(|x| view.marked(x, 2)) && !view.marked(53, 2));
    assert!((4..60).all(|x| view.marked(x, 3)));
    assert!((4..22).all(|x| view.marked(x, 4)) && !view.marked(22, 4));
}

/// Rows of one cell: `(text, prefix, continues)` each.
fn laid(rows: &[(&str, usize, Option<usize>)]) -> ItemLines<'static> {
    let mut layout = Layout::default();
    for &(text, prefix, continues) in rows {
        layout.push(Line::from(text.to_owned()), RowSource { prefix, continues });
    }
    ItemLines::Cell(std::sync::Arc::new(layout))
}

#[test]
fn a_split_hit_joins_over_the_indent_and_a_dropped_space() {
    let query = "match phrase";
    let split = laid(&[("  the match", 2, None), ("  phrase here", 2, Some(1))]);
    assert_eq!(row_hits(&split, 0, query), [(6, 11)]);
    assert_eq!(row_hits(&split, 1, query), [(2, 8)]);
    let other = laid(&[("  the match", 2, None), ("  other", 2, Some(1))]);
    assert!(row_hits(&other, 0, query).is_empty());
    let unspaced = laid(&[("  the match", 2, None), ("  phrase here", 2, Some(0))]);
    assert!(
        row_hits(&unspaced, 0, query).is_empty(),
        "no space was dropped"
    );
    let hard = laid(&[("  the match", 2, None), ("  phrase here", 2, None)]);
    assert!(row_hits(&hard, 0, query).is_empty());
    let word = laid(&[("xMATCH-", 0, None), ("PHRASE", 0, Some(0))]);
    assert_eq!(row_hits(&word, 0, "match-phrase"), [(1, 7)]);
    let verb = laid(&[("• Ran ls", 6, None)]);
    assert_eq!(
        row_hits(&verb, 0, "ran ls"),
        [(4, 10)],
        "a line's lead stays searchable"
    );
}

#[test]
fn the_prompt_being_answered_pins_to_the_top_row() {
    let mut view = View::new();
    view.push(CellModel::User {
        from: UserFrom::Operator,
        text: "explain the flake".into(),
    });
    view.replies(1, 30);
    let rows = view.frame();
    assert_eq!(rows[0], "› explain the flake …");
    assert_eq!(rows[1], "", "a blank row parts it from the reply under it");
    view.scroll(ScrollIntent::Top);
    let rows = view.frame();
    assert_eq!(rows[0], "", "its own padding row, nothing pinned");
    assert_eq!(rows[1], "› explain the flake");
}

#[test]
fn find_accents_the_current_match_and_keeps_the_others_quiet() {
    let mut view = View::new();
    let first = view.push(CellModel::Assistant {
        text: "the WAL syncs".into(),
    });
    let second = view.push(CellModel::Assistant {
        text: "more wal work".into(),
    });
    view.app.bottom.find = Some(FindBar {
        query: "wal".into(),
        matches: vec![first, second],
        current: Some(1),
        ..FindBar::default()
    });
    view.frame();
    let buffer = view.terminal.backend().buffer();
    let (current_y, other_y) = (2, 0);
    let at = |x: u16, y: u16| buffer[(x, y)].style();
    let loud = Modifier::BOLD | Modifier::UNDERLINED;
    let accent = view.theme.accent().fg;
    // `• more wal work`: the hit starts at column 7.
    assert_eq!(at(7, current_y).fg, accent);
    assert!(at(7, current_y).add_modifier.contains(loud));
    assert!(!at(2, current_y).add_modifier.contains(loud));
    // `• the WAL syncs`: case folds; the hit starts at column 6 and stays quiet.
    assert_ne!(
        at(6, other_y),
        at(2, other_y),
        "other hits are still marked"
    );
    assert_ne!(at(6, other_y).fg, accent);
    assert!(!at(6, other_y).add_modifier.contains(Modifier::BOLD));
    assert_eq!(at(10, other_y), at(2, other_y));
}

#[test]
fn repeated_live_decoration_and_find_highlights_leave_cached_rows_unchanged() {
    let mut view = View::new();
    let cell = view.push(CellModel::Waiting {
        on: vec![(AgentId::new(), "researcher".into())],
        since: view.app.now,
        ended: None,
    });
    view.app.bottom.find = Some(FindBar {
        query: "researcher".into(),
        matches: vec![cell],
        current: Some(0),
        ..FindBar::default()
    });
    let first = view.frame();
    let transcript = &view.app.transcripts[&view.agent];
    let rows = rows_for(
        &view.app,
        view.agent,
        transcript,
        W,
        &mut view.cache.stream,
        &view.theme,
    );
    let key = rows.key(rows.cell(0).expect("waiting cell"));
    let cached = view.cache.layout.get(&key).expect("cached waiting rows");
    let undecorated = cached.lines().to_vec();
    assert_eq!(view.frame(), first, "elapsed suffix must not accumulate");
    let next = view.cache.layout.get(&key).expect("same cached rows");
    assert!(std::sync::Arc::ptr_eq(&cached, &next));
    assert_eq!(next.lines(), undecorated.as_slice());
    view.app.bottom.find = None;
    view.frame();
    assert_eq!(
        view.cache
            .layout
            .get(&key)
            .expect("unhighlighted cache")
            .lines(),
        undecorated.as_slice()
    );
}

#[test]
fn the_streaming_reply_follows_the_cells_with_a_caret() {
    let mut view = View::new();
    view.replies(1, 3);
    let now = view.app.now;
    view.transcript().stream(
        InvocationId::new(),
        &Delta::Text {
            text: "half a thou".into(),
        },
        now,
    );
    let rows = view.frame();
    let caret = view.theme.glyphs().caret;
    // Short transcripts sit at the top: replies on rows 0, 2, 4; the stream on 6.
    assert_eq!(rows[4], "• reply 3");
    assert_eq!(rows[6], format!("• half a thou{caret}"));
    assert!(rows[7..].iter().all(String::is_empty), "{rows:#?}");
}

/// Committing the reply must not move a word: the stream wraps where the cell will.
#[test]
fn a_streaming_reply_wraps_where_its_committed_cell_will() {
    use crate::app::transcript::StreamState;
    use crate::ui::cells::ChatCell;

    use crate::ui::theme::tokens::ThemeMode;

    let themes = ThemeMode::ALL
        .into_iter()
        .flat_map(|mode| {
            [false, true].map(|ascii| {
                Theme::resolve(ThemeSpec {
                    mode,
                    ascii,
                    ..ThemeSpec::default()
                })
            })
        })
        .chain(std::iter::once(crate::ui::cells::plain_theme()));
    let text = "Terminals descend from teletypes and early video displays, which drew \
        characters on a fixed grid. Each character occupied one identical cell, so \
        hardware could be simple.\n\nA fixed grid makes alignment trivial. Columns line \
        up vertically, so tables, indentation, and ASCII art stay intact.";
    let stream = StreamState {
        text: text.to_owned(),
        ..StreamState::default()
    };
    for theme in themes {
        let caret = theme.glyphs().caret;
        for width in (30..=90).chain([99, 100, 160]) {
            let mut cache = stream::StreamCache::default();
            let view = stream::stream_lines(AgentId::new(), &stream, width, &theme, 0, &mut cache)
                .expect("a streaming reply");
            let mut live: Vec<String> = (0..view.len())
                .map(|row| view.get(row).expect("a row").to_string())
                .collect();
            let last = live.last_mut().expect("a row");
            *last = last
                .strip_suffix(caret)
                .expect("the caret ends it")
                .to_owned();
            let committed: Vec<String> = CellModel::Assistant { text: text.into() }
                .display_lines(width, &theme)
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(live, committed, "at {width} columns");
        }
    }
}

#[test]
fn occurrences_fold_ascii_case_and_overlap() {
    assert_eq!(occurrences("É é", "é"), vec![(0, 2), (3, 5)]);
    assert_eq!(occurrences("İ i", "i"), vec![(0, 2), (3, 4)]);
    // A query folds as the text does: a final capital sigma still finds itself.
    assert_eq!(occurrences("ΟΔΟΣ οδοσ", "ΟΔΟΣ"), vec![(0, 8), (9, 17)]);
    assert_eq!(occurrences("KELVIN", "\u{212A}elvin"), vec![(0, 6)]);
    assert_eq!(occurrences("aaaa", "aa"), vec![(0, 2), (1, 3), (2, 4)]);
    assert_eq!(occurrences("Wal wAL", "wal"), vec![(0, 3), (4, 7)]);
    assert_eq!(occurrences("é wal", "wal"), vec![(3, 6)]);
    assert!(occurrences("short", "longer query").is_empty());
    assert_eq!(occurrences("abab-ABAB", "aba"), vec![(0, 3), (5, 8)]);
    assert_eq!(occurrences("aabaab", "aab"), vec![(0, 3), (3, 6)]);
    let long = "a".repeat(2_000);
    let hits = occurrences(&long, &"a".repeat(900));
    assert_eq!(
        (hits.len(), hits[0], hits[1100]),
        (1_101, (0, 900), (1_100, 2_000))
    );
}

/// However many spaces a wrap dropped, a row's search never rebuilds more
/// of the gap than a match overlapping it could reach.
#[test]
fn a_huge_dropped_gap_is_bounded_by_the_query() {
    let gap = 5_000_000;
    let split = laid(&[("ab", 0, None), ("ab", 0, Some(gap))]);
    assert_eq!(row_hits(&split, 0, "ab"), [(0, 2)]);
    assert_eq!(row_hits(&split, 1, "ab"), [(0, 2)]);
    assert!(row_hits(&split, 1, "b a").is_empty());
    let near = laid(&[("ab", 0, None), ("cd", 0, Some(2))]);
    assert_eq!(row_hits(&near, 1, "b  c"), [(0, 1)]);
    assert_eq!(row_hits(&near, 0, "b  c"), [(1, 2)]);
}

/// A user cell is a tinted block: pad row, prompt, pad row. Its empty pad rows
/// must carry the tint too, or the block reads as one row with two gaps.
#[test]
fn a_user_cell_tints_its_padding_rows_and_one_gap_follows() {
    let mut view = View::new();
    view.theme = crate::ui::bottom::status_line::tests::dark_theme(false);
    view.push(CellModel::User {
        from: UserFrom::Operator,
        text: "how does the build work?".into(),
    });
    view.push(CellModel::Assistant {
        text: "In three stages.".into(),
    });
    let rows = view.frame();
    let prompt = rows
        .iter()
        .position(|r| r.contains("how does the build work?"))
        .expect("the prompt row");
    assert_eq!(rows[prompt + 3].trim(), "• In three stages.", "{rows:#?}");

    let tint = view
        .theme
        .tint()
        .bg
        .expect("a truecolor theme tints user cells");
    let buffer = view.terminal.backend().buffer();
    let bg = |y: usize| -> Vec<_> { (0..W).map(|x| buffer[(x, y as u16)].bg).collect() };
    for y in [prompt - 1, prompt, prompt + 1] {
        assert!(
            bg(y).iter().all(|&c| c == tint),
            "row {y} is not tinted across"
        );
    }
    assert!(
        bg(prompt + 2).iter().all(|&c| c != tint),
        "one untinted gap row separates the cells"
    );
}

/// Codex keeps the session header atop history: the first message must not
/// push it away, and Ctrl+Home must find it again.
#[test]
fn a_root_transcript_opens_with_the_session_header() {
    use crate::testing::{app_with_transcript, render_with};
    let mut cache = ViewCache::default();
    let mut app = app_with_transcript(&["hello", "hi there"]);
    let text = render_with(&app, &mut cache, 80, 30);
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    assert!(first.contains("aether"), "header first:\n{text}");
    assert!(text.contains("› hello"), "{text}");

    let many: Vec<String> = (0..40).map(|i| format!("message {i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    app = app_with_transcript(&many);
    let mut cache = ViewCache::default();
    let text = render_with(&app, &mut cache, 80, 30);
    assert!(
        !text.contains("aether v"),
        "scrolled to the latest:\n{text}"
    );
    let agent = app.selected.expect("a viewed agent");
    app.scroll_log.push(agent, ScrollIntent::Top);
    let text = render_with(&app, &mut cache, 80, 30);
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    assert!(
        first.contains("aether"),
        "Ctrl+Home reaches the header:\n{text}"
    );
}

#[test]
fn following_and_reading_can_reach_beyond_65535_rows_in_one_cell() {
    use crate::testing::render_with;
    let text = format!(
        "{}\nTHE-REAL-END",
        (0..33000).map(|n| format!("L{n}\n\n")).collect::<String>()
    );
    let mut app = crate::testing::app_with_transcript(&["show a long log", &text]);
    app.board.open = false;
    app.details = true;
    let agent = app.selected.expect("agent");
    let mut cache = ViewCache::default();
    let screen = render_with(&app, &mut cache, 80, 20);
    assert!(screen.contains("THE-REAL-END"), "{screen}");
    app.scroll_log.push(agent, ScrollIntent::LineUp(1));
    render_with(&app, &mut cache, 80, 20);
    assert!(matches!(cache.transcript.anchor(agent), Anchor::Reading { row, .. } if row > 65535));
    app.scroll_log.push(agent, ScrollIntent::Latest);
    let screen = render_with(&app, &mut cache, 80, 20);
    assert!(screen.contains("THE-REAL-END"), "{screen}");
}

#[test]
fn only_current_occurrence_is_emphasized_in_every_theme() {
    for builtin in crate::ui::theme::Builtin::ALL {
        for mode in crate::ui::theme::tokens::ThemeMode::ALL {
            let mut view = View::new();
            view.theme = Theme::resolve(ThemeSpec {
                scheme: builtin.scheme(),
                mode,
                ..Default::default()
            });
            let cell = view.push(CellModel::Assistant {
                text: "match match".into(),
            });
            view.find("match", cell);
            view.app.bottom.find.as_mut().unwrap().matches = vec![cell, cell];
            let rows = view.frame();
            let y = rows
                .iter()
                .position(|row| row.contains("match match"))
                .unwrap() as u16;
            assert!(view.marked(2, y));
            assert!(!view.marked(8, y), "only the first occurrence is current");
            view.app.bottom.find.as_mut().unwrap().current = Some(1);
            view.frame();
            assert!(!view.marked(2, y));
            assert!(view.marked(8, y));
        }
    }
}

#[test]
fn next_occurrence_below_view_is_revealed() {
    let mut view = View::new();
    let cell = view.push(ran(
        "cat output",
        output(|i| [1, 100].contains(&i).then(|| "match".into())),
    ));
    view.app.details = true;
    view.find("match", cell);
    let bar = view.app.bottom.find.as_mut().unwrap();
    bar.matches = vec![cell, cell];
    bar.current = Some(1);
    view.scroll(ScrollIntent::Reveal(cell));
    let rows = view.frame();
    assert!(rows[2].contains("match"), "{rows:?}");
    assert!(rows[3].contains("line 101"), "{rows:?}");
}

#[test]
fn the_current_hit_is_mapped_once_not_per_frame() {
    let mut view = View::new();
    let cell = view.push(ran(
        "cat output",
        output(|i| (i % 2 == 0).then(|| "match".into())),
    ));
    view.app.details = true;
    view.find("match", cell);
    let bar = view.app.bottom.find.as_mut().unwrap();
    bar.matches = vec![cell; 75];
    bar.current = Some(74);
    view.scroll(ScrollIntent::Reveal(cell));
    for _ in 0..3 {
        view.frame();
    }
    let marked = (0..H).any(|y| (0..W).any(|x| view.marked(x, y)));
    assert!(marked, "the current hit shows");
    assert_eq!(view.cache.find_hit.maps, 1);
    view.app.bottom.find.as_mut().unwrap().current = Some(73);
    view.scroll(ScrollIntent::Reveal(cell));
    view.frame();
    assert_eq!(view.cache.find_hit.maps, 2, "a new occurrence maps again");
}

#[test]
fn upward_scroll_requests_older_history_only_when_already_at_the_top() {
    for intent in [
        ScrollIntent::Top,
        ScrollIntent::PageUp,
        ScrollIntent::LineUp(1),
    ] {
        let mut view = View::new();
        view.replies(1, 30);
        view.frame();
        assert!(view.cache.transcript.older_requests.is_empty());
        if intent != ScrollIntent::Top {
            view.scroll(intent);
            view.frame();
            assert!(view.cache.transcript.older_requests.is_empty());
        }
        let top = view.app.transcripts[&view.agent].cells[0].id;
        for _ in 0..100 {
            view.scroll(intent);
            view.frame();
            assert!(view.cache.transcript.older_requests.is_empty());
            if view.anchor() == (Anchor::Reading { cell: top, row: 0 }) {
                break;
            }
        }
        assert_eq!(view.anchor(), Anchor::Reading { cell: top, row: 0 });
        view.scroll(intent);
        view.frame();
        assert_eq!(view.cache.transcript.older_requests, [view.agent]);
        view.cache.transcript.older_requests.clear();
        view.frame();
        assert!(
            view.cache.transcript.older_requests.is_empty(),
            "drawing alone must not auto-load all history"
        );
    }
}

#[test]
fn scroll_to_top_loads_the_evicted_window_through_the_reducer() {
    use crate::app::{AppEvent, Effect, Then};
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::EventId;
    use aether_core::model::{ContentBlock, MessageRole};
    use aether_protocol::{Command, Reply};

    let mut view = View::new();
    let agent = view.agent;
    let event = |seq| {
        Event::from_draft(
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
            now(),
        )
    };
    for seq in 1..=5_001 {
        view.app
            .reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
                seq,
            )))));
    }
    view.frame();
    view.scroll(ScrollIntent::Top);
    view.frame();
    assert!(view.cache.transcript.older_requests.is_empty());
    view.scroll(ScrollIntent::PageUp);
    view.frame();
    let requested = view.cache.transcript.older_requests.pop().unwrap();
    let Effect::Request {
        command: Command::GetEvents { query, .. },
        then,
    } = view.app.older_history_request(requested).unwrap()
    else {
        panic!("history request")
    };
    assert_eq!(query.before, Some(2_502));
    assert!(matches!(then, Then::Backfill { agent: id, .. } if id == agent));
    view.app.reduce(AppEvent::Reply {
        then,
        result: Ok(Reply::Events(
            (1..=2_502)
                .rev()
                .filter(|seq| *seq < query.before.unwrap())
                .map(event)
                .collect(),
        )),
    });
    let screen = view.frame().join("\n");
    assert!(screen.contains("reply 2501"), "{screen}");
    assert!(!screen.contains("reply 5001"), "{screen}");
}

#[test]
fn eviction_preserves_reading_anchor_find_matches_and_selection_endpoints() {
    use crate::app::AppEvent;
    use crate::ui::selection::TextPoint;
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::EventId;
    use aether_core::model::{ContentBlock, MessageRole};

    let mut view = View::new();
    let agent = view.agent;
    let event = |seq| {
        AppEvent::Frame(SubscriptionFrame::Event(Box::new(Event::from_draft(
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
            now(),
        ))))
    };
    for seq in 1..=5_000 {
        view.app.reduce(event(seq));
    }
    view.frame();
    for _ in 0..20 {
        view.scroll(ScrollIntent::PageUp);
        view.frame();
    }
    let anchor = view.anchor();
    let Anchor::Reading { cell, row } = anchor else {
        panic!("reading anchor")
    };
    let point = TextPoint {
        cell,
        row,
        column: 0,
        stream_generation: None,
    };
    let screen = view.frame();
    let mut find = FindBar {
        query: "reply 4885".into(),
        ..FindBar::default()
    };
    crate::ui::bottom::find::refresh(&mut find, &view.app);
    let matches = find.matches.clone();
    assert_eq!(matches.len(), 1);
    let epoch = view.app.transcripts[&agent].epoch;
    view.app.reduce(event(5_001));
    assert_eq!(view.frame(), screen);
    assert_eq!(view.anchor(), anchor);
    assert_eq!(view.app.transcripts[&agent].epoch, epoch);
    assert!(selection_order(&view.app, point).is_some());
    crate::ui::bottom::find::refresh(&mut find, &view.app);
    assert_eq!(find.matches, matches);
}

#[test]
fn an_evicted_reading_anchor_moves_to_the_oldest_retained_content() {
    use crate::app::AppEvent;
    use aether_client::SubscriptionFrame;
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::EventId;
    use aether_core::model::{ContentBlock, MessageRole};

    let mut view = View::new();
    let agent = view.agent;
    let event = |seq| {
        AppEvent::Frame(SubscriptionFrame::Event(Box::new(Event::from_draft(
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
            now(),
        ))))
    };
    for seq in 1..=5_000 {
        view.app.reduce(event(seq));
    }
    view.frame();
    view.scroll(ScrollIntent::Top);
    assert!(view.frame().join("\n").contains("reply 1"));
    assert_eq!(
        view.anchor(),
        Anchor::Reading {
            cell: CellId(1),
            row: 0
        }
    );
    view.app.reduce(event(5_001));
    let screen = view.frame().join("\n");
    assert!(screen.contains("reply 2502"), "{screen}");
    assert!(!screen.contains("reply 5001"), "{screen}");
    assert_eq!(
        view.anchor(),
        Anchor::Reading {
            cell: CellId(2_502),
            row: 0
        }
    );
    assert!(view.cache.transcript.older_requests.is_empty());
    view.scroll(ScrollIntent::PageUp);
    view.frame();
    assert_eq!(view.cache.transcript.older_requests, [agent]);
}

#[test]
fn a_rebuilt_window_returns_old_reading_anchors_to_latest() {
    for count in [3, 100] {
        let mut view = View::new();
        view.replies(1, 100);
        view.frame();
        view.scroll(ScrollIntent::PageUp);
        view.frame();
        assert!(matches!(view.anchor(), Anchor::Reading { .. }));
        let fresh = view.transcript().continuing();
        view.app.transcripts.insert(view.agent, fresh);
        for i in 1..=count {
            view.push(CellModel::Assistant {
                text: format!("fresh {i}"),
            });
        }
        let screen = view.frame().join("\n");
        assert!(screen.contains(&format!("fresh {count}")), "{screen}");
        if count == 100 {
            assert!(screen.contains("fresh 95"), "{screen}");
            assert!(!screen.lines().any(|line| line == "• fresh 1"), "{screen}");
        }
        assert_eq!(view.anchor(), Anchor::Latest);
        for i in 1..=20 {
            view.push(CellModel::Assistant {
                text: format!("new {i}"),
            });
        }
        let screen = view.frame().join("\n");
        assert!(screen.contains("new 20"), "{screen}");
        assert_eq!(view.anchor(), Anchor::Latest);
    }
}
