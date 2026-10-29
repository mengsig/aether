#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The TCP transport: the token is the whole authorization, quiet connections
//! are dropped, the cap bounds how many are served, and everything the socket
//! answers it answers - including a replay from a sequence number.

mod support;

use std::time::Duration;

use aether_protocol::PROTOCOL_VERSION;
use aether_protocol::compat::MIN_PROTOCOL_VERSION;
use serde_json::json;
use support::{RawConn, TestDaemon};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn the_token_is_the_whole_authorization() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let addr = daemon.tcp_addr();

    let mut good = RawConn::connect_tcp(addr).await.expect("connect");
    let welcome = good
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("the token is accepted");
    assert_eq!(welcome["protocol_version"], PROTOCOL_VERSION);
    assert!(
        welcome["idle_timeout_secs"].is_u64(),
        "a tokened transport tells the client how quiet it may be: {welcome}"
    );

    for presented in [None, Some(""), Some("0123456789abcdef0123456789abcdeg")] {
        let mut conn = RawConn::connect_tcp(addr).await.expect("connect");
        let refusal = conn
            .hello_with(PROTOCOL_VERSION, presented)
            .await
            .expect_err("refused");
        assert_eq!(refusal["code"], "unauthorized", "{refusal}");
        assert!(
            conn.recv().await.is_none(),
            "a refused connection is closed, not left open"
        );
    }
    daemon.stop().await;
}

/// The socket authorizes on its file mode, so a token there is neither
/// needed nor rejected.
#[tokio::test]
async fn the_socket_of_a_tcp_daemon_still_needs_no_token() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let mut conn = RawConn::connect(&daemon.socket).await.expect("connect");
    let welcome = conn.hello(PROTOCOL_VERSION).await.expect("served");
    assert!(
        welcome["idle_timeout_secs"].is_null(),
        "the socket has no idle timeout: {welcome}"
    );
    daemon.stop().await;
}

/// v6 and v7 both distinguish authentication errors from version mismatches.
#[tokio::test]
async fn a_previous_version_client_reads_the_refusal_it_knows() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let mut conn = RawConn::connect_tcp(daemon.tcp_addr())
        .await
        .expect("connect");
    let refusal = conn
        .hello_with(MIN_PROTOCOL_VERSION, Some("wrong"))
        .await
        .expect_err("refused");
    assert_eq!(refusal["code"], "unauthorized", "{refusal}");
    daemon.stop().await;
}

#[tokio::test]
async fn a_quiet_connection_is_closed_and_a_pinging_one_is_not() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        listen.idle_timeout = Duration::from_secs(1);
    })
    .await;
    let addr = daemon.tcp_addr();

    let mut quiet = RawConn::connect_tcp(addr).await.expect("connect");
    quiet
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    let notice = quiet.recv().await.expect("a reason, not a bare close");
    assert_eq!(notice["result"]["Err"]["code"], "unavailable", "{notice}");
    assert!(
        notice["result"]["Err"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ping"),
        "the notice says how to hold a quiet connection open: {notice}"
    );
    assert!(quiet.recv().await.is_none(), "and then it is closed");

    let mut talking = RawConn::connect_tcp(addr).await.expect("connect");
    talking
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let pong = talking.call(json!({"cmd": "ping"})).await;
        assert_eq!(pong["result"]["Ok"]["reply"], "ok", "{pong}");
    }
    daemon.stop().await;
}

