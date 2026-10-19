#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Ignored requests through a whole poll: who is still refused, and the one
//! notice a refused human or operator login leaves instead of silence.
use super::notice::IGNORED_EVENT;
use super::operator_tests::{
    add_node, comment_source, fixture_operators, node, triggers, with_node,
};
use super::tests::{Fake, at, comment, comment_on, fixture, permission_calls, runtime};
use super::*;
use aether_core::{
    id::AgentId,
    note::{Note, NoteContext},
};
use aether_store::EventFilter;
use serde_json::{Value, json};

fn custom_events(rt: &Runtime, filter: EventFilter) -> Vec<EventKind> {
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..filter
    };
    let events = rt.store.events(0, 1000, &filter).unwrap();
    events.into_iter().map(|event| event.kind).collect()
}

/// The notes on `agent`'s transcript.
fn notes(rt: &Runtime, agent: AgentId) -> Vec<Note> {
    custom_events(rt, EventFilter::own_events(agent))
        .iter()
        .filter_map(Note::of)
        .collect()
}

/// The payload of every notice that found no live agent.
fn feed(rt: &Runtime) -> Vec<Value> {
    custom_events(rt, EventFilter::all())
        .into_iter()
        .filter_map(|kind| match kind {
            EventKind::Custom { name, payload } if name == IGNORED_EVENT => Some(payload),
            _ => None,
        })
        .collect()
}

fn counts(rt: &Runtime) -> (u64, u64) {
    let health = &rt.github_watcher_status().unwrap().repositories[0];
    assert!(health.last_error.is_none(), "{:?}", health.last_error);
    (health.dispatched, health.ignored)
}

/// A source listing `comments`, whose authors have no write access.
fn readers(comments: Vec<Value>) -> Fake {
    let mut source = Fake::new();
    source.route("issues/comments", Value::Array(comments));
    for login in ["outsider", "visitor"] {
        source.route(
            &format!("collaborators/{login}/permission"),
            json!({"permission":"read"}),
        );
    }
    source
}

#[tokio::test]
async fn a_login_without_write_access_is_still_ignored_and_noticed_once_per_thread() {
    let (rt, _dir, id) = fixture_operators(&["mengsig"]).await;
    let source = readers(vec![
        comment_on(2, 7, "outsider", "@aether deploy"),
        comment_on(3, 7, "visitor", "@aether deploy too"),
    ]);
    rt.github_poll(&source, at()).await.unwrap();
    assert!(triggers(&rt, id).is_empty());
    assert_eq!(counts(&rt), (0, 2));
    assert_eq!(
        permission_calls(&source).len(),
        2,
        "non-operators are looked up"
    );
    let noticed = notes(&rt, id);
    assert_eq!(noticed.len(), 1, "one per thread and reason: {noticed:?}");
    assert_eq!(
        noticed[0].text,
        "GitHub watcher ignored a request by `outsider` on o/r#7: the login lacks \
         repository write access."
    );
    assert_eq!(
        noticed[0].links,
        ["https://github.com/o/r/issues/7#issuecomment-2"]
    );
    assert_eq!(noticed[0].context, NoteContext::Digest);
    assert!(
        rt.store.inbox(id, 0).unwrap().is_empty(),
        "a notice wakes no one"
    );

    let later = readers(vec![
        comment_on(4, 7, "outsider", "@aether again"),
        comment_on(5, 8, "outsider", "@aether elsewhere"),
    ]);
    rt.github_poll(&later, at()).await.unwrap();
    assert_eq!(counts(&rt), (0, 4));
    let noticed = notes(&rt, id);
    assert_eq!(noticed.len(), 2, "{noticed:?}");
    assert!(noticed[1].text.contains("on o/r#8:"), "{noticed:?}");
    assert!(feed(&rt).is_empty(), "the fixed target exists");
}

