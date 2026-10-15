#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Version skew against a real daemon: the supported version window is served,
//! anything else is refused at hello with both bounds named.

mod support;

use aether_core::github::GithubWatcherConfig;
use aether_protocol::PROTOCOL_VERSION;
use aether_protocol::compat::MIN_PROTOCOL_VERSION;
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

async fn save_watcher(conn: &mut RawConn, config: &Value, revision: u64) -> Value {
    conn.call(json!({
        "cmd":"configure_github_watcher",
        "config":config,
        "expected_revision":revision
    }))
    .await
}

fn revision_of(reply: &Value) -> u64 {
    reply["result"]["Ok"]["data"]["revision"]
        .as_u64()
        .unwrap_or_else(|| panic!("a watcher reply with a revision: {reply}"))
}

#[tokio::test]
async fn previous_version_preserves_watcher_allowlists() {
    let daemon = TestDaemon::start().await;
    let mut old = RawConn::connect(&daemon.socket).await.unwrap();
    old.hello(MIN_PROTOCOL_VERSION).await.unwrap();
    let initial = old.call(json!({"cmd":"get_github_watcher"})).await;
    let revision = initial["result"]["Ok"]["data"]["revision"]
        .as_u64()
        .unwrap();
    let mut config = serde_json::to_value(GithubWatcherConfig::default()).unwrap();
    config["repositories"] = json!([{"repository":"o/r", "enabled":true,
        "dispatch":{"mode":"automatic"}, "allowed_users":["octocat"]}]);
    let saved = old.call(json!({"cmd":"configure_github_watcher", "config":config, "expected_revision":revision})).await;
    assert_eq!(
        saved["result"]["Ok"]["data"]["config"]["repositories"][0]["allowed_users"],
        json!(["octocat"]),
        "{saved}"
    );
    let read = old.call(json!({"cmd":"get_github_watcher"})).await;
    assert_eq!(
        read["result"]["Ok"]["data"]["config"]["repositories"][0]["allowed_users"],
        json!(["octocat"])
    );
    daemon.stop().await;
}

