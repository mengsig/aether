#![cfg(test)]

//! Cell snapshots (SPEC §8 Phase 3): every family at 80 and 120 columns,
//! compact and detail, unicode and ascii, held against `snapshots/*.txt`.
//! `AETHER_UPDATE_SNAPSHOTS=1 cargo test -p aether-tui` rewrites them.

use std::path::PathBuf;

use aether_core::id::{AgentId, ApprovalId, ToolCallId};
use aether_core::model::Effort;
use aether_core::time::Timestamp;
use chrono::{DateTime, TimeDelta};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Style;
use ratatui::text::Line;

use super::*;
use crate::app::transcript::{
    ApprovalScope, CallTarget, ChildOutcome, Cost, DescendantSummary, DiffLine, DiffSign,
    ExploreItem, ExploreVerb, FileChange, FileEdit, MemoryNote, NoticeKind, NoticeSeparator,
    NoticeText, UserFrom,
};

fn at(secs: i64) -> Timestamp {
    DateTime::from_timestamp(1_790_000_000 + secs, 0).expect("a valid time")
}

fn usd(usd: f64) -> Option<Cost> {
    Some(Cost {
        usd,
        estimated: false,
        basis: Default::default(),
    })
}

fn numbered(prefix: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{prefix} {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn diff(rows: &[(Option<u32>, DiffSign, &str)]) -> Vec<DiffLine> {
    rows.iter()
        .map(|(line_no, sign, text)| DiffLine {
            line_no: *line_no,
            sign: *sign,
            text: (*text).to_owned(),
        })
        .collect()
}

/// One cell per family and state; every name is a snapshot file.
fn fixtures() -> Vec<(&'static str, CellModel)> {
    let tester = AgentId::new();
    let researcher = AgentId::new();
    vec![
        (
            "user_operator",
            CellModel::User {
                from: UserFrom::Operator,
                text: "Fix the flaky shell_bg test and explain the root cause in `store.rs` before touching anything else in the crate.".into(),
            },
        ),
        (
            "user_task",
            CellModel::User {
                from: UserFrom::Task { from: "aether".into() },
                text: "Map the WAL sync path".into(),
            },
        ),
        (
            "assistant",
            CellModel::Assistant {
                text: "The flake comes from **WAL syncs** on the request path.\n\n- `fsync` blocks the writer\n- readers wait on the lock\n\nMoving the sync to a background task fixes both, see [the PR](https://example.com/pr/283).".into(),
            },
        ),
        (
            "reasoning",
            CellModel::Reasoning {
                title: Some("Tracing the sync".into()),
                text: "The writer holds the lock across `fsync`, so every reader stalls behind it.".into(),
                duration_ms: Some(12_400),
            },
        ),
        (
            "exec_ok",
            CellModel::Exec {
                call: Some(ToolCallId::new()),
                command: "cargo test -p aether-store wal".into(),
                status: CallStatus::Succeeded,
                output: numbered("test wal::case", 9),
                exit: Some(0),
                duration_ms: Some(1_240),
                is_search: false,
                artifacts: Vec::new(),
            },
        ),
        (
            "exec_failed",
            CellModel::Exec {
                call: Some(ToolCallId::new()),
                command: "cargo build".into(),
                status: CallStatus::Failed,
                output: "error[E0308]: mismatched types\nerror: could not compile `aether-store`".into(),
                exit: Some(101),
                duration_ms: Some(8_300),
                is_search: false,
                artifacts: Vec::new(),
            },
        ),
        (
            "exec_search_no_match",
            CellModel::Exec {
                call: None,
                command: "rg -n fsync_all crates/aether-store".into(),
                status: CallStatus::Succeeded,
                output: String::new(),
                exit: Some(1),
                duration_ms: Some(40),
                is_search: true,
                artifacts: Vec::new(),
            },
        ),
        (
            "exec_running",
            CellModel::Exec {
                call: None,
                command: "sleep 5 && echo done".into(),
                status: CallStatus::Running,
                output: String::new(),
                exit: None,
                duration_ms: None,
                is_search: false,
                artifacts: Vec::new(),
            },
        ),
        (
            "explored",
            CellModel::Explored {
                items: vec![
                    ExploreItem {
                        verb: ExploreVerb::Read,
                        targets: vec!["store.rs".into(), "wal.rs".into(), "lib.rs".into()],
                        scope: None,
                    },
                    ExploreItem {
                        verb: ExploreVerb::Search,
                        targets: vec!["sync_data".into()],
                        scope: Some("crates/aether-store".into()),
                    },
                    ExploreItem {
                        verb: ExploreVerb::List,
                        targets: vec!["crates/aether-store/src".into()],
                        scope: None,
                    },
                ],
                active: false,
            },
        ),
        (
            "explored_active",
            CellModel::Explored {
                items: vec![ExploreItem {
                    verb: ExploreVerb::Read,
                    targets: vec!["README.md".into()],
                    scope: None,
                }],
                active: true,
            },
        ),
        (
            "edited_one",
            CellModel::Edited {
                files: vec![FileEdit {
                    path: "crates/aether-store/src/wal.rs".into(),
                    change: FileChange::Edited,
                    added: 2,
                    removed: 1,
                    diff: diff(&[
                        (Some(41), DiffSign::Context, "fn commit(&mut self) {"),
                        (Some(42), DiffSign::Remove, "    self.file.sync_all()?;"),
                        (Some(42), DiffSign::Add, "    self.pending = true;"),
                        (Some(43), DiffSign::Add, "    self.notify.notify_one();"),
                        (Some(44), DiffSign::Context, "}"),
                    ]),
                }],
                status: CallStatus::Succeeded,
            },
        ),
        (
            "edited_many",
            CellModel::Edited {
                files: vec![
                    FileEdit {
                        path: "src/wal.rs".into(),
                        change: FileChange::Edited,
                        added: 1,
                        removed: 1,
                        diff: diff(&[
                            (Some(7), DiffSign::Remove, "old();"),
                            (Some(7), DiffSign::Add, "new();"),
                        ]),
                    },
                    FileEdit {
                        path: "src/sync.rs".into(),
                        change: FileChange::Added,
                        added: 1,
                        removed: 0,
                        diff: diff(&[(Some(1), DiffSign::Add, "pub mod sync;")]),
                    },
                ],
                status: CallStatus::Succeeded,
            },
        ),
        (
            "called_mcp",
            CellModel::Called {
                target: CallTarget::Mcp {
                    server: "github".into(),
                    tool: "search_issues".into(),
                },
                args: "{\"q\":\"wal sync\"}".into(),
                result: numbered("issue", 7),
                status: CallStatus::Succeeded,
                error: None,
                artifacts: Vec::new(),
            },
        ),
        (
            "called_failed",
            CellModel::Called {
                target: CallTarget::Tool { name: "spawn_agent".into() },
                args: String::new(),
                result: String::new(),
                status: CallStatus::Failed,
                error: Some("budget exhausted for this subtree".into()),
                artifacts: Vec::new(),
            },
        ),
        (
            "web_fetch_awaiting",
            CellModel::Called {
                target: CallTarget::WebFetch {
                    url: "https://example.com/changelog".into(),
                },
                args: String::new(),
                result: String::new(),
                status: CallStatus::Awaiting,
                error: None,
                artifacts: Vec::new(),
            },
        ),
        (
            "exec_awaiting",
            CellModel::Exec {
                call: None,
                command: "git push origin main".into(),
                status: CallStatus::Awaiting,
                output: String::new(),
                exit: None,
                duration_ms: None,
                is_search: false,
                artifacts: Vec::new(),
            },
        ),
        (
            "web_search",
            CellModel::Called {
                target: CallTarget::WebSearch {
                    query: "linux fdatasync vs fsync latency".into(),
                },
                args: String::new(),
                result: "3 results".into(),
                status: CallStatus::Succeeded,
                error: None,
                artifacts: Vec::new(),
            },
        ),
        (
            "web_fetch",
            CellModel::Called {
                target: CallTarget::WebFetch {
                    url: "https://lwn.net/Articles/457667/".into(),
                },
                args: String::new(),
                result: String::new(),
                status: CallStatus::Succeeded,
                error: None,
                artifacts: Vec::new(),
            },
        ),
        (
            "memory_recalled",
            CellModel::Memory(MemoryNote::Recalled {
                names: vec!["no-pkill".into(), "wal-design".into()],
            }),
        ),
        (
            "memory_remembered",
            CellModel::Memory(MemoryNote::Remembered {
                text: "WAL syncs stay off the request path".into(),
            }),
        ),
        (
            "spawned",
            CellModel::Spawned {
                agent: researcher,
                name: "researcher".into(),
                role: "explorer".into(),
                model: "sonnet-5.5".into(),
                effort: Some(Effort::Medium),
                objective: "Find every caller of `sync_all` in the store and report which ones run on the request path, with file and line for each.".into(),
            },
        ),
        (
            "waiting",
            CellModel::Waiting {
                on: vec![(researcher, "researcher".into()), (tester, "tester".into())],
                since: at(0),
                ended: None,
            },
        ),
        (
            "waited",
            CellModel::Waiting {
                on: vec![(researcher, "researcher".into())],
                since: at(0),
                ended: Some(at(291)),
            },
        ),
        (
            "child_finished",
            CellModel::ChildResult {
                agent: researcher,
                name: "researcher".into(),
                role: "explorer".into(),
                outcome: ChildOutcome::Finished,
                elapsed_ms: 63_000,
                cost_with_children: usd(0.21),
                result: "Three callers sync on the request path:\nstore.rs:88 commit\nstore.rs:140 flush\nwal.rs:42 append\nThe rest run in the compactor.".into(),
                descendants: vec![DescendantSummary {
                    name: "tester".into(),
                    outcome: ChildOutcome::Finished,
                    elapsed_ms: 80_000,
                    cost: usd(0.09),
                }],
            },
        ),
        (
            "child_failed_quota",
            CellModel::ChildResult {
                agent: tester,
                name: "tester".into(),
                role: "worker".into(),
                outcome: ChildOutcome::Failed {
                    error: "provider rejected the request: weekly limit reached".into(),
                    quota: Some("anthropic weekly limit".into()),
                },
                elapsed_ms: 12_000,
                cost_with_children: None,
                result: String::new(),
                descendants: Vec::new(),
            },
        ),
        (
            "sent",
            CellModel::Sent {
                to: "researcher, tester".into(),
                text: "Stop after the store crate; the net crate is out of scope for this pass.".into(),
            },
        ),
        (
            "approval_record",
            CellModel::ApprovalRecord {
                approval: ApprovalId::new(),
                approved: true,
                what: "bash `cargo test`".into(),
                scope: ApprovalScope::Agent { name: "aether".into() },
            },
        ),
        (
            "question_open",
            CellModel::Question {
                asker: researcher,
                asker_name: "researcher".into(),
                text: "Should the compactor keep its own fsync?".into(),
                state: crate::app::transcript::QuestionState::Pending,
                seq: 9,
            },
        ),
        (
            "question_answered",
            CellModel::Question {
                asker: researcher,
                asker_name: "researcher".into(),
                text: "Should the compactor keep its own fsync?".into(),
                state: crate::app::transcript::QuestionState::Answered,
                seq: 9,
            },
        ),
        (
            "error",
            CellModel::Error {
                message: "Turn failed".into(),
                cause: Some("the engine exited with status 1 before producing a reply".into()),
                hint: Some("/restart aether".into()),
            },
        ),
        (
            "notice_backoff",
            CellModel::Notice {
                kind: NoticeKind::Backoff {
                    incident: "anthropic".into(),
                },
                text: NoticeText::joined(&["Anthropic busy", "retrying in 8s"], NoticeSeparator::Dot),
            },
        ),
        (
            "compaction_done",
            CellModel::Compaction(CompactionState::Done {
                before: Some(182_000),
                after: Some(31_000),
            }),
        ),
        (
            "compaction_active",
            CellModel::Compaction(CompactionState::Active),
        ),
        (
            "notice_trigger",
            CellModel::Notice {
                kind: NoticeKind::Trigger { schedule: None },
                text: NoticeText::joined(&[
                    "Trigger from github",
                    &format!(r#"{{"kind":"issue","number":42,"text":"{}"}}"#, "the build is red again ".repeat(6).trim_end()),
                ], NoticeSeparator::Dot),
            },
        ),
        (
            "notice_note",
            CellModel::Notice {
                kind: NoticeKind::Note(aether_core::note::NoteLevel::Info),
                text: NoticeText::joined(&[
                    &format!("3 items shipped: {}", "the release notes are ready for review ".repeat(3).trim_end()),
                    "https://example.com/pull/12",
                ], NoticeSeparator::Dot),
            },
        ),
        (
            "notice_note_applied",
            CellModel::Notice {
                kind: NoticeKind::Note(aether_core::note::NoteLevel::Applied),
                text: NoticeText::joined(&["/answer 3 yes: answered question 3"], NoticeSeparator::Dot),
            },
        ),
        (
            "notice_note_refused",
            CellModel::Notice {
                kind: NoticeKind::Note(aether_core::note::NoteLevel::Refused),
                text: NoticeText::joined(&["/answer 9 yes: no open question 9"], NoticeSeparator::Dot),
            },
        ),
        (
            "exec_engine_wrapped",
            CellModel::Exec {
                call: Some(ToolCallId::new()),
                command: r#"/usr/bin/zsh -lc 'python3 -c "print(17*23)"'"#.into(),
                status: CallStatus::Succeeded,
                output: "391".into(),
                exit: Some(0),
                duration_ms: Some(80),
                is_search: false,
                artifacts: Vec::new(),
            },
        ),
    ]
}

fn unicode() -> Theme {
    Theme::resolve(ThemeSpec::default())
}

fn ascii() -> Theme {
    Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    })
}

/// The screen a person reads: lines drawn through a `TestBackend`, trailing
/// blanks trimmed.
fn screen(lines: &[Line<'static>], width: u16) -> String {
    let height = u16::try_from(lines.len()).expect("a short cell").max(1);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| {
            for (y, line) in lines.iter().enumerate() {
                let row = ratatui::layout::Rect::new(0, y as u16, width, 1);
                frame.render_widget(line, row);
            }
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(usize::from(width))
        .take(lines.len())
        .map(|row| {
            let text: String = row.iter().map(|c| c.symbol()).collect();
            text.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn snapshot(cell: &CellModel) -> String {
    let mut out = String::new();
    for (label, theme) in [("unicode", unicode()), ("ascii", ascii())] {
        for width in [80, 120] {
            for detail in [false, true] {
                let lines = if detail {
                    cell.detail_lines(width, &theme)
                } else {
                    cell.display_lines(width, &theme)
                };
                let mode = if detail { "detail" } else { "compact" };
                out.push_str(&format!("── {label} {width} {mode}\n"));
                out.push_str(&screen(&lines, width));
                out.push('\n');
                if lines.iter().any(|l| l.width() > usize::from(width)) {
                    out.push_str("!! a line overflows its width\n");
                }
            }
        }
    }
    out
}

#[test]
fn every_family_matches_its_snapshot() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ui/cells/snapshots");
    let update = std::env::var_os("AETHER_UPDATE_SNAPSHOTS").is_some();
    let mut stale = Vec::new();
    for (name, cell) in fixtures() {
        let path = dir.join(format!("{name}.txt"));
        let got = snapshot(&cell);
        if update {
            std::fs::create_dir_all(&dir).expect("the snapshot dir");
            std::fs::write(&path, &got).expect("a snapshot write");
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        if got != want {
            stale.push(format!("{name}:\n{got}"));
        }
    }
    assert!(stale.is_empty(), "snapshots differ:\n{}", stale.join("\n"));
}

#[test]
fn plain_lines_are_the_display_text_without_styles() {
    for (name, cell) in fixtures() {
        for width in [40, 80, 120] {
            let shown = screen(&cell.display_lines(width, &plain_theme()), width);
            let shown = shown.replace(&format!("{}\n", text::DETAILS_HINT), "\n");
            let plain = cell.plain_lines(width).join("\n");
            assert_eq!(
                plain,
                shown.trim_end_matches(text::DETAILS_HINT),
                "{name} at {width}"
            );
        }
    }
}

/// Plain output has no details view, so a cut cell names what it hid but no key.
#[test]
fn plain_lines_name_hidden_rows_without_the_details_key() {
    let (_, cell) = fixtures()
        .into_iter()
        .find(|(name, _)| *name == "exec_ok")
        .expect("the exec_ok fixture");
    let plain = cell.plain_lines(80).join("\n");
    assert!(plain.contains("+4 lines"), "{plain}");
    assert!(!plain.contains("ctrl+t"), "{plain}");
}

#[test]
fn no_line_is_wider_than_the_viewport() {
    let mut wide = Vec::new();
    for (name, cell) in fixtures() {
        for theme in [unicode(), ascii()] {
            for width in [12, 24, 37, 80] {
                for lines in [
                    cell.display_lines(width, &theme),
                    cell.detail_lines(width, &theme),
                ] {
                    for line in &lines {
                        if line.width() > usize::from(width) {
                            wide.push(format!("{name} at {width}: {line}"));
                        }
                    }
                }
            }
        }
    }
    assert!(wide.is_empty(), "rows overflow:\n{}", wide.join("\n"));
}

#[test]
fn the_separator_reads_time_cost_and_agents_in_local_time() {
    let finished_at = at(0);
    let cell = CellModel::Separator {
        elapsed_ms: 374_000,
        cost: usd(1.84),
        agents: 5,
        finished_at,
    };
    let clock = finished_at.with_timezone(&chrono::Local).format("%H:%M");
    let row = cell.plain_lines(60).join("\n");
    let lead = format!("── Worked for 6m 14s • $1.84 • 5 agents • {clock} ");
    assert!(row.starts_with(&lead), "{row}");
    assert_eq!(row.chars().count(), 60, "the rule fills the row: {row}");
    let one = CellModel::Separator {
        elapsed_ms: 41_000,
        cost: None,
        agents: 1,
        finished_at,
    };
    let row = one.plain_lines(60).join("\n");
    assert!(
        row.starts_with(&format!("── Worked for 41s • {clock} ")),
        "{row}"
    );
    let ascii_row: String = one
        .display_lines(60, &ascii())
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert!(ascii_row.is_ascii(), "{ascii_row}");
}

fn text_of(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ascii_notices_draw_their_separators_from_the_glyph_set() {
    let notice = |kind, text: NoticeText| CellModel::Notice { kind, text };
    let cells = [
        notice(
            NoticeKind::ModelChange,
            NoticeText::joined(
                &["Model", "gpt-6-luna low from the next turn"],
                NoticeSeparator::Arrow,
            ),
        ),
        notice(
            NoticeKind::Handoff,
            NoticeText::joined(
                &["Continued in a fresh context", "summary carried over"],
                NoticeSeparator::Dot,
            ),
        ),
        CellModel::Error {
            message: "Turn failed".into(),
            cause: None,
            hint: Some(NoticeText::joined(
                &["/wake aether", "/restart aether"],
                NoticeSeparator::Dot,
            )),
        },
    ];
    for cell in &cells {
        let drawn = text_of(&cell.display_lines(80, &ascii()));
        assert!(drawn.is_ascii(), "{drawn}");
    }
    assert!(text_of(&cells[0].display_lines(80, &ascii())).contains("Model -> gpt-6-luna"));
    assert!(text_of(&cells[1].display_lines(80, &ascii())).contains("context | summary"));
    assert!(text_of(&cells[0].display_lines(80, &unicode())).contains("Model → gpt-6-luna"));
}

#[test]
fn ascii_notices_preserve_external_text() {
    for kind in [
        NoticeKind::Trigger { schedule: None },
        NoticeKind::BudgetWarning,
        NoticeKind::Engine,
    ] {
        let cell = CellModel::Notice {
            kind,
            text: "literal a → b · c".into(),
        };
        assert_eq!(
            text_of(&cell.detail_lines(160, &ascii())),
            "* literal a → b · c"
        );
    }
    let cell = CellModel::Error {
        message: "error".into(),
        cause: None,
        hint: Some("literal a → b · c".into()),
    };
    assert!(text_of(&cell.detail_lines(160, &ascii())).contains("literal a → b · c"));
}

#[test]
fn authored_separators_preserve_payload_glyphs_after_name_substitution() {
    let id = AgentId::new();
    let cell = CellModel::Notice {
        kind: NoticeKind::Engine,
        text: NoticeText::joined(
            &[&format!("{id} interrupted"), "literal a → b · c"],
            NoticeSeparator::Dot,
        ),
    };
    let named = cell
        .with_names(|value| (value == id.to_string()).then(|| "worker".into()))
        .unwrap();
    for width in [59, 60, 99, 100, 160] {
        assert_eq!(
            text_of(&named.detail_lines(width, &ascii())),
            "* worker interrupted | literal a → b · c"
        );
        assert_eq!(
            text_of(&named.detail_lines(width, &unicode())),
            "• worker interrupted · literal a → b · c"
        );
        assert_eq!(
            named.plain_lines(width),
            ["• worker interrupted · literal a → b · c"]
        );
    }
}

#[test]
fn a_compaction_with_an_unknown_side_names_no_count() {
    let cell = CellModel::Compaction(CompactionState::Done {
        before: Some(971),
        after: None,
    });
    assert_eq!(cell.plain_lines(80), ["• Compacted context"]);
    let known = CellModel::Compaction(CompactionState::Done {
        before: Some(182_000),
        after: Some(31_000),
    });
    let row = known.plain_lines(80).join("\n");
    assert!(
        row.contains("182") && row.contains("31") && row.ends_with("tokens"),
        "{row}"
    );
}

#[test]
fn a_trigger_takes_one_row_and_details_show_it_whole() {
    let cell = fixtures()
        .into_iter()
        .find_map(|(name, cell)| (name == "notice_trigger").then_some(cell))
        .expect("the notice_trigger fixture");
    let compact = cell.display_lines(60, &unicode());
    assert_eq!(compact.len(), 1, "{}", text_of(&compact));
    assert!(text_of(&compact).starts_with("• Trigger from github · {"));
    assert!(text_of(&compact).ends_with('…'), "{}", text_of(&compact));
    let detail = text_of(&cell.detail_lines(60, &unicode()));
    assert!(detail.lines().count() > 1);
    assert!(detail.contains("again\"}"), "{detail}");
}

#[test]
fn an_engine_shell_wrapper_shows_only_the_command() {
    let cell = fixtures()
        .into_iter()
        .find_map(|(name, cell)| (name == "exec_engine_wrapped").then_some(cell))
        .expect("the exec_engine_wrapped fixture");
    let compact = text_of(&cell.display_lines(80, &unicode()));
    assert!(
        compact.starts_with(r#"• Ran python3 -c "print(17*23)""#),
        "{compact}"
    );
    let detail = text_of(&cell.detail_lines(80, &unicode()));
    assert!(
        detail.contains(r#"/usr/bin/zsh -lc 'python3 -c "print(17*23)"'"#),
        "{detail}"
    );
}

/// A turn under a second must not read as if it took no time at all.
#[test]
fn a_sub_second_turn_reads_less_than_a_second() {
    let quick = CellModel::Separator {
        elapsed_ms: 400,
        cost: None,
        agents: 1,
        finished_at: at(0),
    };
    let row = quick.plain_lines(60).join("\n");
    assert!(row.starts_with("── Worked for <1s •"), "{row}");
}

/// Style of the first span whose text contains `needle`.
fn style_of(lines: &[Line<'static>], needle: &str) -> Style {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains(needle))
        .unwrap_or_else(|| panic!("no span holds {needle:?}"))
        .style
}

#[test]
fn styles_come_from_theme_tokens() {
    let theme = unicode();
    let cells: Vec<_> = fixtures().into_iter().collect();
    let get = |name: &str| {
        &cells
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no fixture {name}"))
            .1
    };
    let user = get("user_operator").display_lines(80, &theme);
    assert!(
        user.iter().all(|l| l.style == theme.tint()),
        "user rows are tinted"
    );
    let prompt = style_of(&user, theme.glyphs().prompt);
    assert_eq!(prompt, theme.user().patch(theme.title()));

    let failed = get("exec_failed").display_lines(80, &theme);
    assert_eq!(style_of(&failed, "(exit 101)"), theme.failure());
    let no_match = get("exec_search_no_match").display_lines(80, &theme);
    assert_eq!(style_of(&no_match, "(no output)"), theme.secondary());

    let child = get("child_finished").display_lines(80, &theme);
    assert_eq!(
        style_of(&child, "researcher"),
        theme.tool(ToolKind::Agent).patch(theme.title())
    );
    let quota = get("child_failed_quota").display_lines(80, &theme);
    assert_eq!(style_of(&quota, "provider rejected"), theme.failure());

    let edited = get("edited_one").detail_lines(80, &theme);
    assert_eq!(style_of(&edited, "+    self.pending"), theme.diff_add());
    assert_eq!(style_of(&edited, "-    self.file"), theme.diff_remove());
}

#[test]
fn live_cells_tick_and_wear_the_pulse_and_the_wait_timer() {
    let theme = unicode();
    for (name, cell) in fixtures() {
        let live = matches!(
            name,
            "exec_running" | "explored_active" | "waiting" | "compaction_active"
        );
        assert_eq!(cell.animation_tick().is_some(), live, "{name}");
    }
    let waiting = CellModel::Waiting {
        on: vec![(AgentId::new(), "researcher".into())],
        since: at(0),
        ended: None,
    };
    let mut lines = waiting.display_lines(80, &theme);
    let rows = lines.len();
    decorate_live(
        &mut lines[0],
        (0, rows),
        &waiting,
        &theme,
        0.5,
        at(0) + TimeDelta::seconds(41),
    );
    let row = lines[0].to_string();
    assert!(row.ends_with("Waiting for researcher · since 41s"), "{row}");
    assert_eq!(
        lines[0].spans[0].style,
        theme.pulse(0.5).patch(theme.title())
    );
}

#[test]
fn copy_takes_the_text_a_person_would_paste() {
    let cells = fixtures();
    let source = |name: &str| {
        cells
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, c)| c.copy_source())
    };
    assert_eq!(
        source("exec_ok"),
        Some(numbered("test wal::case", 9).as_str())
    );
    assert_eq!(
        source("exec_search_no_match"),
        None,
        "empty output copies nothing"
    );
    assert_eq!(source("spawned"), None);
}

#[test]
fn a_record_remembered_for_a_subtree_names_its_root() {
    let cell = CellModel::ApprovalRecord {
        approval: ApprovalId::new(),
        approved: true,
        what: "bash `git push`".into(),
        scope: ApprovalScope::Subtree {
            root: "worker".into(),
        },
    };
    let text = screen(&cell.display_lines(100, &unicode()), 100);
    assert!(
        text.contains("You approved bash `git push` · saved for worker and its subagents"),
        "{text}"
    );
}

#[test]
fn a_role_that_repeats_the_name_is_not_shown() {
    let spawned = |name: &str, role: &str| CellModel::Spawned {
        agent: AgentId::new(),
        name: name.into(),
        role: role.into(),
        model: "sonnet-5.5".into(),
        effort: None,
        objective: String::new(),
    };
    let head = |cell: CellModel| cell.plain_lines(80)[0].clone();
    assert!(!head(spawned("worker", "worker")).contains('['));
    assert!(!head(spawned("worker 2", "worker")).contains('['));
    assert!(head(spawned("worker2", "worker")).contains("[worker]"));
    assert!(head(spawned("researcher", "explorer")).contains("[explorer]"));
}

/// The pinned header is a dim excerpt ending in `…`, never mistaken for a live prompt.
#[test]
fn a_pinned_prompt_is_dim_and_ends_in_an_ellipsis() {
    let theme = Theme::default();
    let pinned = |text: &str, width| {
        let cell = CellModel::User {
            from: UserFrom::Operator,
            text: text.into(),
        };
        user::pinned_line(&cell, width, &theme).expect("a user cell pins")
    };
    let short = pinned("count slowly please", 80);
    assert_eq!(short.to_string(), "› count slowly please …");
    assert!(
        short.spans[1..]
            .iter()
            .all(|s| s.style == theme.secondary()),
        "{short:?}"
    );
    for width in [20, 22, 23, 24] {
        let cut = pinned("count slowly please", width).to_string();
        assert!(cut.ends_with('…'), "{cut:?} at {width}");
        assert!(
            cut.chars().count() < usize::from(width),
            "{cut:?} at {width}"
        );
    }
}

#[test]
fn pinned_prompt_uses_a_bounded_unicode_prefix() {
    let theme = Theme::default();
    for width in [20, 80, 120] {
        let prefix = "名前 café ".repeat(usize::from(width));
        let prompt = |text| CellModel::User {
            from: UserFrom::Operator,
            text,
        };
        let baseline = user::pinned_line(&prompt(prefix.clone()), width, &theme).unwrap();
        let large = user::pinned_line(
            &prompt(format!(
                "{prefix}{}",
                "ignored trailing data ".repeat(100_000)
            )),
            width,
            &theme,
        )
        .unwrap();
        assert_eq!(large, baseline);
        assert!(large.to_string().contains('…'));
    }
}
