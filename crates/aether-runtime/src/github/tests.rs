#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::{CreateAgentSpec, RuntimeConfig};
use aether_core::{
    agent::AgentStatus,
    github::{GithubDispatchMode, GithubRepositoryConfig},
    id::AgentId,
    model::ModelRef,
    schedule::SpawnSpec,
};
use aether_model::{Models, testing::ScriptedClient};
use aether_store::SqliteStore;
use aether_tools::ToolRegistry;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

const EXTERNAL: standing::Standing = standing::Standing::External;

pub(super) struct Fake {
    routes: HashMap<String, Value>,
    pub(super) calls: Mutex<Vec<String>>,
    missing_actor: Option<String>,
    rate_limited: bool,
    /// GraphQL objects by node id; an id without one answers `null`.
    pub(super) nodes: HashMap<String, Value>,
    /// The ids each GraphQL request asked about.
    pub(super) lookups: Mutex<Vec<Vec<String>>>,
    pub(super) graphql_fails: bool,
}
impl Fake {
    pub(super) fn new() -> Self {
        Self {
            routes: HashMap::new(),
            calls: Mutex::new(Vec::new()),
            missing_actor: None,
            rate_limited: false,
            nodes: HashMap::new(),
            lookups: Mutex::new(Vec::new()),
            graphql_fails: false,
        }
    }
    pub(super) fn route(&mut self, endpoint: &str, value: Value) {
        self.routes.insert(endpoint.into(), value);
    }
}
#[async_trait::async_trait]
impl source::Source for Fake {
    async fn get(&self, path: &str) -> std::result::Result<Value, source::Error> {
        self.calls.lock().unwrap().push(path.into());
        if self.rate_limited {
            return Err(source::Error {
                message: "rate limited".into(),
                retry_after: 3600,
                status: Some(429),
            });
        }
        let endpoint = path
            .strip_prefix("repos/o/r/")
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        if let Some(actor) = &self.missing_actor
            && endpoint == format!("collaborators/{actor}/permission")
        {
            return Err(source::Error {
                message: "not found".into(),
                retry_after: 0,
                status: Some(404),
            });
        }
        if let Some(v) = self.routes.get(endpoint) {
            return Ok(v.clone());
        }
        if endpoint.starts_with("collaborators/") {
            return Ok(json!({"permission":"write"}));
        }
        Ok(json!([]))
    }
    async fn graphql(&self, request: &Value) -> std::result::Result<Value, source::Error> {
        let ids: Vec<String> = serde_json::from_value(request["variables"]["ids"].clone()).unwrap();
        self.lookups.lock().unwrap().push(ids.clone());
        if self.graphql_fails {
            return Err(source::Error::new("GitHub HTTP 502"));
        }
        let nodes: Vec<Value> = ids
            .iter()
            .map(|id| self.nodes.get(id).cloned().unwrap_or(Value::Null))
            .collect();
        Ok(json!({"data": {"nodes": nodes}}))
    }
}
pub(super) async fn runtime(dir: &tempfile::TempDir) -> Arc<Runtime> {
    let rt = Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.path().to_path_buf()),
            github_checkout_root: Some(dir.path().join("github-workspaces")),
            delegated_workspace_root: Some(dir.path().join("managed")),
            ..Default::default()
        },
        Arc::new(SqliteStore::open(dir.path().join("store.db")).unwrap()),
        Arc::new(Models::load(Some(dir.path().to_path_buf())).unwrap()),
        Arc::new(ToolRegistry::with_builtins()),
        Some(Arc::new(ScriptedClient::with_responses(vec![]))),
        HashMap::new(),
    )
    .await
    .unwrap();
    rt.shutdown.cancel();
    tokio::task::yield_now().await;
    rt
}
pub(super) async fn fixture() -> (Arc<Runtime>, tempfile::TempDir, AgentId) {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(&dir).await;
    let id = rt
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap()
        .0
        .id;
    let config = GithubWatcherConfig {
        enabled: true,
        repositories: vec![GithubRepositoryConfig {
            repository: "o/r".into(),
            enabled: true,
            agent: Some(id),
            dispatch: GithubDispatchMode::Fixed,
            allowed_users: None,
        }],
        ..Default::default()
    };
    rt.configure_github_watcher(config, 0).unwrap();
    (rt, dir, id)
}
/// A `Thread` dispatch template that spawns a bounded worker per issue/PR.
fn thread_template() -> SpawnSpec {
    SpawnSpec {
        role: "worker".into(),
        name: None,
        objective: "Handle this GitHub thread.".into(),
        model: None,
        fallbacks: Vec::new(),
        scope: None,
        config: None,
        approval: None,
    }
}
async fn fixture_thread() -> (Arc<Runtime>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(&dir).await;
    let config = GithubWatcherConfig {
        enabled: true,
        repositories: vec![GithubRepositoryConfig {
            repository: "o/r".into(),
            enabled: true,
            agent: None,
            dispatch: GithubDispatchMode::Thread(thread_template()),
            allowed_users: None,
        }],
        ..Default::default()
    };
    rt.configure_github_watcher(config, 0).unwrap();
    (rt, dir)
}
pub(super) fn at() -> Timestamp {
    "2030-01-01T00:00:00Z".parse().unwrap()
}

fn managed_checkout(dir: &tempfile::TempDir, number: u64) -> std::path::PathBuf {
    managed_checkout_generation(dir, number, 0)
}

