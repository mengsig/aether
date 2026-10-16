use aether_core::agent::{AgentStatus, AgentStatusKind, BlockReason, ExhaustionClass};
use aether_core::model::{ModelRef, Usage};
use aether_core::provider::{ProviderUsage, QuotaWindow, UsageWindow};
use chrono::Duration;

use super::fixtures::{TreeBuilder, app_with, approval, ask, fleet, now};
use super::*;

fn names(app: &App) -> Vec<&str> {
    app.board
        .model
        .rows
        .iter()
        .map(|r| r.name.as_str())
        .collect()
}

fn row(app: &App, id: AgentId) -> &BoardRow {
    let index = app.board.model.index_of(id).expect("a shown row");
    &app.board.model.rows[index]
}

fn usd(cost: Option<Cost>) -> f64 {
    cost.expect("a priced cost").usd
}

/// A root running a lead whose two workers have both finished.
fn finished_subtree() -> (App, AgentId, AgentId) {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    let done = || AgentStatus::Completed { summary: None };
    let lead = t.add(Some(root), "lead", "lead", done());
    t.add(Some(lead), "first", "worker", done());
    t.add(Some(lead), "second", "worker", done());
    (app_with(t.build()), root, lead)
}

#[test]
fn mock_fleet_projects_depth_guides_and_statuses() {
    let f = fleet();
    let rows = &f.app.board.model.rows;
    assert_eq!(
        names(&f.app),
        ["aether", "researcher", "implementer", "tester", "docs"]
    );
    let depths: Vec<u16> = rows.iter().map(|r| r.depth).collect();
    assert_eq!(depths, [0, 1, 1, 2, 2]);
    let guides: Vec<&[Guide]> = rows.iter().map(|r| r.guides.as_slice()).collect();
    assert_eq!(
        guides,
        [
            &[][..],
            &[Guide::Tee],
            &[Guide::Corner],
            &[Guide::Blank, Guide::Tee],
            &[Guide::Blank, Guide::Corner],
        ]
    );
    let statuses: Vec<BoardStatus> = rows.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [
            BoardStatus::Running,
            BoardStatus::Completed,
            BoardStatus::Running,
            BoardStatus::NeedsYou,
            BoardStatus::Idle,
        ]
    );
    assert!(row(&f.app, f.tester).needs_you);
    assert!(row(&f.app, f.aether).has_children);
    assert!(!row(&f.app, f.researcher).has_children);
    assert_eq!(row(&f.app, f.researcher).role, "explorer");
    assert_eq!(row(&f.app, f.docs).model, "haiku-4.5");
}

#[test]
fn summary_counts_running_agents_and_rolls_up_cost_including_children() {
    let f = fleet();
    let summary = &f.app.board.model.summary;
    assert_eq!(summary.agents, 5);
    // tester runs but waits on the operator, so it reads as needs-you.
    assert_eq!(summary.running, 2);
    let total = summary.total_cost.expect("a priced fleet");
    assert!(!total.estimated);
    assert!((total.usd - 1.84).abs() < 1e-9, "{}", total.usd);
}

#[test]
fn elapsed_is_time_in_status_for_live_agents_and_runtime_for_finished_ones() {
    let f = fleet();
    let later = now() + Duration::seconds(30);
    assert_eq!(row(&f.app, f.implementer).elapsed_ms(now()), Some(160_000));
    assert_eq!(row(&f.app, f.implementer).elapsed_ms(later), Some(190_000));
    assert_eq!(row(&f.app, f.researcher).elapsed_ms(later), Some(63_000));
}

