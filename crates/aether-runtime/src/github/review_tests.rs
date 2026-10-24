//! The resumable review sweep and its backoff, apart from comment polling.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::tests::{Fake, at, comment, fixture, fixture_sweeping, runtime_sweeping};
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// `count` open pull requests, each with one submitted review mentioning
/// Aether; every fifth also has a mentioning inline comment in the feed.
fn open_pulls_with_review_mentions(count: u64) -> Fake {
    let mut source = Fake::new();
    source.route(
        "pulls",
        Value::Array((1..=count).map(|n| json!({ "number": n })).collect()),
    );
    let mut feed = Vec::new();
    for n in 1..=count {
        source.route(&format!("pulls/{n}/reviews"), json!([review(n)]));
        if n % 5 == 0 {
            let mut inline = comment(5000 + n, "maintainer", &format!("@aether inline {n}"));
            inline["pull_request_url"] =
                json!(format!("https://api.github.com/repos/o/r/pulls/{n}"));
            inline["pull_request_review_id"] = json!(1000 + n);
            feed.push(inline);
        }
    }
    source.route("pulls/comments", Value::Array(feed));
    source
}

fn review(n: u64) -> Value {
    json!({"id": 1000 + n, "state": "COMMENTED", "submitted_at": "2029-12-31T23:59:30Z",
           "body": format!("@aether review {n}"), "html_url": format!("https://github.com/o/r/pull/{n}"),
           "user": {"login": "maintainer", "type": "User"}})
}

/// Every delivered trigger as `kind#number#text`, failing on any repeat.
fn delivered(rt: &Runtime, agent: AgentId) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    for item in rt.store.inbox(agent, 0).unwrap() {
        let InboxKind::ExternalTrigger { payload, .. } = &item.kind else {
            panic!("not an external trigger");
        };
        let kind = payload["kind"].as_str().unwrap();
        let key = format!(
            "{kind}#{}#{}",
            payload["number"],
            payload["text"].as_str().unwrap()
        );
        assert!(seen.insert(key.clone()), "delivered twice: {key}");
    }
    seen
}

/// Which pull requests' reviews were read since the calls were last cleared.
fn reviews_read(source: &Fake) -> Vec<u64> {
    let mut calls = source.calls.lock().unwrap();
    let read = calls
        .iter()
        .filter_map(|c| c.strip_prefix("repos/o/r/pulls/")?.split_once("/reviews"))
        .map(|(n, _)| n.parse().unwrap())
        .collect();
    calls.clear();
    read
}

fn pending(rt: &Runtime) -> Option<Vec<u64>> {
    let state = rt.github_repository_state("o/r").unwrap();
    state.review.cycle.map(|cycle| cycle.pending)
}

#[tokio::test]
async fn review_mentions_on_45_open_prs_all_arrive_once_over_resumed_passes() {
    let (rt, _dir, id) = fixture().await;
    let source = open_pulls_with_review_mentions(45);
    let start = rt.github_repository_state("o/r").unwrap().review_cursor;
    let mut read = Vec::new();
    let mut passes = 0;
    while passes == 0 || pending(&rt).is_some() {
        assert!(passes < 5, "the sweep stopped making progress");
        let pass_at = at() + chrono::Duration::minutes(passes);
        rt.github_poll(&source, pass_at).await.unwrap();
        let swept = reviews_read(&source);
        assert!(!swept.is_empty() && swept.len() <= 30, "{swept:?}");
        read.extend(swept);
        let state = rt.github_repository_state("o/r").unwrap();
        assert_eq!(state.status.last_error, None);
        if state.review.cycle.is_some() {
            assert_eq!(state.review_cursor, start, "cursor moved mid-cycle");
        }
        passes += 1;
    }
    assert!(passes >= 2, "45 PRs never fit one pass");
    assert_eq!(
        read,
        (1..=45).collect::<Vec<_>>(),
        "oldest first, each once"
    );
    assert_eq!(
        rt.github_repository_state("o/r").unwrap().review_cursor,
        at()
    );
    let mentions = delivered(&rt, id);
    assert_eq!(mentions.len(), 45 + 9);
    // The next cycle reads them all again and delivers nothing twice.
    for later in 0..passes {
        let pass_at = at() + chrono::Duration::minutes(10 + later);
        rt.github_poll(&source, pass_at).await.unwrap();
    }
    assert_eq!(delivered(&rt, id), mentions);
}

#[tokio::test]
async fn a_failing_review_sweep_never_delays_a_comment_mention() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route("pulls", json!([{ "number": 1 }]));
    source.route("pulls/1/reviews", json!({ "message": "not a listing" }));
    let first = now();
    rt.github_poll(&source, first).await.unwrap();
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!((state.failures, state.review.failures), (0, 1));
    assert!(
        state
            .status
            .last_error
            .unwrap()
            .starts_with("review sweep: ")
    );
    let mut mention = comment(2, "maintainer", "@aether inspect");
    mention["updated_at"] = json!((first + chrono::Duration::seconds(60)).to_rfc3339());
    source.route("issues/comments", json!([mention]));
    source.calls.lock().unwrap().clear();
    // Due again for comments (120 s), still backing off for reviews (240 s).
    rt.github_poll(&source, first + chrono::Duration::seconds(130))
        .await
        .unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
    assert!(reviews_read(&source).is_empty(), "review sweep ran early");
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!((state.failures, state.review.failures), (0, 1));
    assert!(state.status.last_error.unwrap().contains("not an array"));
}