/// Image references arrived in v19; only earlier clients need a text fallback.
#[tokio::test]
async fn served_clients_read_images_according_to_their_protocol_version() {
    const PNG_BASE64: &str = "iVBORw0KGgpwaXhlbHM=";
    let daemon = TestDaemon::start().await;
    let mut current = RawConn::connect(&daemon.socket).await.expect("connect");
    current.hello(PROTOCOL_VERSION).await.expect("served");
    let created = current
        .call(json!({"cmd": "create_agent", "role": "skew", "start": false, "model": {"provider": "claude-code", "model": "haiku"}}))
        .await;
    let agent = created["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("an agent id: {created}"))
        .to_owned();
    let sent = current
        .call(
            json!({"cmd": "send_message", "to": agent, "content": "look",
            "attachments": [{"media_type": "image/png", "data": PNG_BASE64, "name": "shot.png"}]}),
        )
        .await;
    assert_eq!(sent["result"]["Ok"]["reply"], "message_sent", "{sent}");

    let inbox = json!({"cmd": "get_inbox", "agent": agent, "after": 0});
    let now = current.call(inbox.clone()).await;
    let blocks = &now["result"]["Ok"]["data"][0]["kind"]["content"];
    assert_eq!(blocks[0], json!({"type": "text", "text": "look"}), "{now}");
    assert_eq!(blocks[1]["type"], "image_ref", "{now}");
    assert_eq!(blocks[1]["name"], "shot.png", "{now}");
    assert_eq!(blocks[1]["size_bytes"], 14, "{now}");

    for version in MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION {
        let mut old = RawConn::connect(&daemon.socket).await.expect("connect");
        old.hello(version).await.expect("served");
        let before = old.call(inbox.clone()).await;
        assert_eq!(
            before["result"]["Ok"]["data"][0]["kind"]["content"],
            if version < 19 {
                json!([
                    {"type": "text", "text": "look"},
                    {"type": "text", "text": "[image: shot.png, 14 B]"}
                ])
            } else {
                blocks.clone()
            },
            "v{version}: {before}"
        );
    }
    daemon.stop().await;
}

/// `operator_users` arrived in v17, now the oldest version served, so a
/// client one version back reads and saves the list like the current one.
#[tokio::test]
async fn a_client_one_version_back_reads_and_saves_operator_users() {
    let daemon = TestDaemon::start().await;
    let mut old = RawConn::connect(&daemon.socket).await.expect("connect");
    old.hello(MIN_PROTOCOL_VERSION).await.expect("N-1 served");
    let initial = old.call(json!({"cmd":"get_github_watcher"})).await;
    let mut listed = serde_json::to_value(GithubWatcherConfig::default()).unwrap();
    assert!(listed.get("operator_users").is_none(), "{listed}");
    listed["operator_users"] = json!(["Mengsig", "octoCat"]);
    let saved = save_watcher(&mut old, &listed, revision_of(&initial)).await;
    let stored = json!(["mengsig", "octocat"]);
    assert_eq!(
        saved["result"]["Ok"]["data"]["config"]["operator_users"], stored,
        "{saved}"
    );

    let mut current = RawConn::connect(&daemon.socket).await.expect("connect");
    current.hello(PROTOCOL_VERSION).await.expect("N served");
    for conn in [&mut old, &mut current] {
        let read = conn.call(json!({"cmd":"get_github_watcher"})).await;
        assert_eq!(revision_of(&read), revision_of(&saved), "{read}");
        assert_eq!(
            read["result"]["Ok"]["data"]["config"]["operator_users"], stored,
            "{read}"
        );
    }
    daemon.stop().await;
}

/// Create an agent and pause it, returning the raw `paused` reply data.
async fn pause_reply(conn: &mut RawConn) -> serde_json::Value {
    let created = conn
        .call(json!({"cmd": "create_agent", "role": "skew", "start": false, "model": {"provider": "claude-code", "model": "haiku"}}))
        .await;
    let agent = created["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("an agent id: {created}"))
        .to_owned();
    let paused = conn.call(json!({"cmd": "pause", "agent": agent})).await;
    assert_eq!(
        paused["result"]["Ok"]["reply"], "paused",
        "expected a paused reply: {paused}"
    );
    paused["result"]["Ok"]["data"].clone()
}

#[tokio::test]
async fn the_current_version_is_matched_and_the_window_reported() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    let welcome = conn.hello(PROTOCOL_VERSION).await.expect("served");
    assert_eq!(welcome["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(welcome["daemon_protocol_version"], PROTOCOL_VERSION);
    assert_eq!(welcome["min_protocol_version"], MIN_PROTOCOL_VERSION);

    // At the current version the reply keeps its current shape.
    let data = pause_reply(&mut conn).await;
    assert!(data.get("agents").is_some(), "{data}");
    assert!(data.get("after_current_turn").is_none(), "{data}");
    daemon.stop().await;
}

/// N-1 is served rather than refused, and it goes on working: the reply it
/// gets is the one its own version reads. v10 and v11 share reply/event shapes;
/// v11's new operator requests are never sent by an older client.
#[tokio::test]
async fn a_client_one_version_back_is_served_and_keeps_working() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    let welcome = conn.hello(MIN_PROTOCOL_VERSION).await.expect("served");
    assert_eq!(
        welcome["protocol_version"], MIN_PROTOCOL_VERSION,
        "the welcome states the version in effect: {welcome}"
    );
    assert_eq!(welcome["daemon_protocol_version"], PROTOCOL_VERSION);

    let data = pause_reply(&mut conn).await;
    assert!(data.get("agents").is_some(), "{data}");
    assert!(data.get("mid_turn").is_some(), "{data}");
    daemon.stop().await;
}

#[tokio::test]
async fn a_version_outside_the_window_is_refused_with_both_bounds() {
    let daemon = TestDaemon::start().await;
    // `saturating_sub`: MIN_PROTOCOL_VERSION is itself floored at 1, so a
    // plain `- 1` would underflow if the window ever starts there; the low
    // case is then MIN_PROTOCOL_VERSION itself, inside the window, so it is
    // skipped rather than asserted as a refusal.
    let too_low = MIN_PROTOCOL_VERSION.saturating_sub(1);
    let cases = [too_low, PROTOCOL_VERSION + 1]
        .into_iter()
        .filter(|&asked| asked != MIN_PROTOCOL_VERSION);
    for asked in cases {
        let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
        let refusal = conn.hello(asked).await.expect_err("refused");
        assert_eq!(refusal["code"], "incompatible", "{refusal}");
        let message = refusal["message"].as_str().unwrap_or_default().to_owned();
        assert!(
            message.contains(&MIN_PROTOCOL_VERSION.to_string())
                && message.contains(&PROTOCOL_VERSION.to_string()),
            "the refusal must name the served window: {message}"
        );
        assert!(
            conn.recv().await.is_none(),
            "a refused connection is closed, not left open"
        );
    }
    daemon.stop().await;
}

/// Spawn schedules arrived in v9, which is now the oldest version served, so a
/// client one version back works them like any other: nothing is refused and
/// nothing is hidden.
#[tokio::test]
async fn a_client_one_version_back_works_a_spawn_schedule() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    conn.hello(MIN_PROTOCOL_VERSION).await.expect("served");
    let created = conn
        .call(json!({"cmd": "create_schedule", "spec": {
            "name": "nightly",
            "spawn": {
                "role": "coordinator",
                "objective": "Summarise overnight failures",
                "model": {"provider": "claude-code", "model": "haiku"}
            },
            "recurrence": {"kind": "interval", "seconds": 3600}
        }}))
        .await;
    let id = created["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a schedule id: {created}"))
        .to_owned();

    let listed = conn.call(json!({"cmd": "list_schedules"})).await;
    assert_eq!(
        listed["result"]["Ok"]["data"][0]["id"],
        json!(id),
        "the schedule must be listed for N-1: {listed}"
    );
    let cancel = conn
        .call(json!({"cmd": "update_schedule", "id": id, "action": "cancel"}))
        .await;
    assert_eq!(cancel["result"]["Ok"]["reply"], "schedule", "{cancel}");
    let still = conn.call(json!({"cmd": "get_schedule", "id": id})).await;
    assert_eq!(
        still["result"]["Ok"]["data"]["status"]["state"], "cancelled",
        "the change must have applied: {still}"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn operator_commands_round_trip_and_do_not_fabricate_authorship() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    conn.hello(PROTOCOL_VERSION).await.unwrap();
    let created = conn.call(json!({"cmd":"create_agent", "role":"operator-test", "start":false, "model":{"provider":"claude-code","model":"haiku"}})).await;
    let id = created["result"]["Ok"]["data"]["id"].as_str().unwrap();
    let renamed = conn
        .call(json!({"cmd":"update_agent", "id":id, "patch":{"name":" 名前 café "}}))
        .await;
    assert_eq!(renamed["result"]["Ok"]["data"]["name"], "名前 café");
    let invalid = conn
        .call(json!({"cmd":"update_agent", "id":id, "patch":{"name":"   "}}))
        .await;
    assert_eq!(invalid["result"]["Err"]["code"], "invalid_argument");
    let request = json!({"cmd":"create_memory", "spec":{"scope":{"scope":"agent","id":id},"kind":"fact","text":"Prefer deterministic checks before publishing changes."}});
    let admitted = conn.call(request.clone()).await;
    assert_eq!(admitted["result"]["Ok"]["reply"], "memory", "{admitted}");
    assert!(
        admitted["result"]["Ok"]["data"]["provenance"]
            .get("agent")
            .is_none()
    );
    let provenance = &admitted["result"]["Ok"]["data"]["provenance"];
    assert_eq!(provenance["actor"]["actor"], "user");
    assert_eq!(provenance["actor"]["origin"]["transport"], "local_socket");
    assert!(
        !provenance["actor"]["origin"]["connection"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    let memories = conn.call(json!({"cmd":"memories", "query":{}})).await;
    assert_eq!(
        memories["result"]["Ok"]["data"][0]["provenance"],
        *provenance
    );
    let mut forged = request.clone();
    forged["spec"]["provenance"] = json!({"actor":{"actor":"user","name":"forged"}});
    forged["req"] = json!(1000);
    conn.send(&forged).await;
    let refused = conn.await_reply(1000).await;
    assert!(
        refused["result"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown field `provenance`"),
        "{refused}"
    );
    assert_eq!(admitted["result"]["Ok"]["data"]["scope"]["id"], id);
    let duplicate = conn.call(request).await;
    assert!(
        duplicate["result"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("redundant")
    );
    daemon.stop().await;
}

/// Over the wire, `recorded_by` lists what a person filed and leaves out a
/// memory merely visible beside it; a recorder the daemon does not know is
/// refused rather than read as no filter.
#[tokio::test]
async fn memories_recorded_by_the_operator_are_listed_by_that_filter() {
    let daemon = TestDaemon::start().await;
    let mut conn = RawConn::connect(&daemon.socket).await.unwrap();
    let welcome = conn.hello(PROTOCOL_VERSION).await.unwrap();
    assert!(
        welcome["features"]
            .as_array()
            .unwrap()
            .contains(&json!("memory_recorded_by")),
        "{welcome}"
    );
    let created = conn.call(json!({"cmd":"create_agent", "role":"recorder-test", "start":false, "model":{"provider":"claude-code","model":"haiku"}})).await;
    let id = created["result"]["Ok"]["data"]["id"].as_str().unwrap();
    let taught = conn.call(json!({"cmd":"create_memory", "spec":{"scope":{"scope":"agent","id":id},"kind":"constraint","text":"Never push without a green local CI run."}})).await;
    let taught = taught["result"]["Ok"]["data"]["id"].clone();
    assert!(taught.is_string(), "{taught}");
    // Synced on the operator's request, but the file states it, not the person.
    let workspace = daemon.store_path().with_file_name("workspace");
    let notes = workspace.join(".aether/knowledge");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("ci.md"),
        "- **[constraint]** Merge only on green CI\n",
    )
    .unwrap();
    let synced = conn.call(json!({"cmd":"sync_repository_knowledge"})).await;
    assert!(synced["result"]["Ok"].is_object(), "{synced}");
    let every = conn
        .call(json!({"cmd":"memories", "query":{"kinds":["constraint"], "limit":10}}))
        .await;
    assert_eq!(
        every["result"]["Ok"]["data"].as_array().unwrap().len(),
        2,
        "{every}"
    );
    let listed = conn
        .call(json!({"cmd":"memories", "query":{"recorded_by":"operator", "kinds":["constraint"], "limit":10}}))
        .await;
    let listed = listed["result"]["Ok"]["data"].as_array().unwrap().clone();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["id"], taught);
    assert_eq!(listed[0]["provenance"]["actor"]["actor"], "user");
    let other_kind = conn
        .call(json!({"cmd":"memories", "query":{"recorded_by":"operator", "kinds":["lesson"]}}))
        .await;
    assert_eq!(
        other_kind["result"]["Ok"]["data"],
        json!([]),
        "{other_kind}"
    );
    conn.send(&json!({"req":1001, "cmd":"memories", "query":{"recorded_by":"imported_operator"}}))
        .await;
    let refused = conn.await_reply(1001).await;
    assert!(
        refused["result"]["Err"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown variant `imported_operator`"),
        "{refused}"
    );
    daemon.stop().await;
}
