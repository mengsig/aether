use super::*;
use aether_core::agent::{AgentConfig, AgentStatus, Role, Scope};
use aether_core::event::Actor;
use aether_core::model::ModelRef;
use aether_core::time::Timestamp;

/// A full id of this kind whose ULID starts with `head`.
fn id<I: FromStr>(prefix: &str, head: &str) -> I
where
    I::Err: fmt::Debug,
{
    format!("{prefix}_{head:0<26}").parse().expect("fixture id")
}

fn at(second: u32) -> Timestamp {
    format!("2026-10-16T10:00:{second:02}Z")
        .parse()
        .expect("fixture time")
}

fn agent(head: &str, name: &str, run: RunId, parent: Option<AgentId>, second: u32) -> Agent {
    Agent {
        id: id("agt", head),
        parent,
        role: Role::new("worker"),
        name: name.into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "mock-1"),
        fallbacks: Vec::new(),
        status: AgentStatus::Ready,
        status_since: None,
        paused: false,
        paused_via: None,
        current_task: None,
        archived: false,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: AgentConfig::default(),
        stats: Default::default(),
        run,
        created_at: at(second),
        updated_at: at(second),
    }
}

fn finished(mut agent: Agent) -> Agent {
    agent.status = AgentStatus::Completed { summary: None };
    agent
}

/// Two runs. The older run's worker was created after the newer run's root,
/// so "newest agent anywhere" and "newest agent of the current run" differ.
fn fleet() -> Vec<Agent> {
    let old_run: RunId = id("run", "01J5A");
    let new_run: RunId = id("run", "01J5B");
    let old_root = agent("01J5A0", "planner", old_run, None, 1);
    let new_root = agent("01J5B0", "reviewer", new_run, None, 2);
    let new_child = agent("01J5B1", "tester", new_run, Some(new_root.id), 3);
    let late_old_child = agent("01J5A1", "builder", old_run, Some(old_root.id), 4);
    let done = finished(agent("01J5C0", "tester", new_run, Some(new_root.id), 0));
    vec![old_root, new_root, new_child, late_old_child, done]
}

fn error_text(result: anyhow::Result<impl fmt::Debug>) -> String {
    format!("{:#}", result.expect_err("refused"))
}

#[test]
fn a_unique_prefix_resolves_with_or_without_its_kind_prefix_in_any_case() {
    let agents = fleet();
    let builder: AgentId = id("agt", "01J5A1");
    for text in ["01J5A1", "01j5a1", "agt_01J5A1", "AGT_01j5A1"] {
        assert_eq!(resolve_agent(text, &agents).expect(text), builder, "{text}");
    }
    assert_eq!(
        resolve_agent(&builder.to_string(), &agents).expect("a full id"),
        builder
    );
    assert!(
        resolve_agent("run_01J5A1", &agents).is_err(),
        "another kind's prefix"
    );
}

#[test]
fn a_live_name_resolves_and_a_finished_namesake_does_not_compete() {
    let agents = fleet();
    assert_eq!(
        resolve_agent("planner", &agents).expect("unique name"),
        id::<AgentId>("agt", "01J5A0")
    );
    // A finished "tester" exists too; only the live one counts.
    assert_eq!(
        resolve_agent("tester", &agents).expect("unique live name"),
        id::<AgentId>("agt", "01J5B1")
    );
    let message = error_text(resolve_agent("Planner", &agents));
    assert!(message.contains("no agent matches `Planner`"), "{message}");
    assert!(message.contains("aether agents --all"), "{message}");
}

#[test]
fn an_ambiguous_prefix_or_name_is_refused_with_the_candidates() {
    let agents = fleet();
    let message = error_text(resolve_agent("01J5A", &agents));
    assert!(message.contains("`01J5A` matches 2 agents"), "{message}");
    for candidate in ["agt_01J5A0", "agt_01J5A1"] {
        assert!(message.contains(candidate), "{candidate} in {message}");
    }
    assert!(message.contains("planner") && message.contains("builder"));

    let mut twins = fleet();
    twins.push(agent("01J5D0", "planner", id("run", "01J5D"), None, 5));
    let message = error_text(resolve_agent("planner", &twins));
    assert!(message.contains("matches 2 agents"), "{message}");
    assert!(message.contains("agt_01J5A0") && message.contains("agt_01J5D0"));
}

#[test]
fn a_long_ambiguity_lists_the_newest_and_counts_the_rest() {
    let run: RunId = id("run", "01J6");
    let agents: Vec<Agent> = (0..13)
        .map(|n| agent(&format!("01J6{n:02}"), "w", run, None, n))
        .collect();
    let message = error_text(resolve_agent("01J6", &agents));
    assert!(message.contains("matches 13 agents"), "{message}");
    assert!(message.contains("and 3 more"), "{message}");
    assert!(message.contains("agt_01J612"), "newest listed: {message}");
    assert!(
        !message.contains("agt_01J600"),
        "oldest left out: {message}"
    );
}

#[test]
fn last_is_the_newest_agent_of_the_current_run() {
    let agents = fleet();
    // The builder is the newest agent anywhere, but its run is not current.
    assert_eq!(
        resolve_agent(LAST, &agents).expect("@last"),
        id::<AgentId>("agt", "01J5B1")
    );
    assert_eq!(
        resolve_run(LAST, &agents).expect("@last run"),
        id::<RunId>("run", "01J5B")
    );
    assert!(error_text(resolve_agent(LAST, &[])).contains("no run yet"));
    let message = error_text(resolve_agent("@first", &agents));
    assert!(message.contains("`@first` is not a reference"), "{message}");
}

