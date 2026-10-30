use aether_core::agent::{AgentStatus, BlockReason, ExhaustionClass, WaitCondition};
use aether_core::model::ModelRef;
use aether_core::time::Timestamp;

use super::*;
use crate::app::approval_flow::fixtures::{
    Fleet, assert_style, at, column_of, draw, fleet, node_mut, push_cell, rows, shell_approval,
};
use crate::app::transcript::{Cell, CellId, CellRevision, UserFrom};
use crate::ui::theme::tokens::ThemeSpec;

/// Root mid-turn: the operator spoke 72 s ago (mock a's `1m 12s`).
fn busy() -> Fleet {
    let mut f = fleet();
    let user = CellModel::User {
        from: UserFrom::Operator,
        text: "Add a --json flag".to_owned(),
        images: Vec::new(),
    };
    push_cell(&mut f.app, f.root, at(-72_000), user);
    f
}

fn set_active(f: &mut Fleet, model: CellModel) {
    f.app.transcripts.entry(f.root).or_default().active = Some(Cell {
        id: CellId(999),
        revision: CellRevision::default(),
        at: at(-1_000),
        model,
    });
}

fn exec(command: &str) -> CellModel {
    CellModel::Exec {
        call: None,
        command: command.to_owned(),
        status: CallStatus::Running,
        output: String::new(),
        exit: None,
        duration_ms: None,
        is_search: false,
        artifacts: Vec::new(),
    }
}

fn header_of(f: &Fleet) -> StatusHeader {
    model(&f.app).expect("busy").header
}

fn exhausted(next_probe: Timestamp) -> AgentStatus {
    AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            model: ModelRef::new("anthropic", "opus-5.5"),
            class: ExhaustionClass::UsageLimit,
            since: at(-60_000),
            reported_reset: None,
            next_probe,
            probes: 1,
            detail: String::new(),
        },
    }
}

#[test]
fn header_follows_priority() {
    let mut f = busy();
    assert_eq!(header_of(&f), StatusHeader::Working);

    set_active(&mut f, CellModel::Compaction(CompactionState::Active));
    assert_eq!(header_of(&f), StatusHeader::Compacting);

    set_active(&mut f, exec("cargo check -p aether\n--verbose"));
    assert_eq!(
        header_of(&f),
        StatusHeader::Running("cargo check -p aether".to_owned())
    );

    let transcript = f.app.transcripts.get_mut(&f.root).expect("transcript");
    transcript.stream.reasoning_title = Some("Planning the flag".to_owned());
    assert_eq!(
        header_of(&f),
        StatusHeader::Reasoning("Planning the flag".to_owned())
    );

    let implementer = f.implementer;
    node_mut(&mut f.app, f.root).status = AgentStatus::Waiting {
        condition: WaitCondition::Any {
            of: vec![
                WaitCondition::MessageFrom { agent: implementer },
                WaitCondition::Deadline { at: at(60_000) },
            ],
        },
    };
    assert_eq!(
        header_of(&f),
        StatusHeader::Waiting(vec!["implementer".to_owned()]),
        "a waiting agent is not reasoning, whatever the stream says"
    );
}

#[test]
fn idle_agents_have_no_status_row() {
    let mut f = busy();
    node_mut(&mut f.app, f.root).status = AgentStatus::Waiting {
        condition: WaitCondition::Inbox,
    };
    assert_eq!(model(&f.app), None);
    node_mut(&mut f.app, f.root).status = AgentStatus::Completed { summary: None };
    assert_eq!(model(&f.app), None);
    assert!(next_frame_in(&f.app).is_some(), "the strip still animates");
}

#[test]
fn context_counts_descendants_and_needs_you() {
    let mut f = busy();
    let deferred = shell_approval(f.tester, "cargo test", at(-5_000));
    f.app.bottom.approval.deferred.insert(deferred.id);
    f.app.approvals = vec![deferred];
    node_mut(&mut f.app, f.researcher).status = AgentStatus::Completed { summary: None };

    let row = model(&f.app).expect("busy");
    assert_eq!(row.descendants_running, 2, "implementer and tester");
    assert_eq!(row.needs_you, 1);
    assert_eq!(
        row.elapsed_ms, 72_000,
        "only the root's own approvals pause it"
    );
    assert_eq!(row.retrying_in_s, None);
}

#[test]
fn the_clock_pauses_while_this_agents_approval_is_open() {
    let mut f = busy();
    f.app.approvals = vec![shell_approval(f.root, "cargo test", at(-10_000))];
    assert_eq!(model(&f.app).expect("busy").elapsed_ms, 62_000);
    f.app.now = at(30_000);
    assert_eq!(model(&f.app).expect("busy").elapsed_ms, 62_000, "frozen");
}