#[tokio::test]
async fn an_operator_login_the_allowlist_excludes_is_ignored_with_a_notice() {
    let (rt, _dir, id) = fixture_operators(&["mengsig"]).await;
    let status = rt.github_watcher_status().unwrap();
    let mut config = status.config;
    config.repositories[0].allowed_users = Some(vec!["octocat".into()]);
    rt.configure_github_watcher(config, status.revision)
        .unwrap();
    let source = comment_source("mengsig", "@aether inspect");
    rt.github_poll(&source, at()).await.unwrap();
    assert!(triggers(&rt, id).is_empty());
    assert_eq!(counts(&rt), (0, 1));
    assert!(permission_calls(&source).is_empty());
    let noticed = notes(&rt, id);
    assert_eq!(noticed.len(), 1, "{noticed:?}");
    assert!(
        noticed[0]
            .text
            .ends_with("on o/r#1: the login is not in the repository's allowed_users."),
        "{noticed:?}"
    );
}

#[tokio::test]
async fn an_unlisted_login_outside_the_allowlist_stays_silent() {
    let (rt, _dir, id) = fixture_operators(&["mengsig"]).await;
    let status = rt.github_watcher_status().unwrap();
    let mut config = status.config;
    config.repositories[0].allowed_users = Some(vec!["octocat".into()]);
    rt.configure_github_watcher(config, status.revision)
        .unwrap();
    let source = readers(vec![comment(2, "outsider", "@aether deploy")]);
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(counts(&rt), (0, 1));
    assert!(notes(&rt, id).is_empty());
    assert!(feed(&rt).is_empty());
}

#[tokio::test]
async fn bots_are_ignored_and_only_an_operator_login_is_noticed() {
    let (rt, _dir, id) = fixture_operators(&["mengsig"]).await;
    let mut robot = comment(2, "robot", "@aether build");
    robot["user"]["type"] = json!("Bot");
    let mut posing = with_node(comment_on(3, 4, "mengsig", "@aether inspect"), "IC_3");
    posing["user"]["type"] = json!("Bot");
    let mut source = Fake::new();
    source.route("issues/comments", json!([robot, posing]));
    add_node(
        &mut source,
        node("IC_3", "IssueComment", "mengsig", "@aether inspect"),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert!(triggers(&rt, id).is_empty());
    assert_eq!(counts(&rt), (0, 2));
    assert!(permission_calls(&source).is_empty(), "bots cost no lookup");
    let noticed = notes(&rt, id);
    assert_eq!(noticed.len(), 1, "{noticed:?}");
    assert_eq!(
        noticed[0].text,
        "GitHub watcher ignored a request by `mengsig` on o/r#4: the account is a bot."
    );
}

#[tokio::test]
async fn without_a_live_agent_the_notice_goes_to_the_event_feed_once() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(&dir).await;
    rt.configure_github_watcher(
        GithubWatcherConfig {
            enabled: true,
            repositories: vec![GithubRepositoryConfig::automatic("o/r".into())],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let source = readers(vec![comment_on(2, 5, "outsider", "@Aether run this")]);
    rt.github_poll(&source, at()).await.unwrap();
    let again = readers(vec![comment_on(6, 5, "outsider", "@Aether please")]);
    rt.github_poll(&again, at()).await.unwrap();
    assert_eq!(counts(&rt), (0, 2));
    assert!(rt.store.agents(&Default::default()).unwrap().is_empty());
    assert!(!dir.path().join("github-workspaces").exists());
    let feed = feed(&rt);
    assert_eq!(feed.len(), 1, "{feed:?}");
    assert_eq!(
        feed[0],
        json!({
            "repository": "o/r", "number": 5, "requester": "outsider",
            "reason": "the login lacks repository write access",
            "url": "https://github.com/o/r/issues/5#issuecomment-2",
            "notice": "GitHub watcher ignored a request by `outsider` on o/r#5: the login \
                lacks repository write access.",
        })
    );
}

#[tokio::test]
async fn a_fixed_target_that_is_gone_sends_the_notice_to_the_feed() {
    let (rt, _dir, id) = fixture().await;
    let mut agent = rt.store.agent(id).unwrap().unwrap();
    agent.status = aether_core::agent::AgentStatus::Cancelled { reason: None };
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .unwrap();
    let source = readers(vec![comment(2, "outsider", "@aether deploy")]);
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(counts(&rt), (0, 1));
    assert!(notes(&rt, id).is_empty());
    assert_eq!(feed(&rt).len(), 1);
}