#[test]
fn finished_subtrees_fold_until_unfolded_and_count_what_they_hide() {
    let (mut app, _, lead) = finished_subtree();
    assert_eq!(names(&app), ["root", "lead"]);
    let folded = row(&app, lead);
    assert!(folded.folded);
    assert_eq!(folded.folded_agents, 2);
    assert_eq!(app.board.model.summary.agents, 4);

    app.board.cursor = Some(lead);
    app.board_unfold();
    assert_eq!(names(&app), ["root", "lead", "first", "second"]);
    assert!(app.board.expanded.contains(&lead));

    app.board_fold();
    assert_eq!(names(&app), ["root", "lead"]);
    assert_eq!(app.board.cursor, Some(lead));
}

#[test]
fn folding_a_live_subtree_keeps_its_needs_you_visible() {
    let mut f = fleet();
    f.app.board.cursor = Some(f.implementer);
    f.app.board_fold();
    assert_eq!(names(&f.app), ["aether", "researcher", "implementer"]);
    let folded = row(&f.app, f.implementer);
    assert!(folded.folded);
    assert_eq!(folded.folded_agents, 2);
    assert!(folded.folded_needs_you);
    assert!(!folded.needs_you);

    f.app.board_unfold();
    assert_eq!(f.app.board.model.rows.len(), 5);
    assert!(!f.app.board.collapsed.contains(&f.implementer));
}

#[test]
fn a_cursor_inside_a_fold_moves_to_the_nearest_shown_ancestor() {
    let mut f = fleet();
    f.app.board.cursor = Some(f.tester);
    f.app.board.collapsed.insert(f.implementer);
    f.app.rebuild_board();
    assert_eq!(f.app.board.cursor, Some(f.implementer));
}

#[test]
fn left_on_a_leaf_goes_to_the_parent_and_right_on_an_open_node_to_its_first_child() {
    let mut f = fleet();
    f.app.board.cursor = Some(f.docs);
    f.app.board_fold();
    assert_eq!(f.app.board.cursor, Some(f.implementer));
    f.app.board_unfold();
    assert_eq!(f.app.board.cursor, Some(f.tester));
    f.app.board.cursor = Some(f.researcher);
    f.app.board_unfold();
    assert_eq!(
        f.app.board.cursor,
        Some(f.researcher),
        "a leaf has no child"
    );
}

#[test]
fn cursor_moves_from_the_viewed_agent_and_clamps_at_both_ends() {
    let mut f = fleet();
    f.app.selected = Some(f.implementer);
    f.app.board_move(1);
    assert_eq!(f.app.board.cursor, Some(f.tester));
    f.app.board_move(-10);
    assert_eq!(f.app.board.cursor, Some(f.aether));
    f.app.board_move(10);
    assert_eq!(f.app.board.cursor, Some(f.docs));
    f.app.board_home();
    assert_eq!(f.app.board.cursor, Some(f.aether));
    f.app.board_end();
    assert_eq!(f.app.board.cursor, Some(f.docs));
}

#[test]
fn archived_agents_are_hidden_unless_shown() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    let old = t.add(Some(root), "old", "worker", AgentStatus::Ready);
    t.add(Some(root), "new", "worker", AgentStatus::Ready);
    t.agent(old).archived = true;
    let mut app = app_with(t.build());
    assert_eq!(names(&app), ["root", "new"]);
    assert_eq!(app.board.model.summary.agents, 2);
    assert_eq!(row(&app, root).guides, Vec::<Guide>::new());

    app.show_archived = true;
    app.rebuild_board();
    assert_eq!(names(&app), ["root", "old", "new"]);
    assert_eq!(row(&app, old).guides, [Guide::Tee]);
}

#[test]
fn several_roots_show_in_creation_order_with_their_objectives() {
    let mut t = TreeBuilder::default();
    let first = t.add(None, "first", "assistant", AgentStatus::Running);
    let second = t.add(None, "second", "assistant", AgentStatus::Running);
    t.objective(first, "Fix the flaky test\nmore detail");
    t.objective(second, "Write the changelog");
    let mut tree = t.build();
    tree.roots.reverse();
    let app = app_with(tree);
    assert_eq!(names(&app), ["first", "second"]);
    assert_eq!(
        row(&app, first).root_objective.as_deref(),
        Some("Fix the flaky test")
    );
    assert_eq!(
        row(&app, second).root_objective.as_deref(),
        Some("Write the changelog")
    );

    let single = fleet();
    assert_eq!(row(&single.app, single.aether).root_objective, None);
}

