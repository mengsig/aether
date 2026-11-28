//! Stable native system context and incremental user state.
use super::*;
#[tokio::test]
async fn resumed_turns_update_status_without_changing_the_system_prefix() {
    let h = harness(Reply::OneTurn).await;
    let id = chat_agent(&h);
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    for (index, message) in ["first request sentinel", "second request sentinel"]
        .into_iter()
        .enumerate()
    {
        h.runtime
            .send_message(id, vec![ContentBlock::text(message)], Actor::user())
            .expect("send");
        wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns >= (index + 1) as u32 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
    }
    let systems = h.systems.lock().unwrap().clone();
    let prompts = h.prompts.lock().unwrap().clone();
    assert_eq!(systems.len(), 2);
    assert_eq!(
        systems[0], systems[1],
        "ordinary status updates must preserve the cached prefix"
    );
    assert!(
        !systems[0].contains("Scope:"),
        "per-agent scope belongs in the user turn, not the cached prefix"
    );
    assert!(
        systems[0].is_empty(),
        "ordinary turns must preserve the native system instructions"
    );
    assert!(
        prompts[0].contains("Scope:"),
        "the agent is still told its scope, in the user turn"
    );
    assert!(prompts[0].contains("Turn 1."));
    assert!(prompts[1].contains("Turn 2."));
    assert!(prompts[1].contains("second request sentinel"));
    assert!(
        !prompts[1].contains("first request sentinel"),
        "do not resend old conversation as a new message"
    );
}