fn managed_checkout_generation(
    dir: &tempfile::TempDir,
    number: u64,
    generation: u64,
) -> std::path::PathBuf {
    let path = dir
        .path()
        .join("github-workspaces/o/r")
        .join(number.to_string())
        .join(format!("g{generation}"));
    std::fs::create_dir_all(&path).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(init.success());
    let remote = std::process::Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["remote", "add", "origin", "https://github.com/o/r.git"])
        .status()
        .unwrap();
    assert!(remote.success());
    path
}
pub(super) fn comment(id: u64, author: &str, body: &str) -> Value {
    comment_on(id, 1, author, body)
}
fn comment_on(id: u64, number: u64, author: &str, body: &str) -> Value {
    json!({"id":id,"body":body,"issue_url":format!("https://api.github.com/repos/o/r/issues/{number}"),"html_url":format!("https://github.com/o/r/issues/{number}#issuecomment-{id}"),"updated_at":"2029-12-31T23:59:00Z","user":{"login":author,"type":"User"}})
}
pub(super) fn issue() -> Value {
    json!({"id":1,"number":1,"title":"request","body":"@aether do work","labels":[],"html_url":"https://github.com/o/r/issues/1","created_at":"2029-12-31T23:59:00Z","updated_at":"2029-12-31T23:59:00Z","user":{"login":"maintainer","type":"User"}})
}

