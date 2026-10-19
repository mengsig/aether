//! The notification center: a key dedupes a repost, and an answer reaches
//! the posting agent's inbox, waking it, in the commit that records it.
use super::*;
use aether_core::inbox::InboxKind;
use aether_core::notification::{
    Notification, NotificationAction, NotificationChange, NotificationKind, NotificationPost,
    NotificationState,
};
use aether_runtime::notifications::Poster;

fn question(key: &str, title: &str) -> NotificationPost {
    NotificationPost {
        kind: NotificationKind::Question,
        title: title.into(),
        body: "Release 1.3 is staged.".into(),
        key: Some(key.into()),
        link: None,
        actions: vec![
            NotificationAction {
                id: "ship".into(),
                label: "Ship it".into(),
            },
            NotificationAction {
                id: "hold".into(),
                label: "Hold".into(),
            },
        ],
        free_text: true,
    }
}

fn changes(h: &Harness) -> Vec<NotificationChange> {
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::default()
    };
    let events = h.store.events(0, 1000, &filter).unwrap();
    events
        .iter()
        .filter_map(|event| Notification::of(&event.kind).map(|(change, _)| change))
        .collect()
}

#[tokio::test]
async fn a_repost_under_its_key_updates_the_one_notification() {
    let h = harness(Vec::new()).await;
    let archon = || Poster::Service("archon".into());
    let first = h
        .runtime
        .post_notification(archon(), question("q-1", "Ship 1.3?"))
        .unwrap();
    assert_eq!(first.change, Some(NotificationChange::Posted));
    let same = h
        .runtime
        .post_notification(archon(), question("q-1", "Ship 1.3?"))
        .unwrap();
    assert_eq!(same.change, None, "an identical repost records nothing");
    let updated = h
        .runtime
        .post_notification(archon(), question("q-1", "Ship 1.3 today?"))
        .unwrap();
    assert_eq!(updated.change, Some(NotificationChange::Updated));
    assert_eq!(updated.notification.id, first.notification.id);
    let all = h.runtime.notifications(&Default::default()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "Ship 1.3 today?");
    assert_eq!(
        changes(&h),
        [NotificationChange::Posted, NotificationChange::Updated]
    );

    let bad = h
        .runtime
        .post_notification(Poster::Service("Not A Name".into()), question("q", "t"));
    assert!(
        matches!(bad, Err(aether_runtime::RuntimeError::Invalid(_))),
        "{bad:?}"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_service_answer_is_recorded_as_an_event_and_resolves() {
    let h = harness(Vec::new()).await;
    let posted = h
        .runtime
        .post_notification(Poster::Service("archon".into()), question("q-2", "Merge?"))
        .unwrap()
        .notification;
    let read = h
        .runtime
        .mark_notification_read(posted.id, Actor::user())
        .unwrap();
    assert_eq!(read.state, NotificationState::Read);
    let answered = h
        .runtime
        .answer_notification(posted.id, Some("hold".into()), None, Actor::user())
        .unwrap();
    assert_eq!(answered.delivered_to, None, "a service reads the event");
    assert_eq!(answered.notification.state, NotificationState::Resolved);
    let again = h
        .runtime
        .answer_notification(posted.id, Some("ship".into()), None, Actor::user());
    assert!(again.is_err(), "one answer only");
    let resolved = h
        .runtime
        .resolve_notification_by_key("archon", "q-2", Actor::user())
        .unwrap();
    assert_eq!(
        resolved, answered.notification,
        "resolving twice changes nothing"
    );
    assert_eq!(
        changes(&h),
        [
            NotificationChange::Posted,
            NotificationChange::Read,
            NotificationChange::Answered,
        ]
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_answer_wakes_the_posting_agent_with_the_choice_in_its_inbox() {
    let h = harness(vec![
        ScriptedClient::text("asked"),
        ScriptedClient::text("shipping"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("ask before shipping")
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_for("the first turn", || h.client.requests().len() == 1)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let posted = h
        .runtime
        .post_notification(Poster::Agent(agent.id), question("rel-1.3", "Ship 1.3?"))
        .unwrap()
        .notification;
    assert_eq!(posted.source, agent.id.to_string());
    let answered = h
        .runtime
        .answer_notification(
            posted.id,
            Some("ship".into()),
            Some("after the docs land".into()),
            Actor::user(),
        )
        .unwrap();
    assert_eq!(answered.delivered_to, Some(agent.id));
    let inbox = h.store.inbox(agent.id, 0).unwrap();
    let delivered = inbox.iter().any(|item| {
        matches!(&item.kind, InboxKind::ExternalTrigger { source, payload, .. }
            if source == "notification" && payload["action"] == "ship"
                && payload["label"] == "Ship it")
    });
    assert!(delivered, "{inbox:?}");

    h.wait_for("the answer's turn", || h.client.requests().len() == 2)
        .await;
    let request = serde_json::to_string(&h.client.requests()[1].messages).unwrap();
    assert!(request.contains("after the docs land"), "{request}");
    let events = h
        .store
        .events(0, 1000, &EventFilter::own_events(agent.id))
        .unwrap();
    assert!(
        events.iter().any(|e| matches!(
            Notification::of(&e.kind),
            Some((NotificationChange::Answered, _))
        )),
        "an agent's notification events are filed under it"
    );
    h.runtime.shutdown().await;
}

/// The `notify` tool posts under the agent's id and resolves by its key; a
/// post over a limit fails the call, not the turn.
#[tokio::test]
async fn an_agent_posts_and_resolves_through_the_notify_tool() {
    let h = harness(vec![
        ScriptedClient::tool_call(
            "notify",
            json!({"title": "Blocked on CI", "kind": "blocker", "key": "ci"}),
        ),
        ScriptedClient::tool_call(
            "notify",
            json!({"title": "Second", "key": "ci", "link": "x".repeat(600)}),
        ),
        ScriptedClient::tool_call("notify", json!({"resolve": true, "key": "ci"})),
        ScriptedClient::text("done"),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("report the blocker")
            },
            Actor::user(),
        )
        .expect("create");
    let events = wait_until_events(h.store.as_ref(), agent.id, |events| {
        let settled = |name: &str| events.iter().filter(|e| e.type_name() == name).count();
        settled("tool.succeeded") == 2 && settled("tool.failed") == 1
    })
    .await;
    let posted = h
        .runtime
        .notifications(&Default::default())
        .unwrap()
        .pop()
        .expect("one notification");
    assert_eq!(posted.source, agent.id.to_string());
    assert_eq!(posted.kind, NotificationKind::Blocker);
    assert_eq!(
        posted.title, "Blocked on CI",
        "the refused update changed nothing"
    );
    assert_eq!(posted.state, NotificationState::Resolved);
    let refusal = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ToolFailed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("the oversized post failed");
    assert!(refusal.contains("512"), "{refusal}");
    h.runtime.shutdown().await;
}
