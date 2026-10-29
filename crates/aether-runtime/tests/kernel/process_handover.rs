//! What a delegated child leaves running: handed to its live parent when it
//! completes, stopped when it is cancelled, and stopped with the root.
#![cfg(target_os = "linux")]
use super::background_tools::{
    Sibling, custom_event, detach_sleep, process_alive, wait_custom_event, wait_gone,
};
use super::*;
use aether_core::inbox::InboxKind;

/// A parent that is never scheduled, so the script drives its child alone.
fn idle_parent(h: &Harness) -> AgentId {
    idle_agent(h, None)
}

/// An agent under `parent` that is never scheduled.
fn idle_agent(h: &Harness, parent: Option<AgentId>) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                parent,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create the agent");
    agent.id
}

fn child_of(parent: AgentId, objective: &str, long_lived: bool) -> CreateAgentSpec {
    let mut s = spec(objective);
    s.parent = Some(parent);
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy::none(),
        long_lived,
        effort: Some(aether_core::model::Effort::Low),
        ..Default::default()
    });
    s
}

fn holds_pid(payload: &serde_json::Value, pid: i32) -> bool {
    payload["pids"]
        .as_array()
        .is_some_and(|pids| pids.contains(&json!(pid)))
}

/// A child that completes hands the server it detached to its parent, which
/// is told its pid and command; the server runs until the parent ends.
#[tokio::test]
async fn a_completed_childs_server_runs_until_its_parent_ends() {
    let marker_name = "handed-up.pid";
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({ "command": detach_sleep(marker_name) })),
        ScriptedClient::tool_call("complete_task", json!({ "summary": "server is up" })),
    ])
    .await;
    let sibling = Sibling::start();
    let parent = idle_parent(&h);
    let (child, _) = h
        .runtime
        .create_agent(child_of(parent, "start a server", false), Actor::user())
        .expect("create the child");

    let pid = wait_for_marker(&h.workspace_root, marker_name).await;
    let status = h.wait_until(child.id, |s| s.is_terminal()).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "{status:?}"
    );
    let handed = wait_custom_event(&h, child.id, "agent.processes_handed_up").await;
    assert!(holds_pid(&handed, pid), "the hand-up names it: {handed}");
    assert_eq!(handed["to"], json!(parent.to_string()), "{handed}");
    assert!(
        process_alive(pid),
        "the server outlives the child that started it"
    );
    assert!(
        custom_event(&h, child.id, "agent.processes_reaped").is_none(),
        "a completed child stops nothing"
    );
    let told = h
        .store
        .inbox(parent, 0)
        .expect("inbox")
        .into_iter()
        .any(|item| match item.kind {
            InboxKind::Message { content, .. } => content.iter().any(|block| {
                matches!(block, ContentBlock::Text { text }
                    if text.contains(&format!("{pid}: ")) && text.contains("sleep 300"))
            }),
            _ => false,
        });
    assert!(told, "the parent is told the pid and command it now owns");

    h.runtime
        .cancel_agent(parent, Some("done".into()), Actor::user())
        .expect("end the parent");
    let reaped = wait_custom_event(&h, parent, "agent.processes_reaped").await;
    wait_gone(&h, pid, "the handed-up server").await;
    assert!(
        holds_pid(&reaped, pid),
        "the parent's end stopped it: {reaped}"
    );
    assert!(sibling.alive(), "a process outside the tree survives");
}

/// A cancelled child hands nothing up: its server stops while the parent runs on.
#[tokio::test]
async fn a_cancelled_childs_server_stops_with_it() {
    let marker_name = "cancelled-child.pid";
    let h = harness(vec![
        ScriptedClient::tool_call("bash", json!({ "command": detach_sleep(marker_name) })),
        ScriptedClient::text("server is up"),
    ])
    .await;
    let sibling = Sibling::start();
    let parent = idle_parent(&h);
    let (child, _) = h
        .runtime
        .create_agent(
            child_of(parent, "start a server, then wait", true),
            Actor::user(),
        )
        .expect("create the child");
    h.wait_until(child.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let pid = wait_for_marker(&h.workspace_root, marker_name).await;
    assert!(process_alive(pid), "the server outlives its turn");

    h.runtime
        .cancel_agent(child.id, Some("not needed".into()), Actor::user())
        .expect("cancel the child");
    let reaped = wait_custom_event(&h, child.id, "agent.processes_reaped").await;
    wait_gone(&h, pid, "the cancelled child's server").await;

    assert!(holds_pid(&reaped, pid), "{reaped}");
    assert!(
        custom_event(&h, child.id, "agent.processes_handed_up").is_none(),
        "a cancelled child hands nothing up"
    );
    let parent_row = h.store.agent(parent).expect("read").expect("exists");
    assert!(!parent_row.status.is_terminal(), "{:?}", parent_row.status);
    assert!(sibling.alive(), "a process outside the tree survives");
}

/// Write `agent` as ended with `status`, as a daemon that died before the
/// agent's reap ran left it.
fn ended_without_reap(h: &Harness, agent: AgentId, status: AgentStatus) {
    let mut row = h.store.agent(agent).expect("read").expect("exists");
    if matches!(status, AgentStatus::Completed { .. }) {
        row.transition(AgentStatus::Ready, now()).expect("ready");
        row.transition(AgentStatus::Running, now())
            .expect("running");
    }
    row.transition(status, now()).expect("end it");
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(row)))
        .expect("write the end");
}

/// A daemon that died left processes its agents detached. The next start over
/// the same store stops a failed agent's, keeps a live agent's own for its
/// recovery, and keeps a completed child's with its live parent, whose end
/// then stops them; one of an agent it never ran is spared throughout.
#[tokio::test]
async fn a_start_after_a_crash_stops_only_what_no_live_agent_holds() {
    let h = harness(vec![]).await;
    let sibling = Sibling::start();
    let parent = idle_parent(&h);
    let child = idle_agent(&h, Some(parent));
    let failed = idle_parent(&h);
    ended_without_reap(&h, child, AgentStatus::Completed { summary: None });
    ended_without_reap(&h, failed, AgentStatus::Failed { error: "x".into() });
    let parents = Sibling::tagged(parent);
    let childs = Sibling::tagged(child);
    let faileds = Sibling::tagged(failed);
    assert!(faileds.alive(), "the leftover is running before the start");

    let restarted = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(h.store.clone()),
        |_| {},
    )
    .await;
    wait_gone(&restarted, faileds.pid(), "the failed agent's leftover").await;
    assert!(
        parents.alive(),
        "a live agent keeps its server across the restart"
    );
    assert!(
        childs.alive(),
        "a completed child's server stays with its live parent"
    );

    restarted
        .runtime
        .cancel_agent(parent, Some("done".into()), Actor::user())
        .expect("end the parent");
    wait_gone(
        &restarted,
        childs.pid(),
        "the server handed back to the parent",
    )
    .await;
    wait_gone(&restarted, parents.pid(), "the parent's own server").await;
    assert!(
        sibling.alive(),
        "a process of an agent never ran here survives"
    );
}
