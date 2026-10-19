use crate::id::ApprovalId;
use crate::tool::ResourceKey;

use super::*;

#[test]
fn leaf_config_defaults_off_and_round_trips() {
    let old: AgentConfig = serde_json::from_str("{}").unwrap();
    assert!(!old.leaf);
    assert_eq!(old, AgentConfig::default());
    let leaf = AgentConfig { leaf: true, ..old };
    let value = serde_json::to_value(&leaf).unwrap();
    assert_eq!(value["leaf"], true);
    assert_eq!(serde_json::from_value::<AgentConfig>(value).unwrap(), leaf);
    assert!(serde_json::from_str::<AgentConfig>(r#"{"leaf":"true"}"#).is_err());
}

#[test]
fn host_context_defaults_full_off_the_wire_and_lean_round_trips() {
    let old: AgentConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(old.host_context, HostContext::Full);
    let full = serde_json::to_value(&old).unwrap();
    assert!(full.get("host_context").is_none(), "{full}");
    let lean = AgentConfig {
        host_context: HostContext::Lean,
        ..old
    };
    let value = serde_json::to_value(&lean).unwrap();
    assert_eq!(value["host_context"], "lean");
    assert_eq!(serde_json::from_value::<AgentConfig>(value).unwrap(), lean);
    assert!(serde_json::from_str::<AgentConfig>(r#"{"host_context":"minimal"}"#).is_err());
}

fn agent_with(deadline: Option<Timestamp>) -> Agent {
    let at = crate::time::now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("w"),
        name: "w".into(),
        scope: Scope::default(),
        model: crate::model::ModelRef::new("mock", "m"),
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
        config: AgentConfig {
            deadline,
            ..Default::default()
        },
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

#[test]
fn engine_cost_baseline_requires_matching_session_history() {
    let mut agent = agent_with(None);
    assert_eq!(agent.engine_session_usage(), Usage::default());
    agent.engine_session = Some("current".into());
    assert!(!agent.engine_session_usage().cost_complete);
    let total = Usage {
        cost_usd: 0.75,
        ..Usage::default()
    };
    agent.engine_usage = Some(EngineSessionUsage {
        models: None,
        session: "old".into(),
        total,
    });
    assert!(!agent.engine_session_usage().cost_complete);
    agent.engine_usage.as_mut().unwrap().session = "current".into();
    assert_eq!(agent.engine_session_usage(), total);
    agent.engine_session = None;
    assert_eq!(agent.engine_session_usage(), Usage::default());
}

#[test]
fn a_deadline_passes_on_the_instant_it_names() {
    let at = crate::time::now();
    assert_eq!(agent_with(None).deadline_passed(at), None, "no deadline");
    assert_eq!(
        agent_with(Some(at + chrono::Duration::seconds(1))).deadline_passed(at),
        None,
        "not yet"
    );
    // The instant itself counts as passed: a hard stop that needs one more
    // tick is a hard stop that can be missed.
    let reason = agent_with(Some(at))
        .deadline_passed(at)
        .expect("the instant itself");
    assert_eq!(reason, format!("deadline of {}", at.to_rfc3339()));
    assert!(
        agent_with(Some(at - chrono::Duration::seconds(1)))
            .deadline_passed(at)
            .is_some()
    );
}

/// An API-equivalent estimate is not money anyone was charged, so no cap
/// may fire on it. A subtree of nothing but estimated spend
/// is still $0 billed, and stays runnable.
#[test]
fn an_estimate_never_enforces_a_cost_budget() {
    let mut agent = agent_with(None);
    agent.config.cost_budget = Some(1.0);
    let estimated = Usage {
        input_tokens: 40_000,
        cost_usd: 0.0,
        cost_complete: false,
        estimated_cost_usd: Some(1_000.0),
        ..Default::default()
    };
    assert_eq!(
        agent.budget_exceeded(&estimated),
        None,
        "a thousand estimated dollars is still nothing billed"
    );
    assert_eq!(agent.budget_warning(&estimated), None);
    assert!(
        agent.cost_floor_note(&estimated).is_some(),
        "the cap is enforced on a floor, and says so"
    );

    // A single billed cent does fire it, which is what proves the cap works.
    let billed = Usage {
        cost_usd: 1.0,
        cost_complete: true,
        ..estimated
    };
    assert!(agent.budget_exceeded(&billed).is_some());
}

#[test]
fn deadlines_bound_open_waits_only() {
    let at = crate::time::now();
    let task = WaitCondition::Task { id: TaskId::new() };
    let bounded = task.clone().with_deadline(at);
    assert!(matches!(&bounded, WaitCondition::Any { of } if of.len() == 2));
    assert_eq!(bounded.earliest_deadline(), Some(at));
    assert_eq!(
        bounded.clone().with_deadline(at),
        bounded,
        "already bounded"
    );
    assert_eq!(WaitCondition::Inbox.with_deadline(at), WaitCondition::Inbox);
    let approval = WaitCondition::All {
        of: vec![WaitCondition::Approval {
            id: ApprovalId::new(),
        }],
    };
    assert!(approval.involves_approval());
    assert_eq!(
        approval.clone().with_deadline(at),
        approval,
        "people are not on a timer"
    );
    assert!(bounded.describe().starts_with("any of [task "));
}
use crate::time::now;

fn agent(status: AgentStatus) -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "w".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status,
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

#[test]
fn status_since_tracks_continuous_kind_and_preserves_failed_transitions() {
    let at: Timestamp = "2026-09-14T10:00:00Z".parse().unwrap();
    let later = at + chrono::Duration::minutes(4);
    let waiting = AgentStatus::Waiting {
        condition: WaitCondition::Inbox,
    };
    let mut a = agent(AgentStatus::Running);
    a.transition(waiting.clone(), at).unwrap();
    assert_eq!(a.status_since, Some(at));
    let before = a.clone();
    assert!(a.transition(waiting.clone(), later).is_err());
    assert_eq!(a, before, "invalid transitions must not mutate any fields");
    // A condition edit is not a new continuous Waiting period.
    if let AgentStatus::Waiting { condition } = &mut a.status {
        *condition = WaitCondition::Any {
            of: vec![WaitCondition::Inbox],
        };
    }
    assert_eq!(a.status_since, Some(at));
    a.transition(AgentStatus::Ready, later).unwrap();
    a.transition(AgentStatus::Running, later).unwrap();
    a.transition(waiting, later).unwrap();
    assert_eq!(a.status_since, Some(later), "ABA starts a new wait");
    a.transition(
        AgentStatus::Blocked {
            reason: BlockReason::Dependency { on: TaskId::new() },
        },
        later,
    )
    .unwrap();
    let blocked = a.status.clone();
    a.transition(blocked.clone(), at).unwrap();
    assert_eq!(
        a.status_since,
        Some(later),
        "same kind even under clock skew"
    );
    a.status_since = None;
    a.transition(blocked, later).unwrap();
    assert_eq!(a.status_since, None, "same kind cannot invent old history");
    a.transition(AgentStatus::Ready, at).unwrap();
    assert_eq!(
        a.status_since,
        Some(at),
        "record the authoritative clock even if it moved back"
    );
}

#[test]
fn status_since_is_optional_for_old_rows_and_ignored_by_old_readers() {
    let a = agent(AgentStatus::Ready);
    let mut value = serde_json::to_value(&a).unwrap();
    assert!(value.get("status_since").is_none());
    let old: Agent = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(old.status_since, None);
    value["status_since"] = serde_json::to_value(a.created_at).unwrap();
    let new: Agent = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(new.status_since, Some(a.created_at));
    #[derive(Deserialize)]
    struct OldReader {
        status: AgentStatus,
        updated_at: Timestamp,
    }
    let old_reader: OldReader = serde_json::from_value(value).unwrap();
    assert_eq!(old_reader.status, a.status);
    assert_eq!(old_reader.updated_at, a.updated_at);
    let mut unrelated = new.clone();
    unrelated.paused = true;
    unrelated.name = "renamed".into();
    unrelated.config.token_budget = Some(99);
    unrelated.stats.usage.input_tokens = 7;
    unrelated.updated_at += chrono::Duration::hours(1);
    assert_eq!(unrelated.status_since, new.status_since);
}

#[test]
fn allowed_path() {
    let mut a = agent(AgentStatus::Created);
    let t = now();
    a.transition(AgentStatus::Ready, t).expect("ready");
    a.transition(AgentStatus::Running, t).expect("running");
    a.transition(
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        },
        t,
    )
    .expect("waiting");
    a.transition(AgentStatus::Ready, t).expect("ready again");
    a.transition(AgentStatus::Running, t)
        .expect("running again");
    a.transition(AgentStatus::Completed { summary: None }, t)
        .expect("done");
    assert!(a.status.is_terminal());
}

#[test]
fn terminal_states_are_final() {
    let mut a = agent(AgentStatus::Cancelled { reason: None });
    let err = a.transition(AgentStatus::Ready, now()).unwrap_err();
    assert_eq!(err.from, "cancelled");
    assert_eq!(err.to, "ready");
    for status in [
        AgentStatus::Completed { summary: None },
        AgentStatus::Imported {
            was: AgentStatusKind::Running,
        },
    ] {
        let mut a = agent(status);
        assert!(a.transition(AgentStatus::Ready, now()).is_err());
    }
}

/// Failed is terminal, and the one terminal state a person may leave: a
/// wake puts the same agent back to Ready, and only to Ready.
#[test]
fn a_failed_agent_can_be_woken_to_ready_and_nothing_else() {
    let failed = || {
        agent(AgentStatus::Failed {
            error: "provider down".into(),
        })
    };
    assert!(failed().status.is_terminal());
    let mut a = failed();
    a.transition(AgentStatus::Ready, now()).expect("woken");
    assert!(a.is_live());
    for kind in AgentStatusKind::ALL {
        assert_eq!(
            AgentStatusKind::Failed.can_transition_to(kind),
            kind == AgentStatusKind::Ready,
            "failed -> {kind}"
        );
    }
}

#[test]
fn a_block_names_exactly_one_cause() {
    let on = TaskId::new();
    let gate = AgentStatus::Blocked {
        reason: BlockReason::Dependency { on },
    };
    let json = serde_json::to_string(&gate).expect("serialize");
    assert_eq!(
        serde_json::from_str::<AgentStatus>(&json).expect("round trip"),
        gate
    );
    let AgentStatus::Blocked { reason } = &gate else {
        panic!("{gate:?}");
    };
    assert_eq!(reason.dependency(), Some(on));
    let lease = BlockReason::Resource {
        holder: AgentId::new(),
        key: ResourceKey::file("/r/a.rs"),
    };
    assert_eq!(lease.dependency(), None, "a lease wait gates no task");
    assert!(lease.describe().contains("/r/a.rs"));
}

/// Rows written before the reason was typed still load, parked for a
/// person rather than silently resumed.
#[test]
fn a_legacy_blocked_row_becomes_an_inspection() {
    let status: AgentStatus =
        serde_json::from_str(r#"{"state":"blocked","reason":"mid-write","dependency":null}"#)
            .expect("legacy row");
    assert_eq!(
        status,
        AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: "mid-write".into()
            }
        }
    );
}

#[test]
fn created_cannot_run_directly() {
    let mut a = agent(AgentStatus::Created);
    assert!(a.transition(AgentStatus::Running, now()).is_err());
}

#[test]
fn cancel_from_any_live_state() {
    for status in [
        AgentStatus::Created,
        AgentStatus::Ready,
        AgentStatus::Running,
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        },
        AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: "look at it".into(),
            },
        },
    ] {
        let mut a = agent(status);
        a.transition(AgentStatus::Cancelled { reason: None }, now())
            .expect("cancel");
    }
}