/// A request still running is not a quiet connection: the idle clock waits for
/// its reply, then starts again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_awaiting_a_slow_reply_is_not_idle() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        listen.idle_timeout = Duration::from_secs(1);
    })
    .await;
    let mut conn = RawConn::connect_tcp(daemon.tcp_addr())
        .await
        .expect("connect");
    conn.hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    let locker = rusqlite::Connection::open(daemon.store_path()).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let write = conn
        .request(
            json!({"cmd": "create_agent", "role": "idle-test", "start": false,
            "model": {"provider": "claude-code", "model": "haiku"}}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    locker.execute_batch("ROLLBACK").unwrap();

    let reply = conn
        .recv()
        .await
        .expect("the write's reply, not an idle close");
    assert_eq!(reply["id"], write, "{reply}");
    let started = std::time::Instant::now();
    let notice = conn.recv().await.expect("an idle notice once it answered");
    assert_eq!(notice["id"], 0, "{notice}");
    assert_eq!(notice["result"]["Err"]["code"], "unavailable", "{notice}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the idle clock starts again after the reply"
    );
    daemon.stop().await;
}

/// The cap is on connections served at once: the one over it waits in the
/// backlog and is served the moment a slot frees, rather than being dropped.
#[tokio::test]
async fn the_connection_cap_bounds_what_is_served_at_once() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| listen.max_connections = 1).await;
    let addr = daemon.tcp_addr();

    let mut first = RawConn::connect_tcp(addr).await.expect("connect");
    first
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");

    let mut waiting = RawConn::connect_tcp(addr).await.expect("connect");
    let asked = waiting
        .request(support::hello_frame(PROTOCOL_VERSION, Some(TOKEN)))
        .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(400), waiting.recv())
            .await
            .is_err(),
        "the cap holds the second connection back"
    );

    drop(first);
    let welcome =
        support::read_answer(waiting.await_reply(asked).await).expect("served once a slot frees");
    assert_eq!(welcome["protocol_version"], PROTOCOL_VERSION);
    daemon.stop().await;
}

/// A peer that connects and says nothing must not keep its slot on the
/// strength of having said nothing. Before the handshake deadline, the only
/// bound on it was the 300s idle timeout.
#[tokio::test]
async fn a_peer_that_never_greets_is_dropped_on_the_handshake_deadline() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        listen.max_connections = 1;
        listen.handshake_timeout = Duration::from_secs(1);
    })
    .await;
    let addr = daemon.tcp_addr();
    let mut silent = RawConn::connect_tcp(addr).await.expect("connect");

    let mut waiting = RawConn::connect_tcp(addr).await.expect("connect");
    let welcome = tokio::time::timeout(
        Duration::from_secs(15),
        waiting.hello_with(PROTOCOL_VERSION, Some(TOKEN)),
    )
    .await
    .expect("the silent peer's slot came free")
    .expect("served");
    assert_eq!(welcome["protocol_version"], PROTOCOL_VERSION);
    assert!(
        silent.is_closed().await,
        "the silent peer was closed, not answered"
    );
    daemon.stop().await;
}

/// The cap is on connections that authorized. A peer still handshaking holds
/// a slot in a separate pool, so it cannot displace a client that has a
/// token.
#[tokio::test]
async fn an_ungreeted_peer_does_not_hold_an_authorized_connection_slot() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| listen.max_connections = 2).await;
    let addr = daemon.tcp_addr();
    let _silent = RawConn::connect_tcp(addr).await.expect("connect");

    let mut first = RawConn::connect_tcp(addr).await.expect("connect");
    first
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    let mut second = RawConn::connect_tcp(addr).await.expect("connect");
    let welcome = tokio::time::timeout(
        Duration::from_secs(5),
        second.hello_with(PROTOCOL_VERSION, Some(TOKEN)),
    )
    .await
    .expect("a peer that never greeted must not consume the cap")
    .expect("served");
    assert_eq!(welcome["protocol_version"], PROTOCOL_VERSION);
    daemon.stop().await;
}

/// A peer that presents a valid hello but finds the authenticated pool full
/// used to wait on it with no deadline and no shutdown race, so it could
/// hold its handshake permit indefinitely - the exact thing the two-pool
/// split (see the test above) was meant to prevent.
#[tokio::test]
async fn an_authorized_peer_is_refused_rather_than_parked_past_the_handshake_deadline() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        listen.max_connections = 1;
        listen.handshake_timeout = Duration::from_secs(1);
    })
    .await;
    let addr = daemon.tcp_addr();

    let mut first = RawConn::connect_tcp(addr).await.expect("connect");
    first
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");

    let mut second = RawConn::connect_tcp(addr).await.expect("connect");
    let refusal = tokio::time::timeout(
        Duration::from_secs(15),
        second.hello_with(PROTOCOL_VERSION, Some(TOKEN)),
    )
    .await
    .expect("the handshake deadline must bound the wait, not hang forever")
    .expect_err("the authenticated pool is full");
    assert_eq!(refusal["code"], "unavailable", "{refusal}");
    assert!(
        second.recv().await.is_none(),
        "a refused connection is closed, not left open"
    );
    daemon.stop().await;
}