#[tokio::test]
async fn a_comment_failure_never_holds_back_the_review_sweep() {
    let (rt, _dir, id) = fixture().await;
    let mut source = open_pulls_with_review_mentions(1);
    source.route("issues/comments", json!({ "message": "not a listing" }));
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(delivered(&rt, id).len(), 1);
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!((state.failures, state.review.failures), (1, 0));
    assert_eq!(state.review_cursor, at());
    assert_ne!(state.cursor, at());
}

#[tokio::test]
async fn the_review_position_survives_a_daemon_restart() {
    let (rt, dir, id) = fixture_sweeping(10).await;
    let source = open_pulls_with_review_mentions(25);
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(reviews_read(&source), (1..=10).collect::<Vec<_>>());
    assert_eq!(pending(&rt), Some((11..=25).collect()));
    drop(rt);
    let rt = runtime_sweeping(&dir, 10).await;
    rt.github_poll(&source, at() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(reviews_read(&source), (11..=20).collect::<Vec<_>>());
    rt.github_poll(&source, at() + chrono::Duration::minutes(2))
        .await
        .unwrap();
    assert_eq!(reviews_read(&source), (21..=25).collect::<Vec<_>>());
    assert_eq!(pending(&rt), None);
    assert_eq!(
        rt.github_repository_state("o/r").unwrap().review_cursor,
        at()
    );
    assert_eq!(delivered(&rt, id).len(), 25 + 5);
}

#[tokio::test]
async fn a_pr_closed_mid_cycle_is_still_swept_and_then_dropped() {
    let (rt, _dir, id) = fixture_sweeping(10).await;
    let mut source = open_pulls_with_review_mentions(15);
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(reviews_read(&source), (1..=10).collect::<Vec<_>>());
    let still_open: Vec<Value> = (1..=15)
        .filter(|n| *n != 12)
        .map(|n| json!({ "number": n }))
        .collect();
    source.route("pulls", Value::Array(still_open));
    rt.github_poll(&source, at() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(reviews_read(&source), (11..=15).collect::<Vec<_>>());
    assert!(delivered(&rt, id).contains("review#12#@aether review 12"));
    assert_eq!(delivered(&rt, id).len(), 15 + 3);
    // A new cycle no longer finds the closed, inactive pull request.
    rt.github_poll(&source, at() + chrono::Duration::minutes(2))
        .await
        .unwrap();
    assert!(!reviews_read(&source).contains(&12));
    assert_eq!(pending(&rt), Some((11..=15).filter(|n| *n != 12).collect()));
}

#[tokio::test]
async fn a_failed_pass_keeps_the_position_and_retries_the_same_pull_requests() {
    let (rt, _dir, id) = fixture_sweeping(10).await;
    let mut source = open_pulls_with_review_mentions(15);
    rt.github_poll(&source, at()).await.unwrap();
    reviews_read(&source);
    source.route("pulls/13/reviews", json!({ "message": "not a listing" }));
    rt.github_poll(&source, at() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(reviews_read(&source), vec![11, 12, 13]);
    assert_eq!(pending(&rt), Some((11..=15).collect()));
    source.route("pulls/13/reviews", json!([review(13)]));
    rt.github_poll(&source, at() + chrono::Duration::minutes(2))
        .await
        .unwrap();
    assert_eq!(reviews_read(&source), (11..=15).collect::<Vec<_>>());
    assert_eq!(pending(&rt), None);
    assert_eq!(delivered(&rt, id).len(), 15 + 3);
}

#[tokio::test]
async fn a_pull_request_that_overruns_the_budget_waits_for_the_next_pass() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route(
        "pulls",
        json!([{ "number": 1 }, { "number": 2 }, { "number": 3 }]),
    );
    // Ten pages of reviews and ten of inline history: 20 requests a PR.
    let filler = |item: Value| Value::Array(vec![item; 100]);
    for n in 1..=3 {
        let mut reviews = vec![filler(json!({ "state": "PENDING", "submitted_at": null })); 9];
        reviews.push(json!([review(n)]));
        source.route_pages(&format!("pulls/{n}/reviews"), reviews);
        let mut inline = vec![filler(json!({ "body": "" })); 9];
        inline.push(json!([]));
        source.route_pages(&format!("pulls/{n}/comments"), inline);
    }
    rt.github_poll(&source, at()).await.unwrap();
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!(state.status.last_error, None);
    assert_eq!(pending(&rt), Some(vec![3]));
    assert_eq!(delivered(&rt, id).len(), 2);
    rt.github_poll(&source, at() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(pending(&rt), None);
    assert_eq!(delivered(&rt, id).len(), 3);
}