#[test]
fn subtree_walks_every_branch_once() {
    let root = agent(AgentStatus::Ready);
    let mut a = agent(AgentStatus::Ready);
    a.parent = Some(root.id);
    let mut b = agent(AgentStatus::Ready);
    b.parent = Some(root.id);
    let mut deep = agent(AgentStatus::Ready);
    deep.parent = Some(a.id);
    let all = vec![root.clone(), a.clone(), b.clone(), deep.clone()];
    assert_eq!(
        subtree_of(&all, root.id),
        vec![root.id, a.id, b.id, deep.id]
    );
    assert_eq!(subtree_of(&all, a.id), vec![a.id, deep.id]);
    assert!(subtree_of(&all, AgentId::new()).is_empty(), "unknown root");
    // A cycle in stored links must not loop forever.
    let mut x = agent(AgentStatus::Ready);
    let mut y = agent(AgentStatus::Ready);
    x.parent = Some(y.id);
    y.parent = Some(x.id);
    assert_eq!(subtree_of(&[x.clone(), y.clone()], x.id), vec![x.id, y.id]);
}

#[test]
fn scope_narrowing() {
    let parent = Scope {
        workspace_root: Some("/repo".into()),
        network: NetworkPolicy::Denied,
        ..Default::default()
    };
    let child = Scope {
        workspace_root: Some("/repo/crates/x".into()),
        ..Default::default()
    };
    assert!(child.is_narrowing_of(&parent));
    let escaped = Scope {
        workspace_root: Some("/other".into()),
        ..Default::default()
    };
    assert!(!escaped.is_narrowing_of(&parent));
    let net = Scope {
        workspace_root: Some("/repo".into()),
        network: NetworkPolicy::Allowed,
        ..Default::default()
    };
    assert!(!net.is_narrowing_of(&parent));

    // The backend dimension: a container-confined agent cannot delegate
    // its way back onto the host.
    let contained = Scope {
        workspace_root: Some("/repo".into()),
        backend: ExecutionBackend::Container {
            image: "rust:1".into(),
        },
        ..Default::default()
    };
    assert!(contained.is_narrowing_of(&parent));
    let escaped_backend = Scope {
        workspace_root: Some("/repo".into()),
        ..Default::default()
    };
    assert!(!escaped_backend.is_narrowing_of(&contained));
    assert!(contained.is_narrowing_of(&contained));
}