#[test]
fn needs_you_comes_from_open_approvals_and_unanswered_questions() {
    let mut f = fleet();
    let id = approval(&mut f.app, f.docs, "run cargo test");
    assert_eq!(row(&f.app, f.docs).status, BoardStatus::NeedsYou);
    assert!(f.app.board.model.needs_you.contains(&f.tester));

    f.app.resolving.insert(id);
    f.app.rebuild_board();
    assert_eq!(row(&f.app, f.docs).status, BoardStatus::Idle);

    for cell in f.app.transcripts.values_mut().flat_map(|t| &mut t.cells) {
        if let CellModel::Question { state, .. } = &mut cell.model {
            *state = crate::app::transcript::QuestionState::Answered;
        }
    }
    f.app.rebuild_board();
    assert_eq!(row(&f.app, f.tester).status, BoardStatus::Running);
    ask(&mut f.app, f.tester, f.tester, "which branch?");
    assert_eq!(row(&f.app, f.tester).status, BoardStatus::NeedsYou);
}

#[test]
fn a_finished_agent_never_reads_as_needs_you() {
    let mut f = fleet();
    approval(&mut f.app, f.researcher, "stale");
    assert_eq!(row(&f.app, f.researcher).status, BoardStatus::Completed);
}

#[test]
fn status_mapping_covers_pause_quota_and_imports() {
    assert_eq!(
        BoardStatus::of(AgentStatusKind::Created, false),
        BoardStatus::Idle
    );
    assert_eq!(
        BoardStatus::of(AgentStatusKind::Waiting, true),
        BoardStatus::NeedsYou
    );
    assert_eq!(
        BoardStatus::of(AgentStatusKind::Failed, true),
        BoardStatus::Failed
    );

    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    let paused = t.add(Some(root), "paused", "worker", AgentStatus::Running);
    t.agent(paused).paused = true;
    let quota = t.add(
        Some(root),
        "quota",
        "worker",
        AgentStatus::Blocked {
            reason: exhausted(),
        },
    );
    let imported = t.add(
        Some(root),
        "imported",
        "worker",
        AgentStatus::Imported {
            was: AgentStatusKind::Failed,
        },
    );
    let app = app_with(t.build());
    assert_eq!(row(&app, paused).status, BoardStatus::Paused);
    assert_eq!(
        row(&app, quota).status,
        BoardStatus::Blocked { quota: true }
    );
    assert_eq!(row(&app, imported).status, BoardStatus::Failed);
}

fn exhausted() -> BlockReason {
    BlockReason::Exhausted {
        model: ModelRef::new("anthropic", "opus-5.5"),
        class: ExhaustionClass::QuotaExhausted,
        since: now(),
        reported_reset: None,
        next_probe: now() + Duration::minutes(30),
        probes: 0,
        detail: String::new(),
    }
}

#[test]
fn cost_reads_exact_estimated_floor_or_unknown() {
    let exact = Usage {
        cost_usd: 0.38,
        ..Usage::default()
    };
    assert_eq!(
        cost_of(&exact),
        Some(Cost {
            usd: 0.38,
            estimated: false,
            basis: Default::default(),
        })
    );
    let estimated = Usage {
        cost_usd: 0.10,
        cost_complete: false,
        estimated_cost_usd: Some(0.05),
        ..Usage::default()
    };
    let cost = cost_of(&estimated).expect("an estimate");
    assert!(cost.estimated);
    assert!((cost.usd - 0.15).abs() < 1e-9);
    let floor = Usage {
        cost_usd: 0.20,
        cost_complete: false,
        ..Usage::default()
    };
    assert_eq!(
        cost_of(&floor),
        Some(Cost {
            usd: 0.20,
            estimated: true,
            basis: Default::default(),
        })
    );
    let unknown = Usage {
        cost_complete: false,
        ..Usage::default()
    };
    assert_eq!(cost_of(&unknown), None);
}

