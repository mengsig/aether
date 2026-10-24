use super::*;

fn keyed(objective: &str, key: &str) -> serde_json::Value {
    json!({"model":"claude-code/haiku", "effort":"low", "role":"worker",
           "objective": objective, "wait": false, "idempotency_key": key})
}

async fn call(h: &Harness, bridge: &BridgeCredentials, input: serde_json::Value) -> (String, bool) {
    h.runtime
        .engine_tool(bridge.agent, &bridge.token, "delegate".into(), input)
        .await
        .expect("answered")
}

/// The structured payload a delegate reply carries on its last line.
fn ids(reply: &str) -> (serde_json::Value, serde_json::Value) {
    let line = reply.lines().last().expect("a reply line");
    let payload: serde_json::Value = serde_json::from_str(line).expect("structured JSON");
    (payload["agent_id"].clone(), payload["task_id"].clone())
}

#[tokio::test]
async fn a_repeated_key_returns_the_first_child_and_a_changed_request_is_refused() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");

    let (first, is_error) = call(&h, &bridge, keyed("read the docs", "docs-1")).await;
    assert!(!is_error, "{first}");
    assert!(first.contains("\"idempotency_key\":\"docs-1\""), "{first}");
    let (agent, task) = ids(&first);
    assert!(agent.is_string() && task.is_string(), "{first}");

    let (second, is_error) = call(&h, &bridge, keyed("read the docs", "docs-1")).await;
    assert!(!is_error, "{second}");
    assert!(second.contains("\"replayed\":true"), "{second}");
    assert_eq!(ids(&second), (agent.clone(), task.clone()));
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 1);

    let (changed, is_error) = call(&h, &bridge, keyed("read other docs", "docs-1")).await;
    assert!(
        is_error,
        "a different request under a used key ran: {changed}"
    );
    assert!(
        changed.contains("different request needs a new key"),
        "{changed}"
    );
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 1);

    let (fresh, is_error) = call(&h, &bridge, keyed("read other docs", "docs-2")).await;
    assert!(!is_error, "{fresh}");
    assert_ne!(ids(&fresh).0, agent, "a new key creates a new child");
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 2);

    // `wait` is not part of the comparison, and a replay still waits on a
    // child that has not finished.
    let mut waiting = keyed("read the docs", "docs-1");
    waiting["wait"] = json!(true);
    let (third, is_error) = call(&h, &bridge, waiting).await;
    assert!(!is_error, "{third}");
    assert!(
        third.contains("You will be woken when it finishes"),
        "{third}"
    );
    assert_eq!(ids(&third), (agent, task));
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 2);
    h.runtime.shutdown().await;
}

/// A child can report its task done while its turn is still landing the
/// final receipt: a waiting replay must still wait on that task.
#[tokio::test]
async fn a_waiting_replay_of_a_reported_child_still_waits_on_its_task() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");

    let (first, is_error) = call(&h, &bridge, keyed("read the docs", "docs-1")).await;
    assert!(!is_error, "{first}");
    let (agent, task) = ids(&first);
    let child = h.ready.recv().await.expect("child started");
    assert_eq!(json!(child.agent), agent);
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            child.agent,
            &child.token,
            "complete_task".into(),
            json!({"summary": "done"}),
        )
        .await
        .expect("completion reply");
    assert!(!is_error, "{reply}");
    let task_id = task.as_str().expect("task id").parse().expect("task id");
    let row = h.store.task(task_id).expect("task read").expect("task row");
    assert!(row.status.is_terminal(), "{:?}", row.status);

    // The held engine keeps the child's turn open for the park grace.
    let mut waiting = keyed("read the docs", "docs-1");
    waiting["wait"] = json!(true);
    let (replay, is_error) = call(&h, &bridge, waiting).await;
    assert!(!is_error, "{replay}");
    assert!(replay.contains("\"replayed\":true"), "{replay}");
    assert!(
        replay.contains("You will be woken when it finishes"),
        "a replay skipped the wait on a turn still running: {replay}"
    );
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 1);
    h.runtime.shutdown().await;
}

/// The retry arrives while the first call is still pending: both settle on
/// one child, whichever of them committed it.
#[tokio::test]
async fn a_repeat_racing_the_pending_first_call_resolves_to_the_same_child() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");

    let ((first, first_error), (second, second_error)) = tokio::join!(
        call(&h, &bridge, keyed("survey the repo", "survey")),
        call(&h, &bridge, keyed("survey the repo", "survey")),
    );
    assert!(!first_error && !second_error, "{first}\n{second}");
    assert_eq!(ids(&first), ids(&second));
    assert_eq!(children_of(h.store.as_ref(), coordinator).len(), 1);
    h.runtime.shutdown().await;
}

/// Keys are the caller's own: another agent using the same word delegates
/// its own child.
#[tokio::test]
async fn the_same_key_from_two_agents_creates_two_children() {
    let mut h = harness(10).await;
    let one = start_coordinator(&h, None);
    let first_bridge = h.ready.recv().await.expect("first coordinator started");
    let two = start_coordinator(&h, None);
    let second_bridge = h.ready.recv().await.expect("second coordinator started");
    assert_ne!(first_bridge.agent, second_bridge.agent);

    let (a, a_error) = call(&h, &first_bridge, keyed("check", "shared-word")).await;
    let (b, b_error) = call(&h, &second_bridge, keyed("check", "shared-word")).await;
    assert!(!a_error && !b_error, "{a}\n{b}");
    assert_ne!(ids(&a).0, ids(&b).0);
    assert_eq!(children_of(h.store.as_ref(), one).len(), 1);
    assert_eq!(children_of(h.store.as_ref(), two).len(), 1);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_anything_is_created() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    for bad in ["", "has space", &"k".repeat(129)] {
        let (reply, is_error) = call(&h, &bridge, keyed("check", bad)).await;
        assert!(is_error && reply.contains("idempotency_key"), "{reply}");
    }
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}

/// A keyed call skips the tool's early scope check so its replay can come
/// first; creation still refuses a child scope wider than its parent's.
#[tokio::test]
async fn a_keyed_call_that_widens_the_scope_is_still_refused() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let mut widening = keyed("look around", "wide");
    widening["workspace_root"] = json!("/");
    let (reply, is_error) = call(&h, &bridge, widening).await;
    assert!(is_error, "a widened scope was accepted: {reply}");
    assert!(reply.contains("workspace_root"), "{reply}");
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}
