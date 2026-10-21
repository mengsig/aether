use super::*;
use aether_core::agent::Agent;
use aether_core::inbox::InboxKind;
use aether_core::lease::Lease;
use aether_core::tool::ToolCall;

async fn fixture() -> (Arc<Runtime>, tempfile::TempDir, Agent) {
    let (runtime, dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown().await;
    let (mut agent, _) = runtime
        .create_agent(
            crate::CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    agent.status = AgentStatus::Running;
    runtime
        .apply(
            Commit::new()
                .event(
                    EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 })
                        .agent(agent.id)
                        .run(agent.run),
                )
                .mutate(Mutation::UpsertAgent(agent.clone()))
                .mutate(Mutation::SetLease(Lease::new(
                    agent.id,
                    "dead-daemon",
                    1,
                    now(),
                    std::time::Duration::ZERO,
                ))),
        )
        .expect("interrupted turn");
    (runtime, dir, agent)
}

fn request(
    runtime: &Runtime,
    agent: &Agent,
    name: &str,
    side_effect: SideEffectClass,
) -> ToolCallId {
    request_holding(runtime, agent, name, side_effect, Vec::new())
}

fn request_holding(
    runtime: &Runtime,
    agent: &Agent,
    name: &str,
    side_effect: SideEffectClass,
    resources: Vec<ResourceKey>,
) -> ToolCallId {
    let call = ToolCall {
        id: ToolCallId::new(),
        provider_call_id: "provider".into(),
        name: name.into(),
        input: serde_json::json!({}),
        agent_id: agent.id,
        task_id: None,
        invocation_id: None,
        resources,
        idempotency_key: None,
        requested_at: now(),
    };
    let id = call.id;
    runtime
        .apply(
            Commit::new()
                .event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ToolRequested { call, side_effect },
                    )
                    .agent(agent.id)
                    .run(agent.run),
                )
                .event(
                    EventDraft::new(Actor::System, EventKind::ToolStarted { call: id })
                        .agent(agent.id)
                        .run(agent.run),
                ),
        )
        .expect("tool in flight");
    id
}

fn boot(runtime: &Runtime, id: &str) {
    runtime
        .apply(Commit::new().mutate(Mutation::SetMeta {
            key: BOOT_ID_META.into(),
            value: id.into(),
        }))
        .expect("previous daemon boot");
}

fn acquire(runtime: &Runtime, id: AgentId) -> ResourceKey {
    let key = ResourceKey::repo("/repo/recovery");
    runtime
        .apply(
            Commit::new().event(
                EventDraft::new(
                    Actor::System,
                    EventKind::ResourceAcquired {
                        keys: vec![key.clone()],
                    },
                )
                .agent(id)
                .run(runtime.store.agent(id).unwrap().unwrap().run),
            ),
        )
        .expect("resource acquisition");
    key
}