#[test]
fn one_unpriced_child_makes_the_total_an_estimate() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    t.cost(root, 1.0);
    let child = t.add(Some(root), "child", "worker", AgentStatus::Running);
    t.agent(child).stats.usage = Usage {
        cost_usd: 0.5,
        cost_complete: false,
        ..Usage::default()
    };
    let app = app_with(t.build());
    let total = app.board.model.summary.total_cost.expect("a floor");
    assert!(total.estimated);
    assert!((total.usd - 1.5).abs() < 1e-9);
    assert!(row(&app, child).cost.expect("a floor").estimated);
}

#[test]
fn a_cycle_in_child_links_does_not_loop() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "root", "orchestrator", AgentStatus::Running);
    let child = t.add(Some(root), "child", "worker", AgentStatus::Running);
    let mut tree = t.build();
    let link = tree.nodes.get_mut(&child).expect("the child");
    link.children.push(root);
    link.agent.parent = None;
    tree.nodes.get_mut(&root).expect("the root").agent.parent = Some(child);
    let app = app_with(tree);
    assert_eq!(names(&app), ["root", "child"]);
}

#[test]
fn revision_moves_only_when_the_model_changes() {
    let mut f = fleet();
    let before = f.app.board.revision;
    f.app.rebuild_board();
    assert_eq!(f.app.board.revision, before);
    approval(&mut f.app, f.docs, "edit docs/cli.md");
    assert_ne!(f.app.board.revision, before);
}

#[test]
fn board_opens_by_default_only_when_wide_and_remembers_within_dock_widths() {
    assert!(!default_board_open(120, None));
    assert!(default_board_open(140, None));
    assert!(default_board_open(100, Some(true)));
    assert!(!default_board_open(99, Some(true)));
    assert!(!default_board_open(200, Some(false)));
}

#[test]
fn inspector_reports_lineage_children_and_cost_with_children() {
    let f = fleet();
    let model = InspectorModel::build(&f.app, f.implementer).expect("a shown agent");
    assert_eq!(model.name, "implementer");
    assert_eq!(model.role, "worker");
    assert_eq!(model.status, BoardStatus::Running);
    assert_eq!(model.parent_name.as_deref(), Some("aether"));
    assert_eq!(model.turn, Some(6));
    assert_eq!(model.elapsed_ms(now()), Some(160_000));
    assert!(
        model
            .objective
            .as_deref()
            .is_some_and(|o| o.starts_with("Add --json"))
    );
    assert!((usd(model.cost) - 0.38).abs() < 1e-9);
    assert!((usd(model.cost_with_children) - 0.51).abs() < 1e-9);
    let children: Vec<(&str, BoardStatus)> = model
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.status))
        .collect();
    assert_eq!(
        children,
        [
            ("tester", BoardStatus::NeedsYou),
            ("docs", BoardStatus::Idle)
        ]
    );
    assert_eq!(model.asks, None);
    assert_eq!(model.branch, None);

    let leaf = InspectorModel::build(&f.app, f.docs).expect("a shown agent");
    assert_eq!(leaf.cost_with_children, None);
    assert!(leaf.children.is_empty());
}

#[test]
fn inspector_says_what_a_waiting_agent_asks() {
    let mut f = fleet();
    let tester = InspectorModel::build(&f.app, f.tester).expect("a shown agent");
    assert_eq!(
        tester.asks,
        Some(Asks::Question(
            "should --json include imported runs, or only live ones?".into()
        ))
    );
    assert_eq!(
        tester.last_activity, None,
        "the question sits in implementer's transcript"
    );
    approval(&mut f.app, f.docs, "cargo test");
    let docs = InspectorModel::build(&f.app, f.docs).expect("a shown agent");
    assert_eq!(
        docs.asks,
        Some(Asks::Approval("Wants to run cargo test".into()))
    );
}

