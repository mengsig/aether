//! The indexed fleet status must render the same bytes as the scan it replaced.

use super::*;
use aether_core::agent::{AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::id::{ApprovalId, RunId, ToolCallId};
use aether_core::model::{CapabilityProfile, ModelRef};
use aether_core::task::TaskResult;
use aether_core::time::now;
use aether_core::tool::{ApprovalStatus, SideEffectClass};

fn agent(name: &str) -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: name.into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status: AgentStatus::Ready,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

fn task(objective: &str, owner: Option<AgentId>, status: TaskStatus) -> Task {
    let mut task = Task::new(objective, Actor::user(), RunId::new(), now());
    task.owner = owner;
    task.status = status;
    task
}

fn completed(summary: &str) -> TaskStatus {
    TaskStatus::Completed {
        result: TaskResult::summary(summary),
    }
}

fn approval(agent: AgentId) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: agent,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "shell".into(),
        description: "run\ncargo test".into(),
        argument: None,
        resources: Vec::new(),
        side_effect: SideEffectClass::Mutation,
        requested_at: now(),
        status: ApprovalStatus::Pending,
    }
}

fn inputs<'a>(
    agent: &'a Agent,
    children: &'a [Agent],
    tasks: &'a [Task],
    approvals: &'a [ApprovalRequest],
    caps: &'a CapabilityProfile,
) -> ContextInputs<'a> {
    ContextInputs {
        agent,
        task: None,
        children,
        child_tasks: tasks,
        pending_inbox: &[],
        pending_approvals: approvals,
        checkpoint: None,
        transcript: &[],
        memories: &[],
        notes: &[],
        notes_omitted: 0,
        tools: vec![],
        model_profile: None,
        engine_policy: None,
        capabilities: caps,
        adapter: "scripted/1",
        attachment_dir: None,
        turn: 7,
        now: now(),
    }
}

/// The child and task sections exactly as rendered before indexing: a scan of
/// every task per child and of every child per task. Kept as the oracle.
fn scanned_fleet(children: &[Agent], tasks: &[Task]) -> String {
    let mut out = String::new();
    if !children.is_empty() {
        out.push_str("\nChild agents:\n");
        for child in children {
            let task = tasks.iter().find(|t| t.owner == Some(child.id));
            let _ = write!(
                out,
                "- {} \"{}\" ({}) {}",
                child.id,
                child.name,
                child.role,
                child.status.kind()
            );
            if let Some(t) = task {
                let _ = write!(
                    out,
                    " on task {} [{}]: {}",
                    t.id,
                    t.status.kind(),
                    short(&t.objective, 100)
                );
            }
            out.push('\n');
        }
    }
    let orphan_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.owner.is_none() || !children.iter().any(|c| Some(c.id) == t.owner))
        .collect();
    if !orphan_tasks.is_empty() {
        out.push_str("\nOther tasks you created:\n");
        for t in orphan_tasks {
            let _ = writeln!(
                out,
                "- {} [{}]: {}",
                t.id,
                t.status.kind(),
                short(&t.objective, 100)
            );
            if let TaskStatus::Completed { result } = &t.status {
                let _ = writeln!(out, "  result: {}", short(&result.summary, 200));
            }
        }
    }
    out
}

fn indexed_fleet(children: &[Agent], tasks: &[Task]) -> String {
    let links = FleetLinks::new(children, tasks);
    let mut out = String::new();
    push_child_agents(&mut out, children, &links);
    push_other_tasks(&mut out, tasks, &links);
    out
}

/// A deterministic xorshift, so a failing case reproduces from its seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

const OBJECTIVES: [&str; 5] = [
    "Ship it",
    "  line one\nline two  ",
    "Répare le résumé ünïcode — 日本語のテキスト ",
    "",
    "long objective words repeated many times over and over again until it passes one hundred characters",
];

/// Children with duplicates, tasks owned by listed children, by unlisted
/// agents and by nobody, with several tasks per owner in mixed order.
fn random_fleet(rng: &mut Rng) -> (Vec<Agent>, Vec<Task>) {
    let mut children: Vec<Agent> = (0..rng.below(7)).map(|i| agent(&format!("c{i}"))).collect();
    if !children.is_empty() && rng.below(2) == 0 {
        let twin = children[rng.below(children.len())].clone();
        children.insert(rng.below(children.len() + 1), twin);
    }
    let strangers = [AgentId::new(), AgentId::new()];
    let tasks = (0..rng.below(12))
        .map(|_| {
            let owner = match rng.below(4) {
                0 => None,
                1 => Some(strangers[rng.below(2)]),
                _ if children.is_empty() => None,
                _ => Some(children[rng.below(children.len())].id),
            };
            let status = match rng.below(3) {
                0 => TaskStatus::Queued,
                1 => TaskStatus::Active,
                _ => completed(OBJECTIVES[rng.below(OBJECTIVES.len())]),
            };
            task(OBJECTIVES[rng.below(OBJECTIVES.len())], owner, status)
        })
        .collect();
    (children, tasks)
}