#[test]
fn viewed_is_what_a_tui_beside_the_socket_publishes() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let socket = Endpoint::from(dir.path().join("aether.sock"));
    let message = error_text(viewed_agent(&socket));
    assert!(
        message.contains("no running TUI views an agent"),
        "{message}"
    );

    let viewed: AgentId = id("agt", "01J5B1");
    let local = socket.local_path().expect("a local socket");
    let mut tui = aether_tui::viewed::Publisher::new(aether_tui::viewed::viewed_dir(local));
    tui.sync(Some(viewed)).expect("publish");
    assert_eq!(viewed_agent(&socket).expect("viewed"), viewed);
    assert_eq!(
        run_of(viewed, &fleet()).expect("its run"),
        id::<RunId>("run", "01J5B")
    );
    assert!(
        run_of(id("agt", "01J9"), &fleet()).is_err(),
        "not on this daemon"
    );
    tui.finish().expect("withdraw");
    assert!(viewed_agent(&socket).is_err(), "a closed TUI views nothing");
}

#[test]
fn runs_resolve_by_prefix_and_archive_targets_by_either_kind() {
    let agents = fleet();
    assert_eq!(
        resolve_run("01J5A", &agents).expect("run prefix"),
        id::<RunId>("run", "01J5A")
    );
    assert!(matches!(
        resolve_target("run_01J5B", &agents).expect("a run"),
        ArchiveTarget::Run(run) if run == id::<RunId>("run", "01J5B")
    ));
    assert!(matches!(
        resolve_target("reviewer", &agents).expect("an agent by name"),
        ArchiveTarget::Agent(agent) if agent == id::<AgentId>("agt", "01J5B0")
    ));
    // Without a kind prefix, agents and runs compete.
    let message = error_text(resolve_target("01J5B", &agents));
    assert!(message.contains("agt_01J5B0") && message.contains("run_01J5B"));
}

#[test]
fn tasks_and_schedules_resolve_by_prefix_and_schedules_by_live_name() {
    let run: RunId = id("run", "01J7");
    let mut task = Task::new("Review the README", Actor::System, run, at(1));
    task.id = id("tsk", "01J7A");
    assert_eq!(
        resolve_task("01J7", &[task.clone()]).expect("task"),
        task.id
    );
    assert!(error_text(resolve_task("README", &[task])).contains("no task matches"));

    let jobs = [
        schedule("01J8A", "nightly", "active"),
        schedule("01J8B", "weekly", "cancelled"),
    ];
    assert_eq!(
        resolve_schedule("nightly", &jobs).expect("live"),
        jobs[0].id
    );
    assert_eq!(
        resolve_schedule("weekly", &jobs).expect("finished"),
        jobs[1].id,
        "a finished schedule is still found by name"
    );
    let reused = [
        schedule("01J8C", "nightly", "cancelled"),
        schedule("01J8D", "nightly", "active"),
    ];
    assert_eq!(
        resolve_schedule("nightly", &reused).expect("live wins"),
        reused[1].id,
        "a live schedule holds the name"
    );
    assert_eq!(
        resolve_schedule("sch_01J8B", &jobs).expect("prefix"),
        jobs[1].id
    );
}

fn schedule(head: &str, name: &str, state: &str) -> Schedule {
    let id: ScheduleId = id("sch", head);
    let status = match state {
        "active" => r#"{"state": "active", "next_due": "2026-10-16T11:00:00Z"}"#,
        _ => r#"{"state": "cancelled", "at": "2026-10-16T09:00:00Z"}"#,
    };
    let json = format!(
        r#"{{"id": "{id}", "name": "{name}", "owner": {{"actor": "user"}},
            "target": "agt_01J00000000000000000000001", "run": "run_01J00000000000000000000002",
            "recurrence": {{"kind": "interval", "seconds": 3600}}, "missed": "coalesce",
            "busy": "skip", "payload": null, "status": {status}, "max_deliveries": null,
            "deliveries": 0, "last": null,
            "created_at": "2026-10-16T08:00:00Z", "updated_at": "2026-10-16T08:00:00Z"}}"#
    );
    serde_json::from_str(&json).expect("fixture schedule")
}

#[test]
fn an_empty_reference_is_refused_when_parsed() {
    assert!("  ".parse::<AgentRef>().is_err());
    let parsed: AgentRef = " planner ".parse().expect("a name");
    assert_eq!(parsed.to_string(), "planner");
}

#[test]
fn kinds_without_names_take_only_id_shapes_before_any_lookup() {
    for text in ["01J5", "tsk_01j5", "TSK_01J5A", &TaskId::new().to_string()] {
        assert!(text.parse::<TaskRef>().is_ok(), "{text}");
    }
    for text in [
        "bad-task",
        "bad",
        "agt_01J5",
        "tsk_",
        "01JL",
        &format!("{:0<27}", "01"),
    ] {
        assert!(text.parse::<TaskRef>().is_err(), "{text}");
    }
    assert!("@last".parse::<RunRef>().is_ok());
    assert!("bad-task".parse::<AgentRef>().is_ok(), "agents have names");
}

#[test]
fn an_unknown_reference_is_not_found_and_an_ambiguous_one_an_invalid_argument() {
    let missing = resolve_agent("nobody", &fleet()).unwrap_err();
    assert!(missing.downcast_ref::<NotFoundError>().is_some());
    let ambiguous = resolve_agent("01J5", &fleet()).unwrap_err();
    assert!(ambiguous.downcast_ref::<UsageError>().is_some());
}