#[test]
fn inspector_shows_the_first_error_line_and_the_quota_behind_it() {
    let mut t = TreeBuilder::default();
    let root = t.add(
        None,
        "root",
        "orchestrator",
        AgentStatus::Failed {
            error: "\n  provider refused: weekly limit\nretry later".into(),
        },
    );
    let mut app = app_with(t.build());
    app.quotas.insert(
        "anthropic".into(),
        ProviderUsage {
            observed_at: now(),
            windows: vec![UsageWindow {
                name: "seven_day".into(),
                quota: QuotaWindow {
                    used_percent: 100,
                    window_minutes: Some(10_080),
                    resets_at: None,
                },
            }],
        },
    );
    let model = InspectorModel::build(&app, root).expect("a shown agent");
    assert_eq!(model.status, BoardStatus::Failed);
    assert_eq!(
        model.error.as_deref(),
        Some("provider refused: weekly limit")
    );
    assert_eq!(
        model.quota.as_deref(),
        Some("anthropic weekly limit reached")
    );
}

#[test]
fn inspector_names_a_quota_block_and_when_it_retries() {
    let mut t = TreeBuilder::default();
    let root = t.add(
        None,
        "root",
        "orchestrator",
        AgentStatus::Blocked {
            reason: exhausted(),
        },
    );
    let app = app_with(t.build());
    let model = InspectorModel::build(&app, root).expect("a shown agent");
    let quota = model.quota.expect("a quota line");
    assert!(
        quota.starts_with("anthropic quota exhausted · retrying at "),
        "{quota}"
    );
}

#[test]
fn inspector_reads_last_activity_from_the_transcript() {
    use crate::app::transcript::{CallStatus, CellModel};
    let mut f = fleet();
    super::fixtures::push_cell(
        &mut f.app,
        f.implementer,
        CellModel::Exec {
            call: None,
            command: "cargo test -p aether runs_list\n".into(),
            status: CallStatus::Succeeded,
            output: String::new(),
            exit: Some(0),
            duration_ms: Some(1_200),
            is_search: false,
            artifacts: Vec::new(),
        },
    );
    let model = InspectorModel::build(&f.app, f.implementer).expect("a shown agent");
    let activity = model.last_activity.expect("an activity");
    assert_eq!(
        (activity.verb.as_str(), activity.rest.as_str()),
        ("Ran", "cargo test -p aether runs_list")
    );
}

fn waiting(condition: aether_core::agent::WaitCondition) -> AgentStatus {
    AgentStatus::Waiting { condition }
}

#[test]
fn waiting_on_nothing_in_particular_reads_idle_and_shows_no_clock() {
    use aether_core::agent::WaitCondition;
    use aether_core::id::TaskId;
    let deadline = || WaitCondition::Deadline {
        at: now() + Duration::minutes(5),
    };
    let mut t = TreeBuilder::default();
    let inbox = t.add(None, "inbox", "assistant", waiting(WaitCondition::Inbox));
    let alone = t.add(None, "alone", "assistant", waiting(deadline()));
    let parent = t.add(None, "parent", "lead", waiting(deadline()));
    t.add(Some(parent), "child", "worker", AgentStatus::Running);
    let task = t.add(
        None,
        "task",
        "lead",
        waiting(WaitCondition::Task { id: TaskId::new() }),
    );
    let app = app_with(t.build());
    assert_eq!(row(&app, inbox).status, BoardStatus::Idle);
    assert_eq!(row(&app, alone).status, BoardStatus::Idle);
    assert_eq!(row(&app, parent).status, BoardStatus::Waiting);
    assert_eq!(row(&app, task).status, BoardStatus::Waiting);
    assert_eq!(row(&app, inbox).elapsed_ms(now()), None);
    assert!(row(&app, task).elapsed_ms(now()).is_some());
    let inspector = InspectorModel::build(&app, inbox).expect("a shown agent");
    assert_eq!(inspector.status, BoardStatus::Idle);
    assert_eq!(inspector.elapsed_ms(now()), None);
    // The tree-blind mapping the agents strip still reads keeps its old answer.
    let agent = &app.tree.nodes[&inbox].agent;
    assert_eq!(status_of(agent, false), BoardStatus::Waiting);
}