#[test]
fn a_child_may_enter_a_container_but_never_leave_one() {
    let contained = ExecutionBackend::Container {
        image: "img:1".into(),
    };
    assert!(contained.is_narrowing_of(&ExecutionBackend::Host));
    assert!(contained.is_narrowing_of(&contained));
    assert!(!ExecutionBackend::Host.is_narrowing_of(&contained));
    // A different image is a different sandbox, not a narrower one.
    assert!(
        !ExecutionBackend::Container {
            image: "other:1".into()
        }
        .is_narrowing_of(&contained)
    );

    // And the whole scope refuses it, so a delegate cannot escape.
    let parent = Scope {
        workspace_root: Some("/repo".into()),
        backend: contained,
        ..Default::default()
    };
    let escaping = Scope {
        workspace_root: Some("/repo".into()),
        ..Default::default()
    };
    assert!(!escaping.is_narrowing_of(&parent));
}

#[test]
fn a_backend_round_trips_through_the_text_a_person_writes() {
    for text in ["host", "container:python:3.12-slim"] {
        let parsed: ExecutionBackend = text.parse().expect(text);
        assert_eq!(parsed.to_string(), text);
    }
    assert!("container:".parse::<ExecutionBackend>().is_err());
    assert!("vm".parse::<ExecutionBackend>().is_err());
}