#[test]
fn indexed_fleet_matches_the_scan_for_generated_fleets() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut shapes = [false; 3];
    for case in 0..500 {
        let (children, tasks) = random_fleet(&mut rng);
        let expected = scanned_fleet(&children, &tasks);
        assert_eq!(indexed_fleet(&children, &tasks), expected, "case {case}");
        shapes[0] |= expected.contains("Other tasks you created:");
        shapes[1] |= expected.contains(" on task ");
        shapes[2] |= children.len() > FleetLinks::new(&children, &tasks).first_task.len();
    }
    assert_eq!(
        shapes, [true; 3],
        "orphans, owned tasks and duplicate children"
    );
}

#[test]
fn indexed_fleet_matches_the_scan_at_fleet_scale() {
    let children: Vec<Agent> = (0..300).map(|i| agent(&format!("worker-{i}"))).collect();
    let mut tasks: Vec<Task> = children
        .iter()
        .rev()
        .map(|c| task("second task listed first", Some(c.id), TaskStatus::Active))
        .collect();
    tasks.extend(
        children
            .iter()
            .map(|c| task("later", Some(c.id), completed("ok"))),
    );
    tasks.push(task("ownerless", None, completed("kept")));
    assert_eq!(
        indexed_fleet(&children, &tasks),
        scanned_fleet(&children, &tasks)
    );
    assert_eq!(indexed_fleet(&[], &tasks), scanned_fleet(&[], &tasks));
    assert_eq!(indexed_fleet(&children, &[]), scanned_fleet(&children, &[]));
}

#[test]
fn detailed_status_keeps_its_exact_text_and_section_order() {
    let mut root = agent("main");
    root.config.effort = Some(Effort::High);
    root.status = AgentStatus::Waiting {
        condition: WaitCondition::Inbox,
    };
    root.resume_wait = Some(WaitCondition::Inbox);
    let (a, b) = (agent("a"), agent("b"));
    let children = vec![a.clone(), b.clone(), a.clone()];
    let tasks = vec![
        task("stray", Some(AgentId::new()), TaskStatus::Queued),
        task("first of a", Some(a.id), TaskStatus::Queued),
        task("ownerless", None, completed("line\nbreak")),
        task("second of a", Some(a.id), TaskStatus::Active),
    ];
    let approvals = vec![approval(root.id)];
    let caps = CapabilityProfile::default();
    let text = runtime_state(&inputs(&root, &children, &tasks, &approvals, &caps));
    let a_line = format!(
        "- {} \"a\" (worker) ready on task {} [queued]: first of a",
        a.id, tasks[1].id
    );
    let expected = format!(
        "Turn 7. Effort high.\nYou were waiting on: any inbox item.\n\n\
         Child agents:\n{a_line}\n- {} \"b\" (worker) ready\n{a_line}\n\n\
         Other tasks you created:\n- {} [queued]: stray\n- {} [completed]: ownerless\n  result: line break\n\n\
         Pending approvals (the runtime will wake you when resolved):\n- {} for `shell`: run cargo test\n\n\
         You were waiting for any inbox item and were woken early (a person or your parent spoke, a \
         delegated task failed or was cancelled, or a trigger arrived). After this turn you return to \
         that wait unless you complete the task, fail it, or call wait_for.",
        b.id, tasks[0].id, tasks[2].id, approvals[0].id
    );
    assert_eq!(text, expected);
}

fn digest_note(text: &str, links: &[&str]) -> Note {
    Note {
        text: text.into(),
        links: links.iter().map(|link| (*link).to_owned()).collect(),
        context: aether_core::note::NoteContext::Digest,
        level: aether_core::note::NoteLevel::Info,
    }
}

#[test]
fn both_status_texts_carry_the_note_digest_and_only_when_there_is_one() {
    let me = agent("orchestrator");
    let caps = CapabilityProfile::default();
    let notes = [
        digest_note("PR 12 merged", &["https://example.com/pull/12"]),
        digest_note(&format!("{}\nsecond line", "long ".repeat(200)), &[]),
    ];
    let mut with_notes = inputs(&me, &[], &[], &[], &caps);
    with_notes.notes = &notes;
    with_notes.notes_omitted = 3;
    for status in [
        runtime_state(&with_notes),
        engine_runtime_state(&with_notes),
    ] {
        assert!(
            status.contains("- PR 12 merged (https://example.com/pull/12)"),
            "{status}"
        );
        assert!(status.contains("(3 earlier notes left out.)"), "{status}");
        assert!(
            !status.contains("second line\n"),
            "one line per note: {status}"
        );
        assert!(status.contains('…'), "a long note is clipped: {status}");
    }
    let without = inputs(&me, &[], &[], &[], &caps);
    for status in [runtime_state(&without), engine_runtime_state(&without)] {
        assert!(!status.contains("Notes added"), "{status}");
    }
}
