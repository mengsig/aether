use super::*;

fn wake_reason(runtime: &Runtime, agent: AgentId) -> Option<String> {
    runtime
        .store
        .inbox(agent, 0)
        .unwrap()
        .into_iter()
        .find_map(|item| match item.kind {
            InboxKind::Wake { reason } => Some(reason),
            _ => None,
        })
}

// Only Linux finds the stopped command's processes again to stop them.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_headless_agent_resumes_after_a_restart_told_its_calls_have_an_unknown_outcome() {
    let (runtime, _dir, agent) = headless_fixture().await;
    boot(&runtime, "boot");
    let call = request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    let key = acquire(&runtime, agent.id);
    recover_with_boot_id(&runtime, Some("boot")).expect("recover");

    let row = runtime.store.agent(agent.id).unwrap().unwrap();
    assert_eq!(row.status, AgentStatus::Ready, "not parked for inspection");
    assert!(runtime.store.lease(agent.id).unwrap().is_none());
    assert!(
        runtime.resource_holdings().is_empty(),
        "nothing quarantined"
    );
    assert!(
        unreleased_keys(&runtime, agent.id).unwrap().is_empty(),
        "{key:?} is released in the log"
    );
    let detail = wake_reason(&runtime, agent.id).expect("retry wake");
    for text in [
        "cut short",
        "Bash",
        &call.to_string(),
        "outcome is unknown",
        "check the state before repeating",
    ] {
        assert!(detail.contains(text), "{detail}");
    }
    let note = crate::tool_interruption::resume_notes(&runtime, agent.id).unwrap();
    assert!(note.contains("outcome is unknown"), "{note}");

    let events = runtime
        .store
        .events(0, 100, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        EventKind::AgentResumed { reason, .. } if reason == HEADLESS_RESUME
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::AgentBlocked { .. }))
    );

    let wakes = runtime.store.inbox(agent.id, 0).unwrap().len();
    recover_with_boot_id(&runtime, Some("boot")).expect("second restart");
    assert_eq!(
        runtime.store.inbox(agent.id, 0).unwrap().len(),
        wakes,
        "no duplicate wake"
    );
}

fn assert_parked(runtime: &Runtime, agent: AgentId) {
    let row = runtime.store.agent(agent).unwrap().unwrap();
    assert!(
        matches!(
            row.status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ),
        "{:?}",
        row.status
    );
    assert!(wake_reason(runtime, agent).is_none(), "no retry wake");
}

/// A live daemon recovering a lost turn stops none of its processes, so a
/// retry could overlap the command still running: the agent waits instead.
#[tokio::test]
async fn a_headless_agent_waits_when_its_command_may_still_run() {
    let (runtime, _dir, agent) = headless_fixture().await;
    request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    let key = acquire(&runtime, agent.id);
    let lease = runtime.store.lease(agent.id).unwrap().expect("lease");
    let recovered = recover_leases(&runtime, vec![lease], Restart::Live, None).expect("recover");
    assert_eq!(recovered.quarantined, vec![agent.id]);
    assert_parked(&runtime, agent.id);
    assert_eq!(unreleased_keys(&runtime, agent.id).unwrap(), vec![key]);
}

/// A start that could not prove a headless agent's interrupted shell call
/// stopped, nor another's, parks the agent rather than resume it beside it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_headless_agent_waits_when_the_start_could_not_prove_its_call_stopped() {
    let (runtime, _dir, agent) = headless_fixture().await;
    let call = request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    let key = acquire(&runtime, agent.id);
    let restarted = Runtime::start_with_engines(
        crate::RuntimeConfig {
            holder: "restarted".into(),
            unstopped_calls: std::collections::HashSet::from([agent.id]),
            ..Default::default()
        },
        runtime.store.clone(),
        runtime.models.clone(),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("restart");
    restarted.shutdown().await;

    assert_parked(&restarted, agent.id);
    assert_eq!(unreleased_keys(&restarted, agent.id).unwrap(), vec![key]);
    let events = restarted
        .store
        .events(0, 100, &EventFilter::own_events(agent.id))
        .unwrap();
    let blocked = events.iter().find_map(|event| match &event.kind {
        EventKind::AgentBlocked { reason, .. } => Some(reason.clone()),
        _ => None,
    });
    let detail = blocked.expect("blocked for inspection");
    assert!(detail.contains("could not prove"), "{detail}");
    assert!(detail.contains(&call.to_string()), "{detail}");
}

