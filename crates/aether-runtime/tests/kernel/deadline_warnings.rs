//! deadline warning scenarios.
use super::*;

/// The deadlines each `deadline.warning` event was for, oldest first.
fn deadline_warnings(h: &Harness, id: AgentId) -> Vec<String> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Custom { name, payload } if name == "deadline.warning" => {
                Some(payload["deadline"].as_str().expect("deadline").to_owned())
            }
            _ => None,
        })
        .collect()
}

fn inbox_warnings(h: &Harness, id: AgentId) -> Vec<String> {
    h.store
        .inbox(id, 0)
        .expect("inbox")
        .into_iter()
        .filter_map(|item| match item.kind {
            aether_core::inbox::InboxKind::Wake { reason } if reason.contains("deadline") => {
                Some(reason)
            }
            _ => None,
        })
        .collect()
}

fn set_deadline(h: &Harness, id: AgentId, at: Timestamp) {
    h.runtime
        .update_agent(
            id,
            AgentUpdate {
                deadline: Some(DeadlineUpdate::Set(at)),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("set the deadline");
}

async fn warned(h: &Harness, id: AgentId, count: usize) {
    h.wait_for("a deadline warning", || {
        deadline_warnings(h, id).len() >= count
    })
    .await;
}

/// An agent hears, in its inbox, that its deadline is near before the stop,
/// once per deadline value: an extension warns again, a child with its own
/// deadline is warned on its own, and a far deadline is not warned yet.
#[tokio::test]
async fn a_near_deadline_warns_each_agent_once_per_deadline() {
    // Each warning wakes its idle agent for one turn.
    let replies = (0..6).map(|_| ScriptedClient::text("noted")).collect();
    let h = harness(replies).await;
    let mut s = spec("stay on call");
    s.objective = None;
    s.config.as_mut().expect("config").long_lived = true;
    let (parent, _) = h
        .runtime
        .create_agent(s.clone(), Actor::user())
        .expect("parent");
    let mut child_spec = s.clone();
    child_spec.parent = Some(parent.id);
    let (child, _) = h
        .runtime
        .create_agent(child_spec, Actor::user())
        .expect("child");
    let (far, _) = h.runtime.create_agent(s, Actor::user()).expect("far");

    let first = now() + chrono::Duration::minutes(5);
    set_deadline(&h, parent.id, first);
    set_deadline(&h, child.id, first);
    set_deadline(&h, far.id, now() + chrono::Duration::hours(2));
    warned(&h, parent.id, 1).await;
    warned(&h, child.id, 1).await;

    let notes = inbox_warnings(&h, parent.id);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("5 minutes left before your deadline")
            && notes[0].contains(" as of ")
            && notes[0].contains("commit what works now")
            && notes[0].contains("deadline_minutes_left"),
        "{}",
        notes[0]
    );
    assert_eq!(
        inbox_warnings(&h, child.id).len(),
        1,
        "the child hears its own"
    );
    // Several ticks pass: still one warning each, and nobody is stopped.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(deadline_warnings(&h, parent.id), vec![first.to_rfc3339()]);
    assert_eq!(deadline_warnings(&h, child.id), vec![first.to_rfc3339()]);
    assert!(
        deadline_warnings(&h, far.id).is_empty(),
        "two hours out is not near"
    );
    for id in [parent.id, child.id, far.id] {
        let agent = h.store.agent(id).expect("read").expect("agent");
        assert!(!agent.status.is_terminal(), "a warning must not stop {id}");
    }

    // An extension is a new deadline, warned about once it is near in turn.
    let extended = now() + chrono::Duration::minutes(8);
    set_deadline(&h, parent.id, extended);
    warned(&h, parent.id, 2).await;
    assert_eq!(
        deadline_warnings(&h, parent.id),
        vec![first.to_rfc3339(), extended.to_rfc3339()]
    );
    let notes = inbox_warnings(&h, parent.id);
    assert!(notes[1].starts_with("8 minutes left"), "{notes:?}");
    assert_eq!(
        deadline_warnings(&h, child.id),
        vec![first.to_rfc3339()],
        "only the parent moved"
    );
}

/// A wait for something other than the inbox (here, one sender) is not cut
/// short: the warning waits in the inbox and no turn is spent on it.
#[tokio::test]
async fn a_warning_does_not_end_a_wait_for_a_named_sender() {
    let h = harness(vec![]).await;
    let mut idle = spec("stay on call");
    idle.objective = None;
    idle.config.as_mut().expect("config").long_lived = true;
    let (sender, _) = h.runtime.create_agent(idle, Actor::user()).expect("sender");
    h.client.push(ScriptedClient::tool_call(
        "wait_for",
        json!({"message_from": sender.id.to_string()}),
    ));
    let (waiter, _) = h
        .runtime
        .create_agent(spec("wait for the sender"), Actor::user())
        .expect("waiter");
    h.wait_until(waiter.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let parked = h.store.agent(waiter.id).expect("read").expect("agent");
    let requests = h.client.requests().len();

    set_deadline(&h, waiter.id, now() + chrono::Duration::minutes(5));
    warned(&h, waiter.id, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let after = h.store.agent(waiter.id).expect("read").expect("agent");
    assert_eq!(after.status, parked.status, "still waiting on the sender");
    assert_eq!(h.client.requests().len(), requests, "no turn was spent");
    assert_eq!(inbox_warnings(&h, waiter.id).len(), 1, "queued for later");
}