#[tokio::test]
async fn native_reads_and_waits_retry_but_native_writes_and_unknown_mcp_require_inspection() {
    use SideEffectClass::*;
    let (runtime, _dir, agent) = fixture().await;
    for (name, recorded, expected) in [
        ("Read", ReadOnly, RecoveryClass::Retry),
        ("Read", Mutation, RecoveryClass::Retry),
        ("Grep", ReadOnly, RecoveryClass::Retry),
        ("Glob", ReadOnly, RecoveryClass::Retry),
        ("LS", ReadOnly, RecoveryClass::Retry),
        ("WebFetch", ReadOnly, RecoveryClass::Retry),
        ("WebSearch", ReadOnly, RecoveryClass::Retry),
        ("TodoWrite", ReadOnly, RecoveryClass::Retry),
        ("Bash", Mutation, RecoveryClass::Inspect),
        ("Bash", ReadOnly, RecoveryClass::Inspect),
        ("bash", Mutation, RecoveryClass::Inspect),
        ("Write", IdempotentMutation, RecoveryClass::Inspect),
        ("Edit", IdempotentMutation, RecoveryClass::Inspect),
        ("MultiEdit", IdempotentMutation, RecoveryClass::Inspect),
        ("NotebookEdit", IdempotentMutation, RecoveryClass::Inspect),
        ("mcp__aether__wait_for", Mutation, RecoveryClass::Retry),
        ("mcp__aether.wait_for", Mutation, RecoveryClass::Retry),
        ("aether__wait_for", Mutation, RecoveryClass::Retry),
        ("aether.wait_for", Mutation, RecoveryClass::Retry),
        ("mcp__aether__delegate", ReadOnly, RecoveryClass::Retry),
        (
            "mcp__aether__integrate_workspace",
            ReadOnly,
            RecoveryClass::Inspect,
        ),
        ("mcp__aether__unknown", ReadOnly, RecoveryClass::Inspect),
        ("plugin_read", ReadOnly, RecoveryClass::Retry),
        (
            "plugin_idempotent",
            IdempotentMutation,
            RecoveryClass::Retry,
        ),
        ("plugin_mutation", Mutation, RecoveryClass::Inspect),
        ("plugin_external", External, RecoveryClass::Inspect),
        // The event preserves the class at execution time, even if a
        // registered tool's descriptor has changed since then.
        ("read_file", Mutation, RecoveryClass::Inspect),
    ] {
        let id = request(&runtime, &agent, name, recorded);
        assert_eq!(
            classify(&runtime, &agent, false)
                .expect("classification")
                .class,
            expected,
            "{name}"
        );
        // Settled by an outcome: a cancelled unsafe call is still judged.
        runtime
            .apply(
                Commit::new().event(
                    EventDraft::new(
                        Actor::System,
                        EventKind::ToolSucceeded {
                            call: id,
                            outcome: aether_core::tool::ToolOutcome::ok("settled"),
                        },
                    )
                    .agent(agent.id)
                    .run(agent.run),
                ),
            )
            .expect("settle test call");
    }
}

#[tokio::test]
async fn reboot_retries_mutations_with_a_partial_effect_wake_and_releases_keys() {
    let (runtime, _dir, agent) = fixture().await;
    boot(&runtime, "previous-boot");
    let call = request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
    let key = acquire(&runtime, agent.id);
    recover_with_boot_id(&runtime, Some("current-boot")).expect("recover");
    assert_eq!(
        runtime.store.agent(agent.id).unwrap().unwrap().status,
        AgentStatus::Ready
    );
    assert!(runtime.store.lease(agent.id).unwrap().is_none());
    assert!(runtime.resource_holdings().is_empty());
    assert!(
        unreleased_keys(&runtime, agent.id).unwrap().is_empty(),
        "{key:?}"
    );
    let inbox = runtime.store.inbox(agent.id, 0).unwrap();
    let detail = inbox
        .iter()
        .find_map(|item| match &item.kind {
            InboxKind::Wake { reason } => Some(reason),
            _ => None,
        })
        .expect("retry wake");
    for text in [
        "host restarted",
        "Bash",
        &call.to_string(),
        "killed",
        "partial",
        "re-check state",
        "external services",
    ] {
        assert!(detail.contains(text), "{detail}");
    }
    let events = runtime
        .store
        .events(0, 100, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::RecoveryClassified {
            class: RecoveryClass::Retry,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolCancelled { call: id } if id == call))
    );
    assert_eq!(
        runtime.store.meta(BOOT_ID_META).unwrap().as_deref(),
        Some("current-boot")
    );
    recover_with_boot_id(&runtime, Some("current-boot")).expect("second restart");
    assert_eq!(
        runtime.store.inbox(agent.id, 0).unwrap().len(),
        inbox.len(),
        "no duplicate retry wake"
    );
}