#[test]
fn exhausted_agents_count_down_to_the_next_probe() {
    let mut f = busy();
    node_mut(&mut f.app, f.root).status = exhausted(at(7_500));
    let row = model(&f.app).expect("busy");
    assert_eq!(row.retrying_in_s, Some(8));
}

#[test]
fn the_clock_counts_the_turn_its_separator_will() {
    use crate::app::transcript::{IngestCtx, NoticeKind, ingest_event};
    use aether_core::event::{Actor, Event, EventDraft, EventKind};
    use aether_core::id::EventId;

    let mut f = fleet();
    let separator = CellModel::Separator {
        elapsed_ms: 1_000,
        cost: None,
        agents: 1,
        finished_at: at(-90_000),
    };
    push_cell(&mut f.app, f.root, at(-90_000), separator);
    // The model switched long before the operator sent the next message.
    let switched = CellModel::Notice {
        kind: NoticeKind::ModelChange,
        text: "Model → gpt-6-luna low from the next turn".into(),
    };
    push_cell(&mut f.app, f.root, at(-50_000), switched);
    let started = EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 2 }).agent(f.root);
    let event = Event::from_draft(started, EventId::new(), 1, at(-10_000));
    let ctx = IngestCtx {
        tree: &f.app.tree,
        now: f.app.now,
    };
    ingest_event(&mut f.app.transcripts, &event, &ctx);
    assert_eq!(model(&f.app).expect("busy").elapsed_ms, 10_000);
}

#[test]
fn the_clock_falls_back_when_agent_started_is_not_loaded() {
    let mut f = fleet();
    node_mut(&mut f.app, f.root).status_since = Some(at(-20_000));
    assert_eq!(model(&f.app).unwrap().elapsed_ms, 20_000);
    push_cell(
        &mut f.app,
        f.root,
        at(-10_000),
        CellModel::Assistant {
            text: "working".into(),
        },
    );
    assert_eq!(model(&f.app).unwrap().elapsed_ms, 10_000);
    push_cell(
        &mut f.app,
        f.root,
        at(-5_000),
        CellModel::Separator {
            elapsed_ms: 5_000,
            cost: None,
            agents: 1,
            finished_at: at(-5_000),
        },
    );
    assert_eq!(model(&f.app).unwrap().elapsed_ms, 20_000);
}

#[test]
fn a_running_engine_command_reads_without_its_shell_wrapper() {
    let mut f = busy();
    set_active(&mut f, exec("/usr/bin/zsh -lc 'cargo check -p aether'"));
    assert_eq!(
        header_of(&f),
        StatusHeader::Running("cargo check -p aether".to_owned())
    );
}

fn paint(f: &Fleet, width: u16) -> ratatui::Terminal<ratatui::backend::TestBackend> {
    let row = model(&f.app).expect("busy");
    draw(width, 1, |frame| {
        render(frame, frame.area(), &row, &f.app, &f.app.theme)
    })
}

#[test]
fn renders_mock_a_at_120_and_80() {
    let mut f = busy();
    node_mut(&mut f.app, f.researcher).status = AgentStatus::Completed { summary: None };
    let terminal = paint(&f, 120);
    assert_eq!(
        rows(&terminal),
        ["• Working (1m 12s • esc to interrupt) · 2 agents running"]
    );
    let terminal = paint(&f, 80);
    assert_eq!(
        rows(&terminal),
        ["• Working (1m 12s • esc to interrupt) · 2 agents running"]
    );

    let theme = f.app.theme;
    assert_style(&terminal, 0, 0, theme.running());
    assert_style(&terminal, 2, 0, theme.secondary());
    let esc = column_of(&terminal, 0, "esc");
    assert_style(&terminal, esc, 0, theme.title());
    assert_style(&terminal, esc + 3, 0, theme.secondary());
}

#[test]
fn narrow_rows_drop_context_then_cut_the_header() {
    let mut f = busy();
    let deferred = shell_approval(f.tester, "cargo test", at(-5_000));
    f.app.bottom.approval.deferred.insert(deferred.id);
    f.app.approvals = vec![deferred];
    node_mut(&mut f.app, f.root).status = exhausted(at(7_500));

    let wide = paint(&f, 120);
    assert_eq!(
        rows(&wide),
        [
            "• Working (1m 12s • esc to interrupt) · 3 agents running · ◆ 1 needs you · retrying in 8s"
        ]
    );
    let needs = column_of(&wide, 0, "◆");
    assert_style(&wide, needs, 0, f.app.theme.attention());

    assert_eq!(
        rows(&paint(&f, 60)),
        ["• Working (1m 12s • esc to interrupt) · 3 agents running"]
    );
    assert_eq!(rows(&paint(&f, 33)), ["• Wo… (1m 12s • esc to interrupt)"]);
}