/// A line that does not decode used to refresh the idle timeout and take the
/// `continue` before the hello gate, so a peer could hold a slot forever by
/// sending rubbish periodically.
#[tokio::test]
async fn rubbish_before_the_hello_closes_the_connection_rather_than_resetting_it() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let mut conn = RawConn::connect_tcp(daemon.tcp_addr())
        .await
        .expect("connect");
    conn.send_line("{\"nonsense\": true}").await;
    let refusal = conn.recv().await.expect("a refusal, not silence");
    assert_eq!(
        refusal["result"]["Err"]["code"], "invalid_argument",
        "{refusal}"
    );
    assert!(
        conn.is_closed().await,
        "and the connection is closed, not left to try again"
    );
    daemon.stop().await;
}

/// The frame cap before the hello is small, so an unauthenticated peer cannot
/// ask the daemon to buffer 64 MB for it; it is raised once authorized.
#[tokio::test]
async fn the_frame_cap_is_small_before_the_hello_and_full_after_it() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let addr = daemon.tcp_addr();
    let big = "x".repeat(128 * 1024);

    let mut unauthorized = RawConn::connect_tcp(addr).await.expect("connect");
    unauthorized.send_line(&big).await;
    assert!(
        unauthorized.is_closed().await,
        "a pre-auth line past the small cap closes the connection"
    );

    let mut authorized = RawConn::connect_tcp(addr).await.expect("connect");
    authorized
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    let pong = authorized
        .call(json!({"cmd": "ping", "padding": big}))
        .await;
    assert_eq!(
        pong["result"]["Ok"]["reply"], "ok",
        "the same length is an ordinary frame once authorized: {pong}"
    );
    daemon.stop().await;
}

/// What makes the transport worth having: a second process can catch up on
/// everything that happened before it connected.
#[tokio::test]
async fn events_replay_from_a_sequence_number_over_tcp() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;

    let mut over_socket = RawConn::connect(&daemon.socket).await.expect("connect");
    over_socket.hello(PROTOCOL_VERSION).await.expect("served");
    let created = over_socket
        .call(json!({"cmd": "create_agent", "role": "watched", "start": false, "model": {"provider": "claude-code", "model": "haiku"}}))
        .await;
    let agent = created["result"]["Ok"]["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("an agent id: {created}"))
        .to_owned();

    let mut over_tcp = RawConn::connect_tcp(daemon.tcp_addr())
        .await
        .expect("connect");
    over_tcp
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .expect("served");
    let subscribed = over_tcp
        .call(json!({"cmd": "subscribe", "after": 0, "query": {}, "deltas": false}))
        .await;
    assert_eq!(
        subscribed["result"]["Ok"]["reply"], "subscribed",
        "{subscribed}"
    );

    // Everything from sequence 0, including what happened before this
    // connection existed.
    let mut saw_the_agent = false;
    for _ in 0..64 {
        let Some(frame) = over_tcp.recv().await else {
            break;
        };
        if frame["frame"] == "event" && frame["event"]["agent_id"] == agent.as_str() {
            saw_the_agent = true;
            break;
        }
    }
    assert!(
        saw_the_agent,
        "a replay from sequence 0 carries the agent created before the subscription"
    );
    daemon.stop().await;
}

/// The daemon explains an idle drop before it closes. The client keeps that
/// explanation, so the caller is not left with a bare disconnect.
#[tokio::test]
async fn a_client_dropped_for_idleness_is_told_why() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        // Under the keepalive floor, so this client says nothing at all.
        listen.idle_timeout = Duration::from_secs(1);
    })
    .await;
    let client = aether_client::Client::connect_tcp(daemon.tcp_addr(), TOKEN)
        .await
        .expect("connect");
    for _ in 0..40 {
        if !client.is_connected() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!client.is_connected(), "the quiet connection was closed");
    let why = client.disconnect_reason().expect("the daemon said why");
    assert_eq!(why.code, aether_protocol::ErrorCode::Unavailable, "{why:?}");
    assert!(why.message.contains("ping"), "{why:?}");
    daemon.stop().await;
}