#[test]
fn same_named_siblings_are_numbered_in_creation_order() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "aether", "orchestrator", AgentStatus::Running);
    let first = t.add(Some(root), "worker", "worker", AgentStatus::Running);
    let second = t.add(Some(root), "worker", "worker", AgentStatus::Running);
    let literal = t.add(Some(root), "worker 2", "worker", AgentStatus::Running);
    let third = t.add(Some(root), "worker", "worker", AgentStatus::Running);
    let nephew = t.add(Some(second), "worker", "worker", AgentStatus::Running);
    let mut tree = t.build();
    // Stored child order must not matter, only creation order.
    tree.nodes.get_mut(&root).expect("root").children.reverse();
    let renamed = crate::app::helpers::sibling_names(&tree);
    assert_eq!(renamed.get(&first), None);
    assert_eq!(renamed.get(&literal), None);
    assert_eq!(renamed[&second], "worker 3");
    assert_eq!(renamed[&third], "worker 4");
    assert_eq!(renamed.get(&nephew), None, "only siblings are compared");

    let app = app_with(tree);
    assert_eq!(
        names(&app),
        [
            "aether", "worker", "worker 3", "worker", "worker 2", "worker 4"
        ]
    );
    let inspector = InspectorModel::build(&app, nephew).expect("a shown agent");
    assert_eq!(inspector.parent_name.as_deref(), Some("worker 3"));
    assert_eq!(
        inspector.role, "",
        "a role equal to the name is not repeated"
    );
    let parent = InspectorModel::build(&app, root).expect("a shown agent");
    let children: Vec<&str> = parent.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(children, ["worker", "worker 3", "worker 2", "worker 4"]);
}

#[test]
fn the_board_reads_ids_in_free_text_as_names() {
    let mut t = TreeBuilder::default();
    let root = t.add(None, "aether", "orchestrator", AgentStatus::Running);
    let worker = t.add(
        Some(root),
        "worker",
        "implementer",
        AgentStatus::Failed {
            error: String::new(),
        },
    );
    let other = t.add(None, "other", "assistant", AgentStatus::Running);
    t.objective(root, &format!("Supervise {worker}"));
    t.objective(other, &format!("Review what {worker} wrote"));
    t.agent(worker).status = AgentStatus::Failed {
        error: format!("{root} cancelled the task"),
    };
    let mut app = app_with(t.build());
    approval(&mut app, root, "ls");
    let raw = |text: &str| text.contains("agt_") || text.contains("tsk_") || text.contains('{');

    let inspector = InspectorModel::build(&app, root).expect("a shown agent");
    assert_eq!(inspector.objective.as_deref(), Some("Supervise worker"));
    let Some(Asks::Approval(wants)) = &inspector.asks else {
        panic!("an approval: {:?}", inspector.asks);
    };
    assert!(!raw(wants), "{wants}");
    let failed = InspectorModel::build(&app, worker).expect("a shown agent");
    assert_eq!(failed.error.as_deref(), Some("aether cancelled the task"));
    assert_eq!(
        row(&app, other).root_objective.as_deref(),
        Some("Review what worker wrote")
    );
    super::fixtures::push_cell(
        &mut app,
        root,
        CellModel::Sent {
            to: format!("{worker}"),
            text: "hi".into(),
        },
    );
    let activity = InspectorModel::build(&app, root)
        .and_then(|m| m.last_activity)
        .expect("an activity");
    assert_eq!(activity.rest, "worker");
}