#[tokio::test]
async fn a_restarted_agent_keeps_saying_whether_a_person_watches_it() {
    for (stored, successor) in [(Some(true), Some(true)), (None, Some(false))] {
        let (runtime, _dir, agent) = fixture().await;
        let mut failed = store_as_legacy(&runtime, &agent);
        failed.config.interactive = stored;
        failed.status = AgentStatus::Failed {
            error: "stopped".into(),
        };
        runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(failed)))
            .expect("failed agent");
        let next = runtime
            .restart_agent(agent.id, Actor::user())
            .expect("restart");
        assert_eq!(next.config.interactive, successor, "after {stored:?}");
        let row = runtime.store.agent(next.id).unwrap().unwrap();
        assert_eq!(row.config.interactive, successor, "stored as returned");
    }
}

#[tokio::test]
async fn an_interactive_agent_still_waits_for_inspection() {
    let (runtime, _dir, agent) = fixture().await;
    boot(&runtime, "boot");
    request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    recover_with_boot_id(&runtime, Some("boot")).expect("recover");
    let row = runtime.store.agent(agent.id).unwrap().unwrap();
    assert!(
        matches!(
            row.status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ),
        "{:?}",
        row.status
    );
    assert!(wake_reason(&runtime, agent.id).is_none());
}

#[tokio::test]
async fn a_child_of_an_interactive_agent_is_interactive_too() {
    let (runtime, _dir, parent) = fixture().await;
    let (child, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent.id),
                start: false,
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    assert_eq!(parent.config.interactive, Some(true));
    assert_eq!(
        child.config.interactive,
        Some(true),
        "a child inherits its parent's"
    );
}

/// A row stored before `interactive` existed, as the upgraded daemon reads it.
fn store_as_legacy(runtime: &Runtime, agent: &Agent) -> Agent {
    let mut row = serde_json::to_value(agent).unwrap();
    row["config"].as_object_mut().unwrap().remove("interactive");
    let legacy: Agent = serde_json::from_value(row).unwrap();
    assert_eq!(legacy.config.interactive, None, "an old row cannot say");
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(legacy.clone())))
        .expect("legacy row");
    legacy
}

#[tokio::test]
async fn an_agent_stored_before_the_upgrade_still_waits_for_inspection() {
    let (runtime, _dir, agent) = headless_fixture().await;
    let agent = store_as_legacy(&runtime, &agent);
    boot(&runtime, "boot");
    request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    let key = acquire(&runtime, agent.id);
    recover_with_boot_id(&runtime, Some("boot")).expect("recover");
    let row = runtime.store.agent(agent.id).unwrap().unwrap();
    assert!(
        matches!(
            row.status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ),
        "{:?}",
        row.status
    );
    assert_eq!(unreleased_keys(&runtime, agent.id).unwrap(), vec![key]);
    assert!(wake_reason(&runtime, agent.id).is_none());
}

#[tokio::test]
async fn a_new_child_of_an_agent_stored_before_the_upgrade_is_headless() {
    let (runtime, _dir, parent) = fixture().await;
    let parent = store_as_legacy(&runtime, &parent);
    let (child, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                parent: Some(parent.id),
                start: false,
                // As delegation sends it: the parent's own config, cloned.
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..parent.config.clone()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    assert_eq!(child.config.interactive, Some(false));
}