struct Oracle {
    terminal: BTreeSet<TaskId>,
}

impl WaitOracle for Oracle {
    fn task_is_terminal(&self, task: TaskId) -> bool {
        self.terminal.contains(&task)
    }
    fn approval_is_resolved(&self, _: ApprovalId) -> bool {
        false
    }
    fn has_unconsumed_inbox(&self) -> bool {
        false
    }
    fn has_unconsumed_message_from(&self, _: AgentId) -> bool {
        false
    }
    fn now(&self) -> Timestamp {
        now()
    }
}

#[test]
fn wait_for_all_tasks() {
    let a = TaskId::new();
    let b = TaskId::new();
    let cond = WaitCondition::all_tasks([a, b]);
    assert!(!cond.is_satisfied(&Oracle {
        terminal: BTreeSet::from([a])
    }));
    assert!(cond.is_satisfied(&Oracle {
        terminal: BTreeSet::from([a, b])
    }));
    let any = WaitCondition::any_task([a, b]);
    assert!(any.is_satisfied(&Oracle {
        terminal: BTreeSet::from([b])
    }));
    assert_eq!(cond.task_ids(), BTreeSet::from([a, b]));
}

#[test]
fn at_least_and_nested_conditions() {
    let ids: Vec<TaskId> = (0..5).map(|_| TaskId::new()).collect();
    let two_of_five = WaitCondition::AtLeast {
        n: 2,
        of: ids
            .iter()
            .map(|id| WaitCondition::Task { id: *id })
            .collect(),
    };
    assert!(!two_of_five.is_satisfied(&Oracle {
        terminal: BTreeSet::from([ids[0]])
    }));
    assert!(two_of_five.is_satisfied(&Oracle {
        terminal: BTreeSet::from([ids[0], ids[3]])
    }));

    let deadline = now() - chrono::Duration::seconds(1);
    let either = WaitCondition::Any {
        of: vec![
            WaitCondition::Task { id: ids[4] },
            WaitCondition::Deadline { at: deadline },
        ],
    };
    assert!(either.is_satisfied(&Oracle {
        terminal: BTreeSet::new()
    }));
    assert_eq!(either.earliest_deadline(), Some(deadline));
    assert!(!WaitCondition::Any { of: vec![] }.is_satisfied(&Oracle {
        terminal: BTreeSet::new()
    }));
}

#[test]
fn money_reads_back_without_float_noise_or_losing_sub_cent_limits() {
    assert_eq!(format_usd(0.1 + 0.2), "0.3");
    assert_eq!(format_usd(0.004), "0.004");
    assert_eq!(format_usd(5.0), "5");
    assert_eq!(format_usd(0.000001), "0.000001");
    assert_eq!(format_usd(0.0000001), "0.0000001");
}

#[test]
fn a_derived_cost_limit_is_named_without_noise_in_the_reason() {
    let mut agent = agent_with(None);
    agent.config.cost_budget = Some(0.1 + 0.2);
    let spent = Usage {
        cost_usd: 1.0,
        cost_complete: true,
        ..Default::default()
    };
    let reason = agent.budget_exceeded(&spent).unwrap();
    assert!(reason.starts_with("cost budget of $0.3:"), "{reason}");
}