#[tokio::test]
async fn disabled_means_no_source_calls_and_config_is_cas() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(&dir).await;
    let source = Fake::new();
    rt.github_poll(&source, at()).await.unwrap();
    assert!(source.calls.lock().unwrap().is_empty());
    let status = rt
        .configure_github_watcher(GithubWatcherConfig::default(), 0)
        .unwrap();
    assert_eq!(status.revision, 1);
    assert!(
        rt.configure_github_watcher(GithubWatcherConfig::default(), 0)
            .is_err()
    );
    assert_eq!(runtime(&dir).await.github_watcher_status().unwrap(), status);
}
#[tokio::test]
async fn restart_dedup_and_external_not_operator_delivery() {
    let (rt, dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "maintainer", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let inbox = rt.store.inbox(id, 0).unwrap();
    assert_eq!(inbox.len(), 1);
    assert!(matches!(&inbox[0].kind,InboxKind::ExternalTrigger { source,.. } if source=="github"));
    let reopened = runtime(&dir).await;
    reopened.github_poll(&source, at()).await.unwrap();
    assert_eq!(reopened.store.inbox(id, 0).unwrap().len(), 1);
    assert_eq!(
        reopened.github_watcher_status().unwrap().repositories[0].dispatched,
        1
    );
}
#[tokio::test]
async fn permission_404_is_ignored_without_blocking_trusted_comment() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.missing_actor = Some("outsider".into());
    source.route(
        "issues/comments",
        json!([
            comment(2, "outsider", "@aether destructive"),
            comment(3, "maintainer", "@aether inspect")
        ]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
    let s = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!((s.dispatched, s.ignored), (1, 1));
    assert!(s.last_error.is_none());
}
#[tokio::test]
async fn malformed_permission_is_retryable_not_a_final_ignore() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "maintainer", "@aether inspect")]),
    );
    source.route("collaborators/maintainer/permission", json!({}));
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert!(
        rt.github_watcher_status().unwrap().repositories[0]
            .last_error
            .is_some()
    );
    source.route(
        "collaborators/maintainer/permission",
        json!({"permission":"write"}),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn paused_target_does_not_decide_or_advance_cursor() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "maintainer", "@aether inspect")]),
    );
    let mut agent = rt.store.agent(id).unwrap().unwrap();
    agent.paused = true;
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();
    let cursor = rt.github_repository_state("o/r").unwrap().cursor;
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.github_repository_state("o/r").unwrap().cursor, cursor);
    assert!(
        rt.github_watcher_status().unwrap().repositories[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains("paused")
    );
    agent.paused = false;
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .unwrap();
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn labels_use_latest_actor_and_event_identity_not_mutable_body() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    let mut issue = issue();
    issue["body"] = json!("<!-- aether:github-reply --> no mention");
    issue["labels"] = json!([{"name":"aether-review"}]);
    issue["user"]["login"] = json!("outsider");
    source.route("issues", json!([issue.clone()]));
    source.route("issues/1/events",json!([
        {"id":9,"event":"labeled","label":{"name":"aether-removed"},"created_at":"2029-12-31T23:57:00Z","actor":{"login":"wrong","type":"User"}},
        {"id":10,"event":"labeled","label":{"name":"aether-review"},"created_at":"2029-12-31T23:58:00Z","actor":{"login":"old","type":"User"}},
        {"id":11,"event":"labeled","label":{"name":"aether-review"},"created_at":"2029-12-31T23:59:00Z","actor":{"login":"maintainer","type":"User"}}
    ]));
    rt.github_poll(&source, at()).await.unwrap();
    let inbox = rt.store.inbox(id, 0).unwrap();
    assert_eq!(inbox.len(), 1);
    let InboxKind::ExternalTrigger { payload, .. } = &inbox[0].kind else {
        panic!("not external");
    };
    assert_eq!(payload["requester"], "maintainer");
    assert_eq!(payload["label"], "aether-review");
    assert!(
        !source
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("collaborators/outsider")
                || p.contains("collaborators/old")
                || p.contains("collaborators/wrong"))
    );
    issue["body"] = json!("edited title and body");
    source.route("issues", json!([issue]));
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn oversized_review_sweep_does_not_block_ordinary_stream() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "maintainer", "@aether inspect")]),
    );
    source.route(
        "pulls",
        Value::Array((1..=31).map(|n| json!({"number":n})).collect()),
    );
    let review_cursor = rt.github_repository_state("o/r").unwrap().review_cursor;
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!(state.cursor, at());
    assert_eq!(state.review_cursor, review_cursor);
    assert!(state.status.last_error.unwrap().contains("30 PRs"));
}
#[tokio::test]
async fn prebaseline_issue_is_not_reactivated_by_unrelated_activity() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    let mut issue = issue();
    issue["created_at"] = json!("2020-01-01T00:00:00Z");
    source.route("issues", json!([issue]));
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
}
#[tokio::test]
async fn review_inline_requires_submission_and_missing_parent_retries() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    let mut inline = comment(4, "maintainer", "@aether inline");
    inline["pull_request_url"] = json!("https://api.github.com/repos/o/r/pulls/1");
    inline["pull_request_review_id"] = json!(8);
    source.route("pulls/comments", json!([inline]));
    source.route(
        "pulls/1/reviews",
        json!([{"id":8,"state":"PENDING","submitted_at":null}]),
    );
    let review_cursor = rt.github_repository_state("o/r").unwrap().review_cursor;
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert_eq!(
        rt.github_repository_state("o/r").unwrap().review_cursor,
        review_cursor
    );
    source.route(
        "pulls/1/reviews",
        json!([{"id":8,"state":"COMMENTED","submitted_at":"2029-12-31T23:59:30Z","body":""}]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn rate_limit_stops_all_repositories_and_survives_restart() {
    let (rt, dir, _id) = fixture().await;
    let mut source = Fake::new();
    source.rate_limited = true;
    // Real clock is intentional: the persisted cooldown is wall-clock based.
    rt.github_poll(&source, now()).await.unwrap();
    assert_eq!(source.calls.lock().unwrap().len(), 1);
    let rt = runtime(&dir).await;
    rt.github_poll(&source, now()).await.unwrap();
    assert_eq!(source.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn bots_reply_markers_and_durable_thread_cap_bound_feedback() {
    let (rt, dir, id) = fixture().await;
    let mut source = Fake::new();
    let mut comments: Vec<Value> = (1..=21)
        .map(|n| comment(n, "maintainer", &format!("@aether task {n}")))
        .collect();
    comments.push(comment(
        30,
        "maintainer",
        "@aether <!-- aether:github-reply -->",
    ));
    let mut bot = comment(31, "robot", "@aether task");
    bot["user"]["type"] = json!("Bot");
    comments.push(bot);
    source.route("issues/comments", Value::Array(comments));
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 20);
    assert!(
        rt.github_watcher_status().unwrap().repositories[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains("cap")
    );
    let rt = runtime(&dir).await;
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 20);
}
#[tokio::test]
async fn newly_submitted_review_discovers_old_draft_inline_outside_update_feed() {
    let (rt, _dir, id) = fixture().await;
    let mut source = Fake::new();
    source.route("pulls", json!([{"number":1}]));
    source.route(
        "pulls/1/reviews",
        json!([{"id":8,"state":"COMMENTED","submitted_at":"2029-12-31T23:59:30Z","body":""}]),
    );
    let mut inline = comment(4, "maintainer", "@aether inline");
    inline["updated_at"] = json!("2020-01-01T00:00:00Z");
    inline["pull_request_url"] = json!("https://api.github.com/repos/o/r/pulls/1");
    inline["pull_request_review_id"] = json!(8);
    source.route("pulls/1/comments", json!([inline]));
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn disabling_during_poll_rejects_delivery_under_commit_hold() {
    let (rt, _dir, id) = fixture().await;
    let settings = rt.github_settings().unwrap();
    let mut config = settings.config.clone();
    config.enabled = false;
    rt.configure_github_watcher(config, settings.revision)
        .unwrap();
    let candidate = poll::Candidate {
        kind: "comment",
        id: 3,
        number: 1,
        login: "maintainer".into(),
        bot: false,
        body: "@aether work".into(),
        url: "https://github.com/o/r/issues/1".into(),
        at: at(),
        label: None,
        author: None,
        node_id: None,
    };
    assert!(
        rt.github_decide(
            settings.revision,
            &settings.config.repositories[0],
            &candidate,
            true,
            &standing::Standing::External
        )
        .await
        .is_err()
    );
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert!(rt.store.meta(&candidate.key("o/r")).unwrap().is_none());
}
#[tokio::test]
async fn oversized_listing_is_visible_and_never_advances_ordinary_cursor() {
    let (rt, _dir, _id) = fixture().await;
    let mut source = Fake::new();
    source.route("issues", Value::Array((0..100).map(|_| issue()).collect()));
    let cursor = rt.github_repository_state("o/r").unwrap().cursor;
    rt.github_poll(&source, at()).await.unwrap();
    let state = rt.github_repository_state("o/r").unwrap();
    assert_eq!(state.cursor, cursor);
    assert!(state.status.last_error.unwrap().contains("1000 items"));
    assert_eq!(source.calls.lock().unwrap().len(), 10);
}
#[tokio::test]
async fn failure_backoff_never_shortens_long_configured_interval() {
    let (rt, _dir, _id) = fixture().await;
    let mut status = rt.github_watcher_status().unwrap();
    status.config.poll_interval_secs = 86400;
    rt.configure_github_watcher(status.config, status.revision)
        .unwrap();
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "maintainer", "@aether inspect")]),
    );
    source.route("collaborators/maintainer/permission", json!({}));
    let before = now();
    rt.github_poll(&source, at()).await.unwrap();
    let status = &rt.github_watcher_status().unwrap().repositories[0];
    assert!(status.last_error.is_some());
    assert!(status.next_poll_at.unwrap() >= before + chrono::Duration::days(1));
}
fn mapped_agent(rt: &Runtime, number: u64) -> AgentId {
    rt.store
        .meta(&thread_agent_key("o/r", number))
        .unwrap()
        .expect("thread mapping recorded")
        .parse()
        .expect("mapping stores a valid agent id")
}
fn thread_candidate(id: u64) -> poll::Candidate {
    poll::Candidate {
        kind: "comment",
        id,
        number: 5,
        login: "maintainer".into(),
        bot: false,
        body: "@aether work".into(),
        url: "https://github.com/o/r/issues/5".into(),
        at: at(),
        label: None,
        author: None,
        node_id: None,
    }
}