/// The client crate over TCP: the same handshake, and a connection that
/// outlives the idle timeout because the client keeps it alive itself.
#[tokio::test]
async fn the_client_crate_attaches_over_tcp_and_stays_attached() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |listen| {
        listen.idle_timeout = Duration::from_secs(3);
    })
    .await;
    let addr = daemon.tcp_addr();

    let refused = aether_client::Client::connect_tcp(addr, "wrong")
        .await
        .expect_err("the wrong token is refused");
    assert!(
        matches!(refused, aether_client::ClientError::Unauthorized(_)),
        "{refused:?}"
    );

    let client = aether_client::Client::connect_tcp(addr, TOKEN)
        .await
        .expect("connect");
    let welcome = client.welcome().expect("handshaked").clone();
    assert_eq!(welcome.idle_timeout_secs, Some(3));
    client.status().await.expect("status over tcp");

    // Say nothing for longer than the idle timeout: the keepalive is what
    // keeps this from being closed underneath the caller.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(client.is_connected(), "the keepalive held the connection");
    client.status().await.expect("still attached");
    daemon.stop().await;
}

#[tokio::test]
async fn tcp_operator_actions_keep_the_connection_origin_including_approvals() {
    use aether_core::agent::Agent;
    use aether_core::event::{Actor, EventDraft, EventKind};
    use aether_core::id::{ApprovalId, ToolCallId};
    use aether_core::time::now;
    use aether_core::tool::{ApprovalRequest, ApprovalStatus, SideEffectClass};
    use aether_store::{Commit, Mutation, SqliteStore, Store};

    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let mut remote = RawConn::connect_tcp(daemon.tcp_addr()).await.unwrap();
    remote
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .unwrap();
    let created = remote
        .call(json!({"cmd":"create_agent","role":"audited","start":false,"model":{"provider":"claude-code","model":"haiku"}}))
        .await;
    let agent: Agent = serde_json::from_value(created["result"]["Ok"]["data"].clone()).unwrap();
    let approval = ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: agent.id,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "bash".into(),
        description: "Approve fixture".into(),
        argument: Some("git status".into()),
        resources: vec![],
        side_effect: SideEffectClass::External,
        requested_at: now(),
        status: ApprovalStatus::Pending,
    };
    let store = SqliteStore::open(daemon.socket.parent().unwrap().join("aether.db")).unwrap();
    store
        .commit(
            Commit::new()
                .mutate(Mutation::PutApproval(approval.clone()))
                .event(
                    EventDraft::new(Actor::agent(agent.id), EventKind::AgentStarted { turn: 1 })
                        .agent(agent.id)
                        .run(agent.run),
                )
                .event(
                    EventDraft::new(
                        Actor::agent(agent.id),
                        EventKind::ApprovalRequested {
                            approval: approval.id,
                            call: approval.call_id,
                            tool: approval.tool.clone(),
                            description: approval.description.clone(),
                            argument: approval.argument.clone(),
                            side_effect: approval.side_effect,
                        },
                    )
                    .agent(agent.id)
                    .run(agent.run),
                ),
        )
        .unwrap();
    let resolved = remote
        .call(json!({"cmd":"resolve_approval","id":approval.id,"decision":"approved"}))
        .await;
    assert_eq!(resolved["result"]["Ok"]["reply"], "ok", "{resolved}");
    let events = remote
        .call(json!({"cmd":"get_events","after":0,"limit":1000,"query":{"agent":agent.id}}))
        .await;
    let events = events["result"]["Ok"]["data"].as_array().unwrap();
    let created = events
        .iter()
        .find(|e| e["kind"]["type"] == "agent.created")
        .unwrap();
    let resolved = events
        .iter()
        .find(|e| e["kind"]["type"] == "approval.resolved")
        .unwrap();
    let actor = &resolved["actor"];
    assert_eq!(actor["actor"], "user");
    assert_eq!(actor["origin"]["transport"], "tcp");
    let peer: std::net::SocketAddr = actor["origin"]["peer"].as_str().unwrap().parse().unwrap();
    assert_eq!(Some(peer), remote.local_addr);
    assert!(!actor["origin"]["connection"].as_str().unwrap().is_empty());
    assert_eq!(
        &created["actor"], actor,
        "one connection retains its identity"
    );
    assert_eq!(&resolved["kind"]["by"], actor);
    assert!(!serde_json::to_string(events).unwrap().contains(TOKEN));

    let replay = remote.call(json!({"cmd":"replay","agent":agent.id})).await;
    assert_eq!(
        &replay["result"]["Ok"]["data"][0]["approvals"][0]["decided_by"], actor,
        "{replay}"
    );
    let replay_actor: Actor = serde_json::from_value(actor.clone()).unwrap();
    let report = remote
        .call(json!({"cmd":"get_run_report","run":agent.run}))
        .await;
    let report: aether_core::report::RunReport =
        serde_json::from_value(report["result"]["Ok"]["data"].clone()).unwrap();
    assert!(report.to_markdown().contains(&replay_actor.label()));

    let mut local = RawConn::connect(&daemon.socket).await.unwrap();
    local.hello(PROTOCOL_VERSION).await.unwrap();
    let mut second_remote = RawConn::connect_tcp(daemon.tcp_addr()).await.unwrap();
    second_remote
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .unwrap();
    for (conn, transport) in [(&mut local, "local_socket"), (&mut second_remote, "tcp")] {
        let sent = conn
            .call(json!({"cmd":"send_message","to":agent.id,"content":"operator correction"}))
            .await;
        assert_eq!(sent["result"]["Ok"]["reply"], "message_sent", "{sent}");
        let inbox = conn
            .call(json!({"cmd":"get_inbox","agent":agent.id,"after":0}))
            .await;
        let messages = inbox["result"]["Ok"]["data"].as_array().unwrap();
        let from = &messages.last().unwrap()["kind"]["from"];
        assert_eq!(from["actor"], "user", "{inbox}");
        assert_eq!(from["origin"]["transport"], transport);
        assert_ne!(from["origin"]["connection"], actor["origin"]["connection"]);
    }
    daemon.stop().await;
}