#[test]
fn ascii_and_reduced_motion_stay_static() {
    let mut f = busy();
    f.app.theme = Theme::resolve(ThemeSpec {
        ascii: true,
        reduced_motion: true,
        ..ThemeSpec::default()
    });
    node_mut(&mut f.app, f.researcher).status = AgentStatus::Completed { summary: None };
    let terminal = paint(&f, 80);
    assert_eq!(
        rows(&terminal),
        ["* Working (1m 12s * esc to interrupt) | 2 agents running"]
    );
    assert_style(&terminal, 2, 0, f.app.theme.secondary());
    assert!(!f.app.theme.animates());
}

#[test]
fn static_themes_wake_once_per_elapsed_second() {
    let mut f = busy();
    f.app.now = at(250);
    assert_eq!(next_frame_in(&f.app), Some(750));
    let mut idle = fleet();
    for node in idle.app.tree.nodes.values_mut() {
        node.agent.status = AgentStatus::Completed { summary: None };
    }
    assert_eq!(next_frame_in(&idle.app), None);
}

#[test]
fn an_idle_strip_books_no_frames() {
    let mut f = fleet();
    for node in f.app.tree.nodes.values_mut() {
        node.agent.status = AgentStatus::Ready;
    }
    assert!(model(&f.app).is_none());
    assert!(
        !agents_strip::items(&f.app).is_empty(),
        "idle children still listed"
    );
    assert_eq!(next_frame_in(&f.app), None);
}

#[test]
fn the_band_is_a_short_bright_peak_that_sweeps_across() {
    let cols = 20;
    assert!(
        band_phases(cols, 0.0).iter().all(|p| *p == 0.0),
        "enters from the left"
    );
    assert!(
        band_phases(cols, 1.0).iter().all(|p| *p == 0.0),
        "leaves on the right"
    );
    let mid = band_phases(cols, 0.5);
    let lit = mid.iter().filter(|p| **p > 0.0).count();
    assert!((1..=SHIMMER_BAND_COLS as usize).contains(&lit), "{mid:?}");
    let peak = mid.iter().cloned().fold(0.0, f32::max);
    assert!(peak > 0.8, "{mid:?}");
}

#[test]
fn pulse_phase_is_a_triangle_wave() {
    assert_eq!(pulse_phase(0, PULSE_PERIOD_MS), 0.0);
    assert_eq!(pulse_phase(PULSE_PERIOD_MS / 2, PULSE_PERIOD_MS), 1.0);
    assert_eq!(pulse_phase(PULSE_PERIOD_MS, PULSE_PERIOD_MS), 0.0);
    assert_eq!(pulse_phase(PULSE_PERIOD_MS / 4, PULSE_PERIOD_MS), 0.5);
}

#[test]
fn a_clock_paused_for_an_approval_books_no_frames() {
    let mut f = busy();
    f.app.now = at(250);
    for id in [f.implementer, f.researcher, f.tester] {
        node_mut(&mut f.app, id).status = AgentStatus::Ready;
    }
    assert_eq!(next_frame_in(&f.app), Some(750), "the turn clock ticks");
    // Deferred, so no card's wait clock runs either.
    let approval = shell_approval(f.root, "cargo test", at(-10_000));
    f.app.bottom.approval.deferred.insert(approval.id);
    f.app.approvals = vec![approval];
    assert_eq!(next_frame_in(&f.app), None, "nothing on the row can change");
}

/// M4: esc answers the card, so the row above it never offers esc to interrupt.
#[test]
fn a_card_on_screen_turns_the_row_into_waiting_for_your_approval() {
    let mut f = busy();
    f.app.now = at(250);
    f.app.approvals = vec![shell_approval(f.tester, "cargo test", at(-11_750))];
    assert!(f.app.shown_approval().is_some());
    let row = model(&f.app).expect("busy");
    assert_eq!(row.approval_wait_ms, Some(12_000));
    assert_eq!(row.needs_you, 0, "the card says it");
    let terminal = paint(&f, 120);
    let text = rows(&terminal).remove(0);
    assert!(
        text.starts_with("• Waiting for your approval (12s)"),
        "{text}"
    );
    assert!(!text.contains("esc"), "{text}");
    assert_eq!(next_frame_in(&f.app), Some(1_000), "the wait clock ticks");

    f.app.bottom.approval.deferred.insert(f.app.approvals[0].id);
    let text = rows(&paint(&f, 120)).remove(0);
    assert!(text.contains("esc to interrupt"), "{text}");
}