#[tokio::test]
async fn thread_dispatch_refusals_leave_no_orphan_agent_or_task() {
    for refusal in ["revision", "cap", "duplicate"] {
        let (rt, _dir) = fixture_thread().await;
        let settings = rt.github_settings().unwrap();
        let repo = &settings.config.repositories[0];
        let candidate = thread_candidate(2);
        let cap_key = "github.watcher.thread:o/r:5";
        match refusal {
            "revision" => {
                let mut config = settings.config.clone();
                config.enabled = false;
                rt.configure_github_watcher(config, settings.revision)
                    .unwrap();
            }
            "cap" => {
                rt.apply(Commit::new().mutate(encode(cap_key.into(), &(now(), 20u32)).unwrap()))
                    .unwrap();
            }
            "duplicate" => {
                rt.apply(Commit::new().mutate(Mutation::SetMeta {
                    key: candidate.key("o/r"),
                    value: "delivered".into(),
                }))
                .unwrap();
            }
            _ => unreachable!(),
        }
        let last_seq = rt.store.last_seq().unwrap();
        let result = rt
            .github_decide(settings.revision, repo, &candidate, true, &EXTERNAL)
            .await;
        match refusal {
            "revision" => assert!(result.unwrap_err().to_string().contains("settings changed")),
            "cap" => assert!(result.unwrap_err().to_string().contains("dispatch cap")),
            "duplicate" => assert!(!result.unwrap()),
            _ => unreachable!(),
        }
        assert!(
            rt.store.agents(&Default::default()).unwrap().is_empty(),
            "{refusal}"
        );
        assert!(
            rt.store.tasks(&Default::default()).unwrap().is_empty(),
            "{refusal}"
        );
        assert_eq!(rt.store.last_seq().unwrap(), last_seq, "{refusal}");
        assert!(
            rt.store
                .meta(&thread_agent_key("o/r", 5))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            rt.store.meta(&candidate.key("o/r")).unwrap().as_deref(),
            (refusal == "duplicate").then_some("delivered")
        );
        assert_eq!(
            rt.github_repository_state("o/r").unwrap().status.dispatched,
            0
        );
    }
}

#[tokio::test]
async fn capped_thread_dispatch_does_not_replace_a_terminal_or_paused_agent() {
    for paused in [false, true] {
        let (rt, _dir) = fixture_thread().await;
        let settings = rt.github_settings().unwrap();
        let repo = &settings.config.repositories[0];
        rt.github_decide(
            settings.revision,
            repo,
            &thread_candidate(2),
            true,
            &EXTERNAL,
        )
        .await
        .unwrap();
        let first = mapped_agent(&rt, 5);
        let mut agent = rt.store.agent(first).unwrap().unwrap();
        if paused {
            agent.paused = true;
        } else {
            agent.status = AgentStatus::Cancelled { reason: None };
        }
        let cap_key = "github.watcher.thread:o/r:5";
        rt.apply(
            Commit::new()
                .mutate(Mutation::UpsertAgent(agent))
                .mutate(encode(cap_key.into(), &(now(), 20u32)).unwrap()),
        )
        .unwrap();
        let last_seq = rt.store.last_seq().unwrap();
        let candidate = thread_candidate(3);
        for _ in 0..2 {
            let error = rt
                .github_decide(settings.revision, repo, &candidate, true, &EXTERNAL)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("dispatch cap"));
            assert_eq!(rt.store.agents(&Default::default()).unwrap().len(), 1);
            assert_eq!(rt.store.tasks(&Default::default()).unwrap().len(), 1);
            assert_eq!(rt.store.last_seq().unwrap(), last_seq);
            assert_eq!(mapped_agent(&rt, 5), first);
            assert_eq!(rt.store.inbox(first, 0).unwrap().len(), 1);
            assert!(rt.store.meta(&candidate.key("o/r")).unwrap().is_none());
        }
        rt.apply(Commit::new().mutate(encode(cap_key.into(), &(now(), 0u32)).unwrap()))
            .unwrap();
        assert!(
            rt.github_decide(settings.revision, repo, &candidate, true, &EXTERNAL)
                .await
                .unwrap()
        );
        let replacement = mapped_agent(&rt, 5);
        assert_ne!(replacement, first);
        assert_eq!(rt.store.agents(&Default::default()).unwrap().len(), 2);
        assert_eq!(rt.store.tasks(&Default::default()).unwrap().len(), 2);
        assert_eq!(rt.store.inbox(replacement, 0).unwrap().len(), 1);
    }
}