#[tokio::test]
async fn tcp_lifecycle_commands_preserve_the_operator_origin() {
    let daemon = TestDaemon::start_with_tcp(TOKEN, |_| {}).await;
    let mut remote = RawConn::connect_tcp(daemon.tcp_addr()).await.unwrap();
    remote
        .hello_with(PROTOCOL_VERSION, Some(TOKEN))
        .await
        .unwrap();
    let created = remote
        .call(json!({"cmd":"create_agent","role":"audited","start":false,"model":{"provider":"claude-code","model":"default"},"approval":{"require_for":["external"]}}))
        .await;
    let agent = created["result"]["Ok"]["data"]["id"].clone();
    assert!(agent.is_string(), "{created}");
    for command in [
        json!({"cmd":"pause","agent":agent}),
        json!({"cmd":"send_message","to":agent,"content":"operator correction"}),
        json!({"cmd":"set_approvals","agent":agent,"policy":{"require_for":[]}}),
        json!({"cmd":"trigger","agent":agent,"source":"operator-test","payload":{"ready":true}}),
        json!({"cmd":"cancel","agent":agent,"reason":"test finished"}),
        json!({"cmd":"archive","target":{"agent":agent}}),
    ] {
        let reply = remote.call(command.clone()).await;
        assert!(reply["result"].get("Ok").is_some(), "{command}: {reply}");
    }
    let events = remote
        .call(json!({"cmd":"get_events","after":0,"limit":1000,"query":{"agent":agent}}))
        .await;
    let events = events["result"]["Ok"]["data"].as_array().unwrap();
    let actor = &events
        .iter()
        .find(|e| e["kind"]["type"] == "agent.created")
        .unwrap()["actor"];
    assert_eq!(actor["origin"]["transport"], "tcp");
    for kind in [
        "agent.paused",
        "message.sent",
        "agent.config_updated",
        "agent.cancelled",
        "agent.archived",
    ] {
        let event = events
            .iter()
            .find(|e| e["kind"]["type"] == kind)
            .unwrap_or_else(|| panic!("missing {kind}: {events:?}"));
        assert_eq!(&event["actor"], actor, "{kind}");
    }
    let trigger = events
        .iter()
        .find(|e| {
            e["kind"]["type"] == "inbox.delivered"
                && e["kind"]["item"]["kind"]["kind"] == "external_trigger"
        })
        .unwrap();
    assert_eq!(&trigger["actor"], actor);
    daemon.stop().await;
}