#[test]
fn approvals_read_as_what_the_agent_wants_to_do() {
    use crate::app::approvals::wants;
    use aether_core::tool::ResourceKey;
    let mut app = fleet().app;
    let id = approval(&mut app, AgentId::new(), "ignored");
    let base = app
        .approvals
        .iter()
        .find(|a| a.id == id)
        .expect("the approval")
        .clone();
    let with = |tool: &str, argument: Option<&str>, resources: &[&str]| {
        let mut request = base.clone();
        request.tool = tool.into();
        request.description = format!("{tool} {{\"x\":1}}");
        request.argument = argument.map(str::to_owned);
        request.resources = resources.iter().map(|r| ResourceKey((*r).into())).collect();
        wants(&request)
    };
    assert_eq!(
        with("bash", Some("cargo test"), &[]),
        "Wants to run cargo test"
    );
    assert_eq!(
        with(
            "web_fetch",
            Some("https://example.com/changelog"),
            &["network"]
        ),
        "Wants to reach https://example.com/changelog"
    );
    assert_eq!(
        with("web_search", Some("ratatui 0.30"), &[]),
        "Wants to search the web for ratatui 0.30"
    );
    assert_eq!(with("web_fetch", None, &[]), "Wants to reach the network");
    assert_eq!(
        with("write_file", None, &["file:src/main.rs"]),
        "Wants to change src/main.rs"
    );
    assert_eq!(with("mcp.github", None, &[]), "Wants to use mcp.github");
}

#[test]
fn last_activity_uses_the_past_tense_only_after_success() {
    use crate::app::transcript::{CallStatus, CallTarget};
    let fetch = |status| CellModel::Called {
        target: CallTarget::WebFetch {
            url: "https://example.com/changelog".into(),
        },
        args: String::new(),
        result: String::new(),
        status,
        error: None,
        artifacts: Vec::new(),
    };
    let exec = |status, exit| CellModel::Exec {
        call: None,
        command: "cargo test".into(),
        status,
        output: String::new(),
        exit,
        duration_ms: None,
        is_search: false,
        artifacts: Vec::new(),
    };
    let cases = [
        (fetch(CallStatus::Awaiting), "Wants to fetch", false),
        (fetch(CallStatus::Running), "Fetching", false),
        (fetch(CallStatus::Succeeded), "Fetched", false),
        (fetch(CallStatus::Failed), "Failed to fetch", true),
        (fetch(CallStatus::Denied), "Not allowed to fetch", true),
        (fetch(CallStatus::Cancelled), "Stopped fetching", false),
        (exec(CallStatus::Awaiting, None), "Wants to run", false),
        (exec(CallStatus::Running, None), "Running", false),
        (exec(CallStatus::Succeeded, Some(0)), "Ran", false),
        (exec(CallStatus::Failed, Some(101)), "Ran", true),
        (exec(CallStatus::Failed, None), "Failed to run", true),
    ];
    for (cell, verb, failed) in cases {
        let mut f = fleet();
        super::fixtures::push_cell(&mut f.app, f.implementer, cell.clone());
        let activity = InspectorModel::build(&f.app, f.implementer)
            .and_then(|m| m.last_activity)
            .expect("an activity");
        assert_eq!(
            (activity.verb.as_str(), activity.failed),
            (verb, failed),
            "{cell:?}"
        );
    }
    let mut f = fleet();
    super::fixtures::push_cell(
        &mut f.app,
        f.implementer,
        exec(CallStatus::Failed, Some(101)),
    );
    let activity = InspectorModel::build(&f.app, f.implementer)
        .and_then(|m| m.last_activity)
        .expect("an activity");
    assert_eq!(activity.rest, "cargo test (exit 101)");
}