#[tokio::test]
async fn prepared_thread_target_is_discarded_when_another_delivery_wins() {
    let (rt, _dir) = fixture_thread().await;
    let settings = rt.github_settings().unwrap();
    let repo = &settings.config.repositories[0];
    let first = rt.github_target(repo, 5).unwrap();
    let second = rt.github_target(repo, 5).unwrap();
    assert!(rt.store.agents(&Default::default()).unwrap().is_empty());
    assert!(rt.store.tasks(&Default::default()).unwrap().is_empty());

    assert!(
        rt.github_commit_decision(
            settings.revision,
            repo,
            &thread_candidate(2),
            Some(first),
            &EXTERNAL
        )
        .unwrap()
    );
    let winner = mapped_agent(&rt, 5);
    assert!(
        rt.github_commit_decision(
            settings.revision,
            repo,
            &thread_candidate(3),
            Some(second),
            &EXTERNAL
        )
        .unwrap()
    );
    assert_eq!(mapped_agent(&rt, 5), winner);
    assert_eq!(rt.store.agents(&Default::default()).unwrap().len(), 1);
    assert_eq!(rt.store.tasks(&Default::default()).unwrap().len(), 1);
    assert_eq!(rt.store.inbox(winner, 0).unwrap().len(), 2);
    assert_eq!(
        rt.github_repository_state("o/r").unwrap().status.dispatched,
        2
    );
}