#[tokio::test]
async fn same_boot_empty_boot_and_unavailable_boot_keep_mutations_quarantined() {
    for (previous, current) in [("boot", Some("boot")), ("", Some("boot")), ("boot", None)] {
        let (runtime, _dir, agent) = fixture().await;
        boot(&runtime, previous);
        request(
            &runtime,
            &agent,
            "Edit",
            SideEffectClass::IdempotentMutation,
        );
        let key = acquire(&runtime, agent.id);
        recover_with_boot_id(&runtime, current).expect("recover");
        assert!(matches!(
            runtime.store.agent(agent.id).unwrap().unwrap().status,
            AgentStatus::Blocked {
                reason: BlockReason::Inspection { .. }
            }
        ));
        assert!(
            runtime
                .resource_holdings()
                .iter()
                .any(|held| held.agent == agent.id && held.keys.contains(&key) && held.quarantined)
        );
        assert!(runtime.store.inbox(agent.id, 0).unwrap().is_empty());
        assert_eq!(
            runtime.store.meta(BOOT_ID_META).unwrap().as_deref(),
            Some(current.unwrap_or_default())
        );
    }
}

#[tokio::test]
async fn previously_inspected_agents_keep_keys_on_same_boot_and_retry_after_reboot() {
    for rebooted in [false, true] {
        let (runtime, _dir, mut agent) = fixture().await;
        boot(&runtime, "old-boot");
        let call = request(&runtime, &agent, "Bash", SideEffectClass::Mutation);
        let key = acquire(&runtime, agent.id);
        agent.status = AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: format!(
                    "mutating tool calls were in flight when the daemon stopped: Bash ({call})"
                ),
            },
        };
        agent.paused = true;
        runtime
            .apply(
                Commit::new()
                    .event(
                        EventDraft::new(Actor::System, EventKind::ToolCancelled { call })
                            .agent(agent.id)
                            .run(agent.run),
                    )
                    .mutate(Mutation::UpsertAgent(agent.clone()))
                    .mutate(Mutation::ClearLease(agent.id)),
            )
            .expect("legacy inspection");
        recover_with_boot_id(
            &runtime,
            Some(if rebooted { "new-boot" } else { "old-boot" }),
        )
        .expect("restart");
        let recovered = runtime.store.agent(agent.id).unwrap().unwrap();
        assert!(recovered.paused, "operator pause survives recovery");
        if rebooted {
            assert_eq!(recovered.status, AgentStatus::Ready);
            assert!(runtime.resource_holdings().is_empty());
            assert!(unreleased_keys(&runtime, agent.id).unwrap().is_empty());
            assert!(runtime.store.inbox(agent.id, 0).unwrap().iter().any(|item| matches!(&item.kind,
                InboxKind::Wake { reason } if reason.contains("Bash") && reason.contains(&call.to_string()) && reason.contains("partial"))));
        } else {
            assert_eq!(recovered.status, agent.status);
            assert_eq!(unreleased_keys(&runtime, agent.id).unwrap(), vec![key]);
            assert_eq!(runtime.resource_holdings().len(), 1);
            assert!(runtime.store.inbox(agent.id, 0).unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn cancelled_unsafe_calls_stay_quarantined_on_the_same_boot_and_retry_after_reboot() {
    for rebooted in [false, true] {
        let (runtime, _dir, agent) = fixture().await;
        boot(&runtime, "old-boot");
        let key = ResourceKey::repo("/repo/cancelled");
        let call = request_holding(
            &runtime,
            &agent,
            "Bash",
            SideEffectClass::Mutation,
            vec![key.clone()],
        );
        runtime
            .apply(
                Commit::new().event(
                    EventDraft::new(Actor::System, EventKind::ToolCancelled { call })
                        .agent(agent.id)
                        .run(agent.run),
                ),
            )
            .expect("cancelled before the daemon stopped");
        recover_with_boot_id(
            &runtime,
            Some(if rebooted { "new-boot" } else { "old-boot" }),
        )
        .expect("recover");
        let recovered = runtime.store.agent(agent.id).unwrap().unwrap();
        let events = runtime
            .store
            .events(0, 100, &EventFilter::own_events(agent.id))
            .unwrap();
        let released = events.iter().any(|event| {
            matches!(&event.kind,
            EventKind::ResourceReleased { keys } if keys.contains(&key))
        });
        let wake = runtime
            .store
            .inbox(agent.id, 0)
            .unwrap()
            .into_iter()
            .find_map(|item| match item.kind {
                InboxKind::Wake { reason } => Some(reason),
                _ => None,
            });
        if rebooted {
            assert_eq!(recovered.status, AgentStatus::Ready);
            assert!(released, "cancelled call's key released");
            assert!(runtime.resource_holdings().is_empty());
            let wake = wake.expect("retry wake");
            assert!(
                wake.contains("host restarted") && wake.contains(&call.to_string()),
                "{wake}"
            );
        } else {
            let AgentStatus::Blocked {
                reason: BlockReason::Inspection { detail },
            } = &recovered.status
            else {
                panic!("expected inspection, got {:?}", recovered.status);
            };
            assert!(detail.contains(&call.to_string()), "{detail}");
            assert!(!released);
            assert!(
                runtime
                    .resource_holdings()
                    .iter()
                    .any(|held| held.agent == agent.id
                        && held.keys.contains(&key)
                        && held.quarantined)
            );
            assert_eq!(unreleased_keys(&runtime, agent.id).unwrap(), vec![key]);
            assert!(wake.is_none());
        }
    }
}

#[tokio::test]
async fn running_turns_without_events_retry_and_idle_ready_turns_resume() {
    let (runtime, _dir, mut agent) = fixture().await;
    assert_eq!(
        classify(&runtime, &agent, false).unwrap().class,
        RecoveryClass::Retry
    );
    agent.status = AgentStatus::Ready;
    assert_eq!(
        classify(&runtime, &agent, false).unwrap().class,
        RecoveryClass::Resume
    );
    let call = ToolCallId::new();
    runtime
        .apply(
            Commit::new().event(
                EventDraft::new(Actor::System, EventKind::ToolStarted { call })
                    .agent(agent.id)
                    .run(agent.run),
            ),
        )
        .expect("legacy unnamed started call");
    assert_eq!(
        classify(&runtime, &agent, false).unwrap().class,
        RecoveryClass::Inspect
    );
    assert_eq!(
        classify(&runtime, &agent, true).unwrap().class,
        RecoveryClass::Retry
    );
}

#[test]
fn detecting_a_reboot_requires_two_known_boot_ids() {
    assert!(host_restarted(Some("old"), Some("new")));
    for (previous, current) in [
        (Some("same"), Some("same")),
        (None, Some("current")),
        (Some("previous"), None),
        (Some(""), Some("current")),
        (Some("previous"), Some("")),
        (None, None),
    ] {
        assert!(!host_restarted(previous, current));
    }
}

#[tokio::test]
async fn recovery_carries_an_interruption_correction_only_for_in_flight_tools() {
    let (runtime, _dir, agent) = fixture().await;
    assert!(
        crate::tool_interruption::resume_notes(&runtime, agent.id)
            .unwrap()
            .is_empty()
    );
    let call = request(&runtime, &agent, "Read", SideEffectClass::ReadOnly);
    recover(&runtime).unwrap();
    let note = crate::tool_interruption::resume_notes(&runtime, agent.id).unwrap();
    assert!(note.contains("Read"));
    assert!(note.contains("recovery.classified"));
    assert!(note.contains("not denied"));
    assert!(note.contains("Re-run"));
    let events = runtime
        .store
        .events(0, 1000, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ToolCancelled { call: id } if id == call))
    );
    recover(&runtime).unwrap();
    assert_eq!(
        crate::tool_interruption::resume_notes(&runtime, agent.id).unwrap(),
        note
    );

    let (runtime, _dir, agent) = fixture().await;
    recover(&runtime).unwrap();
    assert!(
        crate::tool_interruption::resume_notes(&runtime, agent.id)
            .unwrap()
            .is_empty()
    );
}

mod corrupt;