/// Claude Code caches `--system-prompt` machine-wide, so a byte that
/// varies per agent costs a cache write for the whole block every session.
/// Two agents on one daemon build must therefore be handed identical system
/// bytes, with everything that distinguishes them in the user turn.
#[tokio::test]
async fn agents_share_one_system_prefix_and_differ_only_in_the_user_turn() {
    let h = harness(Reply::OneTurn).await;
    // The third agent works in its own directory: a per-workspace byte in the
    // prefix (a path, a cwd) is what would reintroduce the bug in production,
    // and two agents sharing one workspace cannot catch it.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let elsewhere_root = elsewhere.path().canonicalize().expect("canonical");
    let mut tasks = Vec::new();
    for (name, objective, workspace) in [
        ("alpha-worker", "migrate the ledger sentinel", None),
        ("beta-worker", "audit the invoice sentinel", None),
        (
            "gamma-worker",
            "reconcile the payroll sentinel",
            Some(elsewhere_root.clone()),
        ),
    ] {
        let (agent, _) = h
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "assistant".into(),
                    name: Some(name.into()),
                    objective: Some(objective.into()),
                    model: Some(ModelRef::new("claude-code", "haiku")),
                    start: true,
                    scope: workspace.map(|root| aether_core::agent::Scope {
                        workspace_root: Some(root),
                        ..Default::default()
                    }),
                    config: Some(AgentConfig {
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Actor::user(),
            )
            .expect("create");
        let started = wait_for(h.store.as_ref(), agent.id, |a| a.stats.turns >= 1).await;
        let task_id = started.current_task.expect("the agent was given a task");
        let task = h.store.task(task_id).expect("task").expect("exists");
        // What used to sit in `--system-prompt` and now must not.
        let moved = aether_context::render::role_task(&started, Some(&task), None);
        tasks.push((name, objective, task_id, moved));
    }

    let systems = h.systems.lock().unwrap().clone();
    let prompts = h.prompts.lock().unwrap().clone();
    let first = systems.first().expect("both agents ran a turn").clone();
    for other in &systems {
        assert_eq!(
            &first,
            other,
            "the system prompt must be byte-identical across agents and turns \
             ({} vs {} bytes)",
            first.len(),
            other.len()
        );
    }
    assert!(
        first.is_empty(),
        "the native harness owns the system instructions"
    );
    assert!(
        !prompts
            .iter()
            .any(|prompt| prompt.contains("Report a completed or failed task exactly once")),
        "the removed reporting policy must not move to the user turn"
    );
    assert!(
        !first.contains(elsewhere_root.to_string_lossy().as_ref()),
        "no workspace path may reach the prefix, or agents in different \
         directories stop sharing it"
    );
    // Pinned so a new layer cannot silently join the cached prefix: every
    // byte here is a cache write on every machine, once per daemon build.
    // Changing the policy text is a deliberate act - remeasure, then update.
    assert_eq!(
        first.len(),
        PREFIX_BYTES,
        "the cached prefix changed size; a layer joining it costs a cache \
         write per session on every agent"
    );

    // Turn counts race between two live agents, so match each agent by its
    // own markers rather than by position.
    for (name, objective, task, moved) in &tasks {
        assert!(
            moved.len() > 100,
            "the per-agent block is too small for this test to prove anything"
        );
        assert!(
            !first.contains(moved.as_str()),
            "{} per-agent bytes must be gone from the {}-byte prefix (was {})",
            moved.len(),
            first.len(),
            first.len() + moved.len() + 2
        );
        let markers = [name.to_string(), objective.to_string(), task.to_string()];
        for marker in &markers {
            assert!(
                !first.contains(marker),
                "{marker} varies per agent and must not sit in the cached prefix"
            );
        }
        assert!(
            prompts
                .iter()
                .any(|p| markers.iter().all(|marker| p.contains(marker))),
            "the agent is still told {markers:?}, in its user turn"
        );
    }
    h.runtime.shutdown().await;
}

/// An engine session retains every user turn it was sent, so hoisting a
/// per-agent layer into every turn would duplicate it in the conversation
/// instead of replacing it, the way a system prompt is replaced. Only what
/// changed since the session was last sent it goes again.
#[tokio::test]
async fn a_resumed_turn_carries_only_what_changed() {
    use aether_core::checkpoint::{Checkpoint, CheckpointDraft, CheckpointProvenance};
    use aether_core::id::CheckpointId;
    use aether_store::{Commit, Mutation};

    let h = harness(Reply::Resumable).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                name: Some("gamma-worker".into()),
                objective: Some("migrate the ledger sentinel".into()),
                model: Some(ModelRef::new("claude-code", "haiku")),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    let id = agent.id;
    let started = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    assert!(
        started.engine_session.is_some(),
        "the engine reported a session, so the next turn resumes it"
    );
    let task = h
        .store
        .task(started.current_task.expect("assigned"))
        .expect("task")
        .expect("exists");
    let role_task = aether_context::render::role_task(&started, Some(&task), None);

    let turn = |text: &'static str| {
        let h = &h;
        async move {
            let before = h
                .store
                .agent(id)
                .expect("agent")
                .expect("exists")
                .stats
                .turns;
            h.runtime
                .send_message(id, vec![ContentBlock::text(text)], Actor::user())
                .expect("send");
            wait_for(h.store.as_ref(), id, |a| {
                a.stats.turns > before && a.status.kind() == AgentStatusKind::Waiting
            })
            .await;
            h.prompts
                .lock()
                .unwrap()
                .last()
                .expect("a turn ran")
                .clone()
        }
    };

    let fresh = h.prompts.lock().unwrap()[0].clone();
    assert!(
        fresh.contains(&role_task),
        "the fresh session is told its whole assignment: {fresh}"
    );
    let resumed = turn("carry on").await;
    assert!(
        !resumed.contains("migrate the ledger sentinel") && !resumed.contains("Scope:"),
        "an unchanged assignment is already in the retained session: {resumed}"
    );
    assert!(
        resumed.contains("Turn 2.") && resumed.contains("carry on"),
        "runtime status and the new message still go every turn: {resumed}"
    );
    assert!(
        resumed.len() + role_task.len() <= fresh.len(),
        "the resumed turn saves the whole assignment block ({} vs {} bytes)",
        resumed.len(),
        fresh.len()
    );

    // Send-on-change, not send-once: what the model was told is stale the
    // moment the checkpoint is replaced, so that one goes again - once.
    h.store
        .commit(Commit::new().mutate(Mutation::PutCheckpoint(Checkpoint {
            id: CheckpointId::new(),
            agent_id: id,
            version: 1,
            draft: CheckpointDraft {
                objective: "finish the migration".into(),
                state: "four of five tables are migrated".into(),
                ..Default::default()
            },
            provenance: CheckpointProvenance {
                from_seq: 0,
                to_seq: 0,
                source_snapshot: None,
                compactor_model: None,
                previous: None,
                trigger: None,
            },
            created_at: aether_core::time::now(),
        })))
        .expect("seed checkpoint");
    let changed = turn("and now?").await;
    assert!(
        changed.contains("four of five tables are migrated"),
        "a checkpoint the session has not seen must reach the model: {changed}"
    );
    let again = turn("still here?").await;
    assert!(
        !again.contains("four of five tables are migrated"),
        "and must not be re-asserted as the newest input every turn after: {again}"
    );
    h.runtime.shutdown().await;
}