#[tokio::test]
async fn thread_dispatch_spawns_a_fresh_agent_and_delivers_the_mention() {
    let (rt, _dir) = fixture_thread().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let agent = mapped_agent(&rt, 5);
    let created = rt.store.agent(agent).unwrap().unwrap();
    assert_eq!(created.status, AgentStatus::Ready);
    let task = rt
        .store
        .task(created.current_task.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(task.owner, Some(agent));
    assert!(
        task.objective
            .contains("GitHub thread: repository o/r, issue/PR #5.")
    );
    let inbox = rt.store.inbox(agent, 0).unwrap();
    assert_eq!(inbox.len(), 1);
    assert!(
        matches!(&inbox[0].kind, InboxKind::ExternalTrigger { source, .. } if source == "github")
    );
}
#[tokio::test]
async fn a_second_mention_on_the_same_thread_reuses_the_live_agent() {
    let (rt, _dir) = fixture_thread().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let first = mapped_agent(&rt, 5);
    source.route(
        "issues/comments",
        json!([comment_on(3, 5, "maintainer", "@aether inspect again")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(mapped_agent(&rt, 5), first);
    assert_eq!(rt.store.inbox(first, 0).unwrap().len(), 2);
}
#[tokio::test]
async fn a_terminal_thread_agent_is_replaced_by_a_fresh_one() {
    let (rt, _dir) = fixture_thread().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let first = mapped_agent(&rt, 5);
    let mut agent = rt.store.agent(first).unwrap().unwrap();
    agent.status = AgentStatus::Cancelled { reason: None };
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .unwrap();
    source.route(
        "issues/comments",
        json!([comment_on(3, 5, "maintainer", "@aether inspect again")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let second = mapped_agent(&rt, 5);
    assert_ne!(second, first);
    assert_eq!(rt.store.inbox(second, 0).unwrap().len(), 1);
}
#[tokio::test]
async fn different_thread_numbers_get_different_agents() {
    let (rt, _dir) = fixture_thread().await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([
            comment_on(2, 5, "maintainer", "@aether inspect"),
            comment_on(3, 6, "maintainer", "@aether inspect"),
        ]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_ne!(mapped_agent(&rt, 5), mapped_agent(&rt, 6));
}

#[tokio::test]
async fn automatic_dispatch_uses_one_managed_checkout_and_agent_per_thread() {
    let dir = tempfile::tempdir().unwrap();
    let first_checkout = managed_checkout(&dir, 5);
    let second_checkout = managed_checkout(&dir, 6);
    let rt = runtime(&dir).await;
    let config = GithubWatcherConfig {
        enabled: true,
        repositories: vec![GithubRepositoryConfig::automatic("o/r".into())],
        ..Default::default()
    };
    rt.configure_github_watcher(config, 0).unwrap();
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([
            comment_on(2, 5, "maintainer", "@Aether inspect"),
            comment_on(3, 6, "maintainer", "@aether inspect"),
        ]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let first: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 5))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    let second: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 6))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(first, second);
    for (id, checkout) in [(first, first_checkout), (second, second_checkout)] {
        let agent = rt.store.agent(id).unwrap().unwrap();
        assert_eq!(
            agent.scope.workspace_root.as_deref(),
            Some(checkout.as_path())
        );
        assert_eq!(agent.model, ModelRef::new("anthropic", "claude-sonnet-5-5"));
        assert_eq!(agent.status, AgentStatus::Ready);
        assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
    }
    assert_eq!(
        rt.github_watcher_status().unwrap().repositories[0].dispatched,
        2
    );

    // The durable mapping and checkout survive a daemon restart. A new
    // mention for the same thread is delivered to the same live agent.
    drop(rt);
    let reopened = runtime(&dir).await;
    let current = reopened.github_watcher_status().unwrap();
    let mut settings = current.config;
    settings.poll_interval_secs = 180;
    reopened
        .configure_github_watcher(settings, current.revision)
        .unwrap();
    let mut followup = Fake::new();
    followup.route(
        "issues/comments",
        json!([comment_on(4, 5, "maintainer", "@AETHER follow up")]),
    );
    reopened.github_poll(&followup, at()).await.unwrap();
    assert_eq!(
        reopened
            .store
            .meta(&automatic_agent_key("o/r", 5))
            .unwrap()
            .unwrap(),
        first.to_string()
    );
    assert_eq!(
        reopened.store.inbox(first, 0).unwrap().len(),
        2,
        "{:?}",
        reopened.github_watcher_status().unwrap()
    );
}

#[tokio::test]
async fn automatic_dispatch_ignores_untrusted_mentions_without_a_checkout_or_agent() {
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
    let mut source = Fake::new();
    source.missing_actor = Some("outsider".into());
    let mut bot = comment_on(3, 5, "build-bot", "@Aether run this");
    bot["user"]["type"] = json!("Bot");
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "outsider", "@Aether run this"), bot]),
    );

    rt.github_poll(&source, at()).await.unwrap();
    rt.github_poll(&source, at()).await.unwrap();
    let status = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!((status.dispatched, status.ignored), (0, 2));
    assert!(
        rt.store
            .meta(&automatic_agent_key("o/r", 5))
            .unwrap()
            .is_none()
    );
    assert!(!dir.path().join("github-workspaces").exists());
}

#[tokio::test]
async fn automatic_followup_refuses_an_agent_moved_out_of_its_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let checkout = managed_checkout(&dir, 5);
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
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@Aether first")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let id: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 5))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    let mut agent = rt.store.agent(id).unwrap().unwrap();
    agent.scope.workspace_root = Some(dir.path().join("other"));
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .unwrap();

    source.route(
        "issues/comments",
        json!([comment_on(3, 5, "maintainer", "@Aether second")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let failed = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!(failed.dispatched, 1);
    assert!(
        failed
            .last_error
            .as_deref()
            .unwrap()
            .contains("managed checkout scope")
    );
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);

    agent.scope.workspace_root = Some(checkout);
    rt.apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .unwrap();
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(
        rt.store.inbox(id, 0).unwrap().len(),
        2,
        "{:?}",
        rt.github_watcher_status().unwrap()
    );
}

/// A thread agent created after its repository was aliased stores the
/// canonical id; the follow-up check resolves the configured name the same way.
#[tokio::test]
async fn automatic_followup_reaches_an_agent_created_after_an_alias() {
    let dir = tempfile::tempdir().unwrap();
    managed_checkout(&dir, 5);
    let rt = runtime(&dir).await;
    rt.alias_repository(
        "github.com/o/r",
        None,
        Some("github.com/o/renamed".into()),
        false,
        Actor::user(),
    )
    .unwrap();
    rt.configure_github_watcher(
        GithubWatcherConfig {
            enabled: true,
            repositories: vec![GithubRepositoryConfig::automatic("o/r".into())],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@Aether first")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let id: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 5))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    let agent = rt.store.agent(id).unwrap().unwrap();
    assert_eq!(
        agent.scope.repository.as_deref(),
        Some("github.com/o/renamed")
    );

    source.route(
        "issues/comments",
        json!([comment_on(3, 5, "maintainer", "@Aether second")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let status = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!(status.last_error, None);
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 2);
}

#[tokio::test]
async fn automatic_checkout_rejects_a_symlink_without_touching_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::create_dir_all(dir.path().join("github-workspaces/o")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.path().join("github-workspaces/o/r")).unwrap();
    let rt = runtime(&dir).await;
    let error = checkout::prepare(&rt, "o/r", 5, 0).await.unwrap_err();
    assert!(
        error.to_string().contains("not a real directory"),
        "{error}"
    );
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
}

#[tokio::test]
async fn automatic_replacement_keeps_the_paused_agents_checkout_separate() {
    let dir = tempfile::tempdir().unwrap();
    let old_checkout = managed_checkout(&dir, 5);
    let new_checkout = managed_checkout_generation(&dir, 5, 1);
    std::fs::write(old_checkout.join("unfinished.txt"), "keep this work").unwrap();
    let rt = runtime(&dir).await;
    let config = GithubWatcherConfig {
        enabled: true,
        repositories: vec![GithubRepositoryConfig::automatic("o/r".into())],
        ..Default::default()
    };
    rt.configure_github_watcher(config, 0).unwrap();
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment_on(2, 5, "maintainer", "@Aether first")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let old: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 5))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    rt.pause_agent(old, false, Actor::user()).unwrap();

    source.route(
        "issues/comments",
        json!([comment_on(3, 5, "maintainer", "@Aether next")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let replacement: AgentId = rt
        .store
        .meta(&automatic_agent_key("o/r", 5))
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(old, replacement);
    assert_eq!(
        rt.store
            .agent(old)
            .unwrap()
            .unwrap()
            .scope
            .workspace_root
            .as_deref(),
        Some(old_checkout.as_path())
    );
    assert_eq!(
        rt.store
            .agent(replacement)
            .unwrap()
            .unwrap()
            .scope
            .workspace_root
            .as_deref(),
        Some(new_checkout.as_path())
    );
    assert_eq!(
        rt.store
            .meta(&automatic_generation_key("o/r", 5))
            .unwrap()
            .as_deref(),
        Some("1")
    );
    rt.resume_agent(old, false, Actor::user()).unwrap();
    assert_eq!(
        std::fs::read_to_string(old_checkout.join("unfinished.txt")).unwrap(),
        "keep this work"
    );
    assert!(!new_checkout.join("unfinished.txt").exists());
}

#[tokio::test]
async fn automatic_checkout_failure_is_visible_and_original_mention_retries() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let thread = dir.path().join("github-workspaces/o/r/5");
    std::fs::create_dir_all(&thread).unwrap();
    let unsafe_path = thread.join("g0");
    std::os::unix::fs::symlink(&outside, &unsafe_path).unwrap();
    let rt = runtime(&dir).await;
    let config = GithubWatcherConfig {
        enabled: true,
        repositories: vec![GithubRepositoryConfig::automatic("o/r".into())],
        ..Default::default()
    };
    rt.configure_github_watcher(config, 0).unwrap();
    let mut source = Fake::new();
    let candidate = comment_on(2, 5, "maintainer", "@Aether inspect");
    source.route("issues/comments", json!([candidate]));
    rt.github_poll(&source, at()).await.unwrap();
    let failed = rt.github_watcher_status().unwrap();
    assert!(
        failed.repositories[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("not a real directory")
    );
    assert_eq!(failed.repositories[0].dispatched, 0);
    assert!(
        rt.store
            .meta(&automatic_agent_key("o/r", 5))
            .unwrap()
            .is_none()
    );
    std::fs::remove_file(&unsafe_path).unwrap();
    managed_checkout(&dir, 5);
    rt.github_poll(&source, at()).await.unwrap();
    let recovered = rt.github_watcher_status().unwrap();
    assert!(recovered.repositories[0].last_error.is_none());
    assert_eq!(recovered.repositories[0].dispatched, 1);
    assert!(
        rt.store
            .meta(&automatic_agent_key("o/r", 5))
            .unwrap()
            .is_some()
    );
}
#[tokio::test]
async fn thread_dispatch_needs_no_live_agent_but_fixed_dispatch_does() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(&dir).await;
    let thread_config = GithubWatcherConfig {
        repositories: vec![GithubRepositoryConfig {
            repository: "o/r".into(),
            enabled: true,
            agent: None,
            dispatch: GithubDispatchMode::Thread(thread_template()),
            allowed_users: None,
        }],
        ..Default::default()
    };
    let status = rt.configure_github_watcher(thread_config, 0).unwrap();
    let fixed_config = GithubWatcherConfig {
        repositories: vec![GithubRepositoryConfig {
            repository: "o/r2".into(),
            enabled: true,
            agent: None,
            dispatch: GithubDispatchMode::Fixed,
            allowed_users: None,
        }],
        ..Default::default()
    };
    assert!(
        rt.configure_github_watcher(fixed_config, status.revision)
            .is_err()
    );
}

/// `fixture()` narrowed to `users`; returns the stored (normalized) status.
async fn fixture_allowlisted(
    users: &[&str],
) -> (
    Arc<Runtime>,
    tempfile::TempDir,
    AgentId,
    GithubWatcherStatus,
) {
    let (rt, dir, id) = fixture().await;
    let mut status = rt.github_watcher_status().unwrap();
    let mut config = status.config.clone();
    config.repositories[0].allowed_users = Some(users.iter().map(|u| (*u).into()).collect());
    status = rt
        .configure_github_watcher(config, status.revision)
        .unwrap();
    (rt, dir, id, status)
}
pub(super) fn permission_calls(source: &Fake) -> Vec<String> {
    source
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.contains("/collaborators/"))
        .cloned()
        .collect()
}
#[tokio::test]
async fn allowlist_is_stored_lowercase_deduplicated_and_never_empty() {
    let (rt, _dir, _id, status) = fixture_allowlisted(&["OctoCat", "monaLisa", "octocat"]).await;
    assert_eq!(
        status.config.repositories[0].allowed_users,
        Some(vec!["octocat".into(), "monalisa".into()])
    );
    let mut empty = status.config.clone();
    empty.repositories[0].allowed_users = Some(Vec::new());
    let error = rt
        .configure_github_watcher(empty, status.revision)
        .unwrap_err();
    assert!(
        error.to_string().contains("allowed_users is empty"),
        "{error}"
    );
    let mut invalid = status.config.clone();
    invalid.repositories[0].allowed_users = Some(vec!["-Some-Stranger".into()]);
    assert!(
        rt.configure_github_watcher(invalid, status.revision)
            .is_err()
    );
    assert_eq!(rt.github_watcher_status().unwrap(), status);
}
#[tokio::test]
async fn allowlisted_writer_dispatches_with_case_insensitive_match() {
    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "OCTOCAT", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    let inbox = rt.store.inbox(id, 0).unwrap();
    assert_eq!(inbox.len(), 1);
    let InboxKind::ExternalTrigger { payload, .. } = &inbox[0].kind else {
        panic!("not external");
    };
    assert_eq!(payload["requester"], "OCTOCAT");
    assert_eq!(
        permission_calls(&source),
        ["repos/o/r/collaborators/OCTOCAT/permission"]
    );
}
#[tokio::test]
async fn unlisted_admin_is_ignored_without_a_permission_lookup() {
    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "Some-Stranger", "@aether deploy")]),
    );
    source.route(
        "collaborators/Some-Stranger/permission",
        json!({"permission":"admin"}),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert!(permission_calls(&source).is_empty());
    let health = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!((health.dispatched, health.ignored), (0, 1));
    assert!(health.last_error.is_none());
}
#[tokio::test]
async fn allowlisted_user_without_write_access_is_still_ignored() {
    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "octoCat", "@aether inspect")]),
    );
    source.route(
        "collaborators/octoCat/permission",
        json!({"permission":"triage"}),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert_eq!(permission_calls(&source).len(), 1);
    let health = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!((health.dispatched, health.ignored), (0, 1));
}
#[tokio::test]
async fn label_applied_by_unlisted_user_is_ignored_even_on_a_listed_authors_issue() {
    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    let mut source = Fake::new();
    let mut issue = issue();
    issue["body"] = json!("no mention");
    issue["labels"] = json!([{"name":"aether"}]);
    issue["user"]["login"] = json!("octoCat");
    source.route("issues", json!([issue]));
    source.route(
        "issues/1/events",
        json!([{"id":10,"event":"labeled","label":{"name":"aether"},"created_at":"2029-12-31T23:59:00Z","actor":{"login":"Some-Stranger","type":"User"}}]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert!(permission_calls(&source).is_empty());
    assert_eq!(
        rt.github_watcher_status().unwrap().repositories[0].ignored,
        1
    );
}
#[tokio::test]
async fn without_an_allowlist_any_writer_still_dispatches() {
    let (rt, _dir, id) = fixture().await;
    assert_eq!(
        rt.github_watcher_status().unwrap().config.repositories[0].allowed_users,
        None
    );
    let mut source = Fake::new();
    source.route(
        "issues/comments",
        json!([comment(2, "Some-Stranger", "@aether inspect")]),
    );
    rt.github_poll(&source, at()).await.unwrap();
    assert_eq!(rt.store.inbox(id, 0).unwrap().len(), 1);
    assert_eq!(permission_calls(&source).len(), 1);
}
/// One mention by `author` on each body-carrying path: issue, PR body, review
/// body and inline review comment (labels and comments are covered above).
fn mentions_on_every_body_path(author: &str) -> Fake {
    let mut source = Fake::new();
    let mut issue_body = issue();
    issue_body["user"]["login"] = json!(author);
    let mut pr_body = issue();
    pr_body["id"] = json!(2);
    pr_body["number"] = json!(2);
    pr_body["pull_request"] = json!({});
    pr_body["user"]["login"] = json!(author);
    source.route("issues", json!([issue_body, pr_body]));
    source.route(
        "pulls/2/reviews",
        json!([{"id":8,"state":"COMMENTED","submitted_at":"2029-12-31T23:59:30Z","body":"@aether review","user":{"login":author,"type":"User"}}]),
    );
    let mut inline = comment(4, author, "@aether inline");
    inline["pull_request_url"] = json!("https://api.github.com/repos/o/r/pulls/2");
    inline["pull_request_review_id"] = json!(8);
    source.route("pulls/comments", json!([inline]));
    source
}
#[tokio::test]
async fn allowlist_applies_to_issue_pr_review_and_inline_mentions() {
    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    let source = mentions_on_every_body_path("Some-Stranger");
    rt.github_poll(&source, at()).await.unwrap();
    assert!(rt.store.inbox(id, 0).unwrap().is_empty());
    assert!(permission_calls(&source).is_empty());
    let health = &rt.github_watcher_status().unwrap().repositories[0];
    assert_eq!((health.dispatched, health.ignored), (0, 4));
    assert!(health.last_error.is_none(), "{:?}", health.last_error);

    let (rt, _dir, id, _status) = fixture_allowlisted(&["octoCat"]).await;
    rt.github_poll(&mentions_on_every_body_path("octocat"), at())
        .await
        .unwrap();
    let kinds: BTreeSet<String> = rt
        .store
        .inbox(id, 0)
        .unwrap()
        .iter()
        .map(|item| match &item.kind {
            InboxKind::ExternalTrigger { payload, .. } => {
                format!(
                    "{}#{}",
                    payload["kind"].as_str().unwrap(),
                    payload["number"]
                )
            }
            other => panic!("not external: {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from(["issue#1", "issue#2", "review#2", "review_comment#2"].map(String::from))
    );
}

#[tokio::test]
async fn prepared_github_spawn_rechecks_workspace_gc_admission() {
    for reserved in [true, false] {
        let (rt, dir) = fixture_thread().await;
        let workspace = dir.path().join("managed/collected");
        std::fs::create_dir_all(&workspace).unwrap();
        let settings = rt.github_settings().unwrap();
        let mut repo = settings.config.repositories[0].clone();
        let GithubDispatchMode::Thread(template) = &mut repo.dispatch else {
            panic!("thread");
        };
        template.scope = Some(aether_core::agent::Scope {
            workspace_root: Some(workspace.clone()),
            ..Default::default()
        });
        let prepared = rt.github_target(&repo, 5).unwrap();
        if reserved {
            *rt.workspace_gc_active.lock() = Some(workspace);
        } else {
            std::fs::remove_dir(&workspace).unwrap();
        }
        let error = rt
            .github_commit_decision(
                settings.revision,
                &repo,
                &thread_candidate(2),
                Some(prepared),
                &EXTERNAL,
            )
            .unwrap_err();
        assert!(
            error.to_string().contains(if reserved {
                "being inspected"
            } else {
                "disappeared"
            }),
            "{error}"
        );
        assert!(rt.store.agents(&Default::default()).unwrap().is_empty());
        assert!(rt.store.tasks(&Default::default()).unwrap().is_empty());
    }
}