fn child_under(parent: AgentId, name: &str) -> CreateAgentSpec {
    CreateAgentSpec {
        role: "assistant".into(),
        name: Some(name.into()),
        objective: Some(format!("work for {name}")),
        parent: Some(parent),
        start: true,
        model: Some(ModelRef::new("claude-code", "haiku")),
        config: Some(AgentConfig {
            long_lived: true,
            effort: Some(aether_core::model::Effort::Low),
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The asking guidance follows the parent's stored record through real
/// context assembly: a live parent is named, a never-started run node is
/// not, and a parent finishing later reaches a retained session.
#[tokio::test]
async fn asking_guidance_follows_the_parents_ability_to_answer() {
    use aether_store::{Commit, Mutation};

    let h = harness(Reply::Resumable).await;
    let spawn =
        |spec: CreateAgentSpec| h.runtime.create_agent(spec, Actor::user()).expect("create");
    let (node, _) = spawn(CreateAgentSpec {
        role: "pipeline:run".into(),
        start: false,
        ..Default::default()
    });
    let (live, _) = spawn(CreateAgentSpec {
        role: "assistant".into(),
        name: Some("live-parent".into()),
        objective: Some("stay available".into()),
        start: true,
        model: Some(ModelRef::new("claude-code", "haiku")),
        config: Some(AgentConfig {
            long_lived: true,
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..Default::default()
    });
    wait_for(h.store.as_ref(), live.id, |a| {
        a.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let (under_node, _) = spawn(child_under(node.id, "under-node"));
    let (under_live, _) = spawn(child_under(live.id, "under-live"));
    for id in [under_node.id, under_live.id] {
        wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
    }
    let prompt_of = |name: &str| {
        let prompts = h.prompts.lock().unwrap();
        prompts
            .iter()
            .find(|p| p.contains(&format!("Name: {name}")))
            .unwrap_or_else(|| panic!("no prompt for {name}"))
            .clone()
    };
    let node_prompt = prompt_of("under-node");
    assert!(
        node_prompt.contains("nobody above you can answer"),
        "{node_prompt}"
    );
    assert!(!node_prompt.contains("send_message to"), "{node_prompt}");
    let live_prompt = prompt_of("under-live");
    assert!(
        live_prompt.contains(&format!("send_message to {}", live.id)),
        "{live_prompt}"
    );

    let mut finished = h.store.agent(live.id).unwrap().expect("parent");
    finished.status = AgentStatus::Completed { summary: None };
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(finished)))
        .expect("finish the parent");
    let before = h.prompts.lock().unwrap().len();
    h.runtime
        .send_message(
            under_live.id,
            vec![ContentBlock::text("anyone there?")],
            Actor::user(),
        )
        .expect("send");
    wait_for(h.store.as_ref(), under_live.id, |a| a.stats.turns >= 2).await;
    let resumed = h.prompts.lock().unwrap()[before..]
        .iter()
        .find(|p| p.contains("anyone there?"))
        .expect("the resumed turn")
        .clone();
    assert!(
        resumed.contains("nobody above you can answer") && !resumed.contains("send_message to"),
        "a retained session is told its parent can no longer answer: {resumed}"
    );
    h.runtime.shutdown().await;
}
