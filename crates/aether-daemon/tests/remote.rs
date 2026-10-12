#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Remote control end to end: a real daemon, its listener on loopback, and
//! a device that is a P-256 key speaking HTTP and WebSocket.

#[path = "remote/phone.rs"]
mod phone;
mod support;

use phone::{Host, Phone, Socket, http, post, post_from};
use serde_json::{Value, json};
use tokio::net::TcpStream;

const UNAUTHORIZED: u16 = 4401;
const REVOKED: u16 = 4403;
const DISABLED: u16 = 4503;
const TOO_BIG: u16 = 1009;

/// What both sides of a finished pairing agreed on.
struct Paired {
    device: String,
    fingerprint: Value,
}

/// Pair `phone` step by step, checking what each side is told on the way.
async fn pair_watching_both_sides(host: &mut Host, phone: &Phone) -> Paired {
    let pairing = host.ok(json!({"cmd": "start_pairing"})).await;
    let code = pairing["code"].as_str().expect("the code").to_owned();
    assert_eq!((code.len(), &code[4..5]), (9, "-"), "{code}");
    let url = pairing["url"].as_str().expect("the url");
    assert_eq!(url, format!("{}/pair#code={code}", host.origin()));
    let health = http(host.listening(), &get(host, "/api/health")).await;
    assert_eq!(health.body["pairing_open"], true, "{}", health.body);

    let claim = phone.claim(&code, "Kim's phone");
    let claimed = http(host.listening(), &post(host, "/api/pair/claim", &claim)).await;
    assert_eq!(claimed.status, 200, "{}", claimed.body);
    assert_eq!(claimed.body["pairing"], pairing["id"]);
    let status = json!({"pairing": pairing["id"]});
    let waiting = http(host.listening(), &post(host, "/api/pair/status", &status)).await;
    assert_eq!(waiting.body, json!({"state": "claimed"}));

    // The host sees the same fingerprint and phrase the device was shown.
    let seen = host
        .ok(json!({"cmd": "get_pairing", "id": pairing["id"]}))
        .await;
    assert_eq!(seen["state"], "claimed");
    assert_eq!(seen["code"], Value::Null, "the code is never repeated");
    assert_eq!(seen["candidate"]["name"], "Kim's phone");
    assert_eq!(seen["candidate"]["phrase"], claimed.body["phrase"]);
    assert_eq!(
        seen["candidate"]["fingerprint"],
        claimed.body["fingerprint"]
    );
    let confirm = json!({"cmd": "confirm_pairing", "id": pairing["id"], "accept": true});
    let enrolled = host.ok(confirm.clone()).await;
    assert_eq!(enrolled["state"], "enrolled");
    assert_eq!(
        host.ok(confirm).await,
        enrolled,
        "confirming again changes nothing"
    );
    let told = http(host.listening(), &post(host, "/api/pair/status", &status)).await;
    assert_eq!(told.body["state"], "enrolled");
    assert_eq!(told.body["device"], enrolled["device"]);
    Paired {
        device: enrolled["device"]["id"]
            .as_str()
            .expect("a device id")
            .to_owned(),
        fingerprint: claimed.body["fingerprint"].clone(),
    }
}

/// What a connected device may ask, and what the host sees of it meanwhile.
async fn check_the_connected_device(host: &mut Host, socket: &mut Socket, paired: &Paired) {
    let device = paired.device.as_str();
    let listed = socket.call(json!({"cmd": "list_devices"})).await;
    let own = &listed["result"]["Ok"]["data"][0];
    assert_eq!(
        (&own["id"], &own["current"]),
        (&json!(device), &json!(true)),
        "{listed}"
    );
    assert_eq!(
        own["fingerprint"],
        Value::Null,
        "a device is not told fingerprints"
    );
    for host_only in [
        json!({"cmd": "get_remote"}),
        json!({"cmd": "revoke_device", "id": device}),
        json!({"cmd": "list_devices", "include_revoked": true}),
    ] {
        let refused = socket.call(host_only.clone()).await;
        assert_eq!(
            refused["result"]["Err"]["code"], "forbidden",
            "{host_only}: {refused}"
        );
    }

    let devices = host.ok(json!({"cmd": "list_devices"})).await;
    let record = &devices[0];
    assert_eq!(record["fingerprint"], paired.fingerprint);
    assert_eq!(
        (&record["connections"], &record["current"]),
        (&json!(1), &json!(false))
    );
    assert!(record["last_seen_at"].is_string(), "{record}");
    assert_eq!(
        host.ok(json!({"cmd": "get_remote"})).await["connections"],
        1
    );
}

/// The key is still the device's, and no longer opens anything.
async fn check_the_revoked_device_is_shut_out(host: &mut Host, phone: &Phone, device: &str) {
    let mut again = Socket::open(host.listening(), &host.origin()).await;
    let proof = phone.proof(&again, &host.origin(), device);
    let refused = again
        .hello(Some(proof))
        .await
        .expect_err("a revoked device");
    assert_eq!(refused["code"], "unauthorized");
    assert_eq!(again.closed().await.1, UNAUTHORIZED);
    assert_eq!(host.ok(json!({"cmd": "list_devices"})).await, json!([]));
    let all = host
        .ok(json!({"cmd": "list_devices", "include_revoked": true}))
        .await;
    assert_eq!(all[0]["id"], device);
}

async fn check_the_audit_trail(host: &mut Host, paired: &Paired) {
    let device = paired.device.as_str();
    assert_eq!(host.audit("remote.configured").await.len(), 1);
    assert_eq!(host.audit("remote.pairing_started").await.len(), 1);
    let enrolments = host.audit("remote.device_enrolled").await;
    assert_eq!(enrolments[0]["fingerprint"], paired.fingerprint);
    assert_eq!(
        host.audit("remote.device_authenticated").await[0]["device"],
        device
    );
    let revocations = host.audit("remote.device_revoked").await;
    assert_eq!(
        revocations,
        [json!({"device": device, "name": "Kim's phone", "reason": "lost"})]
    );
    assert_eq!(
        host.audit("remote.device_refused").await[0]["reason"],
        "the device was revoked"
    );
}

#[tokio::test]
async fn a_device_pairs_connects_runs_a_command_and_is_revoked() {
    let mut host = Host::enabled().await;
    let phone = Phone::new();
    let paired = pair_watching_both_sides(&mut host, &phone).await;
    let device = paired.device.clone();
    let mut socket = host.connect(&phone, &device).await;
    check_the_connected_device(&mut host, &mut socket, &paired).await;

    let revoke = json!({"cmd": "revoke_device", "id": device, "reason": "lost"});
    let revoked = host.ok(revoke.clone()).await;
    assert!(revoked["revoked_at"].is_string(), "{revoked}");
    assert_eq!(
        revoked["connections"], 0,
        "its session ended with the reply"
    );
    let (frames, code) = socket.closed().await;
    assert_eq!(code, REVOKED);
    let notice = frames.last().expect("the daemon says why first");
    assert_eq!(
        (&notice["id"], &notice["result"]["Err"]["code"]),
        (&json!(0), &json!("forbidden"))
    );
    assert_eq!(
        host.ok(revoke).await,
        revoked,
        "revoking again changes nothing"
    );

    check_the_revoked_device_is_shut_out(&mut host, &phone, &device).await;
    check_the_audit_trail(&mut host, &paired).await;
    host.daemon.stop().await;
}

/// A claim of `code` by a device nobody has seen before.
fn claim(host: &Host, code: &str) -> String {
    post(host, "/api/pair/claim", &Phone::new().claim(code, "phone"))
}

fn get(host: &Host, path: &str) -> String {
    let listening = host.listening();
    format!("GET {path} HTTP/1.1\r\nHost: {listening}\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn a_foreign_host_or_origin_reaches_nothing() {
    let mut host = Host::enabled().await;
    let listening = host.listening().to_string();
    let pairing = host.ok(json!({"cmd": "start_pairing"})).await;
    let claim = Phone::new().claim(pairing["code"].as_str().expect("the code"), "phone");

    let rebound = post_from("evil.example", &host.origin(), "/api/pair/claim", &claim);
    let rebound = http(host.listening(), &rebound).await;
    assert_eq!(
        (rebound.status, rebound.error_code()),
        (403, "forbidden_host")
    );
    let cross_site = post_from(
        &listening,
        "https://evil.example",
        "/api/pair/claim",
        &claim,
    );
    let cross_site = http(host.listening(), &cross_site).await;
    assert_eq!(
        (cross_site.status, cross_site.error_code()),
        (403, "forbidden_origin")
    );
    assert!(
        cross_site
            .head
            .contains("content-security-policy: default-src 'none'")
    );
    assert!(cross_site.head.contains("x-content-type-options: nosniff"));

    let hijack = Socket::try_open(host.listening(), &listening, "https://evil.example").await;
    assert_eq!(hijack.err(), Some(403));
    let rebound = Socket::try_open(host.listening(), "evil.example", &host.origin()).await;
    assert_eq!(rebound.err(), Some(403));

    // None of it spent the code: the page itself still pairs.
    let claimed = http(host.listening(), &post(&host, "/api/pair/claim", &claim)).await;
    assert_eq!(claimed.status, 200, "{}", claimed.body);
    host.daemon.stop().await;
}

#[tokio::test]
async fn a_challenge_is_answered_once_by_the_right_key_on_its_own_connection() {
    let mut host = Host::enabled().await;
    let phone = Phone::new();
    let device = host.pair(&phone, "phone", "operator").await;
    let origin = host.origin();

    let mut first = Socket::open(host.listening(), &origin).await;
    let proof = phone.proof(&first, &origin, &device);
    first
        .hello(Some(proof.clone()))
        .await
        .expect("the right answer");

    // The same answer on another connection answers another challenge.
    let mut replayed = Socket::open(host.listening(), &origin).await;
    assert_ne!(replayed.nonce, first.nonce);
    let refused = replayed.hello(Some(proof)).await.expect_err("a replay");
    assert_eq!(
        refused,
        json!({"code": "unauthorized", "message": "device authentication failed"})
    );
    assert_eq!(replayed.closed().await.1, UNAUTHORIZED);

    let mut forged = Socket::open(host.listening(), &origin).await;
    let wrong_key = Phone::new().proof(&forged, &origin, &device);
    forged
        .hello(Some(wrong_key))
        .await
        .expect_err("another key");
    assert_eq!(forged.closed().await.1, UNAUTHORIZED);

    let mut elsewhere = Socket::open(host.listening(), &origin).await;
    let other_origin = phone.proof(&elsewhere, "https://evil.example", &device);
    elsewhere
        .hello(Some(other_origin))
        .await
        .expect_err("signed for another origin");
    assert_eq!(elsewhere.closed().await.1, UNAUTHORIZED);

    let mut anonymous = Socket::open(host.listening(), &origin).await;
    anonymous.hello(None).await.expect_err("no proof at all");
    assert_eq!(anonymous.closed().await.1, UNAUTHORIZED);

    let refusals = host.audit("remote.device_refused").await;
    assert_eq!(
        refusals.len(),
        3,
        "each checked answer is recorded: {refusals:?}"
    );
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal["device"] == device.as_str())
    );
    // The first connection never noticed.
    let pong = first.call(json!({"cmd": "ping"})).await;
    assert!(pong["result"].get("Ok").is_some(), "{pong}");
    host.daemon.stop().await;
}

#[tokio::test]
async fn a_flood_of_refused_hellos_adds_only_a_few_audit_events() {
    const HELLOS: usize = 12;
    const RECORDED: usize = 5;
    let mut host = Host::enabled().await;
    let device = host.pair(&Phone::new(), "phone", "operator").await;
    let origin = host.origin();
    for _ in 0..HELLOS {
        let mut forged = Socket::open(host.listening(), &origin).await;
        let wrong_key = Phone::new().proof(&forged, &origin, &device);
        forged
            .hello(Some(wrong_key))
            .await
            .expect_err("another key");
        assert_eq!(forged.closed().await.1, UNAUTHORIZED);
    }
    let refusals = host.audit("remote.device_refused").await;
    assert_eq!(refusals.len(), RECORDED, "{refusals:?}");
    for refusal in &refusals {
        assert_eq!(refusal["device"], device.as_str());
        assert_eq!(refusal["unrecorded"], 0, "nothing went uncounted before it");
    }
    host.daemon.stop().await;
}

#[tokio::test]
async fn a_code_pairs_one_device_and_guessing_is_limited() {
    let mut host = Host::enabled().await;
    let pairing = host
        .ok(json!({"cmd": "start_pairing", "role": "observer"}))
        .await;
    let code = pairing["code"].as_str().expect("the code").to_owned();

    let wrong = http(host.listening(), &claim(&host, "0000-0000")).await;
    assert_eq!((wrong.status, wrong.error_code()), (403, "invalid_code"));
    let first = http(host.listening(), &claim(&host, &code)).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let reused = http(host.listening(), &claim(&host, &code)).await;
    assert_eq!((reused.status, reused.error_code()), (403, "invalid_code"));

    let reject = json!({"cmd": "confirm_pairing", "id": pairing["id"], "accept": false});
    assert_eq!(host.ok(reject).await["state"], "rejected");
    let accept = json!({"cmd": "confirm_pairing", "id": pairing["id"], "accept": true});
    assert_eq!(host.err(accept).await["code"], "invalid_transition");
    assert_eq!(host.ok(json!({"cmd": "list_devices"})).await, json!([]));
    assert_eq!(host.audit("remote.pairing_rejected").await.len(), 1);

    let cancelled = host.ok(json!({"cmd": "start_pairing"})).await;
    let cancel = json!({"cmd": "cancel_pairing", "id": cancelled["id"]});
    assert_eq!(host.ok(cancel).await["state"], "cancelled");
    let code = cancelled["code"].as_str().expect("the code").to_owned();
    let closed = http(host.listening(), &claim(&host, &code)).await;
    assert_eq!(
        (closed.status, closed.error_code()),
        (403, "pairing_closed")
    );
    let unknown = host
        .err(json!({"cmd": "get_pairing", "id": "no-such-pairing"}))
        .await;
    assert_eq!(unknown["code"], "not_found");

    flooded_with_guesses(&mut host).await;
    host.daemon.stop().await;
}

/// Far more wrong codes than a second allows: the surplus is turned away,
/// and the pairing is still there for the right one.
async fn flooded_with_guesses(host: &mut Host) {
    const GUESSES: usize = 40;
    let pairing = host.ok(json!({"cmd": "start_pairing"})).await;
    let code = pairing["code"].as_str().expect("the code").to_owned();
    let guesses: Vec<String> = (0..GUESSES).map(|_| claim(host, "0000-0000")).collect();
    let listening = host.listening();
    let asked = guesses.iter().map(|guess| http(listening, guess));
    let answers = futures::future::join_all(asked).await;
    assert_eq!(answers.len(), GUESSES);
    let limited: Vec<_> = answers.iter().filter(|a| a.status == 429).collect();
    assert!(!limited.is_empty(), "nothing limited {GUESSES} guesses");
    for answer in &limited {
        assert_eq!(answer.error_code(), "rate_limited");
        let waits = answer.head.lines().any(|line| line == "retry-after: 1");
        assert!(waits, "{}", answer.head);
    }
    let wrong = answers.iter().filter(|a| a.status == 403);
    assert_eq!(wrong.clone().count() + limited.len(), GUESSES);
    assert!(wrong.clone().all(|a| a.error_code() == "invalid_code"));

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let claimed = http(listening, &claim(host, &code)).await;
    assert_eq!(
        claimed.status, 200,
        "wrong guesses closed it: {}",
        claimed.body
    );
}

#[tokio::test]
async fn an_oversized_frame_before_the_hello_closes_with_1009() {
    let host = Host::enabled().await;
    let mut socket = Socket::open(host.listening(), &host.origin()).await;
    let oversized = json!({"cmd": "hello", "client_name": "x".repeat(70 * 1024)});
    socket.send_text(oversized.to_string()).await;
    let (frames, code) = socket.closed().await;
    assert_eq!(code, TOO_BIG);
    let notice = frames.last().expect("the daemon says why first");
    assert_eq!(notice["id"], 0, "{notice}");
    assert_eq!(
        notice["result"]["Err"]["code"], "invalid_argument",
        "{notice}"
    );
    host.daemon.stop().await;
}

#[tokio::test]
async fn a_stale_or_invalid_change_is_refused_and_changes_nothing() {
    let mut host = Host::enabled().await;
    let stale = json!({"cmd": "configure_remote", "settings": {}, "expected_revision": 0});
    let refusal = host.err(stale).await;
    assert_eq!(refusal["code"], "invalid_argument");
    let reply: aether_protocol::ErrorReply = serde_json::from_value(refusal).expect("a refusal");
    assert!(
        aether_protocol::remote::is_stale_revision(&reply),
        "{reply}"
    );
    for origin in [
        "http://plain.example",
        "https://box.example/app",
        "https://BOX.example",
    ] {
        let settings = json!({"enabled": true, "public_origins": [origin]});
        let change =
            json!({"cmd": "configure_remote", "settings": settings, "expected_revision": 1});
        let refusal = host.err(change).await;
        assert_eq!(refusal["code"], "invalid_argument", "{origin}");
        let reply: aether_protocol::ErrorReply = serde_json::from_value(refusal).expect("reply");
        assert!(
            !aether_protocol::remote::is_stale_revision(&reply),
            "{reply}"
        );
    }
    let unchanged = host.ok(json!({"cmd": "get_remote"})).await;
    assert_eq!(unchanged["revision"], 1, "{unchanged}");
    assert_eq!(
        unchanged["settings"]["public_origins"],
        json!([]),
        "{unchanged}"
    );
    assert_eq!(host.audit("remote.configured").await.len(), 1);
    host.daemon.stop().await;
}

#[tokio::test]
async fn settings_open_and_close_the_listener_without_a_restart() {
    let mut host = Host::start().await;
    let closed = host.ok(json!({"cmd": "get_remote"})).await;
    assert_eq!(
        (&closed["revision"], &closed["listening"]),
        (&json!(0), &Value::Null)
    );
    assert_eq!(closed["settings"]["enabled"], false);
    let early = host.err(json!({"cmd": "start_pairing"})).await;
    assert_eq!(early["code"], "invalid_argument", "{early}");

    let enabled = host
        .configure(json!({"enabled": true, "pairing": true}))
        .await;
    assert_eq!(enabled["revision"], 1);
    let bound = host.listening();
    assert!(bound.ip().is_loopback() && bound.port() != 0, "{bound}");

    let phone = Phone::new();
    let device = host.pair(&phone, "phone", "operator").await;
    let socket = host.connect(&phone, &device).await;
    let pairing = host.ok(json!({"cmd": "start_pairing"})).await;

    let disabled = host
        .configure(json!({"enabled": false, "pairing": true}))
        .await;
    assert_eq!(
        (&disabled["revision"], &disabled["listening"]),
        (&json!(2), &Value::Null)
    );
    let (frames, code) = socket.closed().await;
    assert_eq!(code, DISABLED);
    assert_eq!(
        frames.last().expect("a notice")["result"]["Err"]["code"],
        "unavailable"
    );
    assert!(
        TcpStream::connect(bound).await.is_err(),
        "the port is released"
    );
    let open = host
        .ok(json!({"cmd": "get_pairing", "id": pairing["id"]}))
        .await;
    assert_eq!(
        open["state"], "cancelled",
        "disabling cancels the open pairing"
    );

    // Enabled again with pairing off: the device still connects, no new one can.
    host.configure(json!({"enabled": true, "pairing": false}))
        .await;
    let mut back = host.connect(&phone, &device).await;
    let pong = back.call(json!({"cmd": "ping"})).await;
    assert!(pong["result"].get("Ok").is_some(), "{pong}");
    let no_pairing = host.err(json!({"cmd": "start_pairing"})).await;
    assert_eq!(no_pairing["code"], "invalid_argument", "{no_pairing}");
    assert_eq!(host.audit("remote.configured").await.len(), 3);
    host.daemon.stop().await;
}

#[tokio::test]
async fn a_rename_keeps_connections_and_a_new_role_ends_them() {
    let mut host = Host::enabled().await;
    let phone = Phone::new();
    let device = host.pair(&phone, "phone", "operator").await;
    let mut socket = host.connect(&phone, &device).await;

    let nothing = host
        .err(json!({"cmd": "update_device", "id": device}))
        .await;
    assert_eq!(nothing["code"], "invalid_argument");
    let blank = json!({"cmd": "update_device", "id": device, "name": "  "});
    assert_eq!(host.err(blank).await["code"], "invalid_argument");
    let renamed = host
        .ok(json!({"cmd": "update_device", "id": device, "name": "Kim's phone"}))
        .await;
    assert_eq!(
        (&renamed["name"], &renamed["connections"]),
        (&json!("Kim's phone"), &json!(1))
    );
    let pong = socket.call(json!({"cmd": "ping"})).await;
    assert!(
        pong["result"].get("Ok").is_some(),
        "a rename leaves the connection: {pong}"
    );

    let demoted = host
        .ok(json!({"cmd": "update_device", "id": device, "role": "observer"}))
        .await;
    assert_eq!(demoted["role"], "observer");
    assert_eq!(socket.closed().await.1, REVOKED);
    let observer = host.connect(&phone, &device).await;
    drop(observer);
    let updates = host.audit("remote.device_updated").await;
    assert_eq!(updates.len(), 2, "{updates:?}");
    assert_eq!(
        updates[1],
        json!({"device": device, "name": "Kim's phone", "role": "observer"})
    );

    host.ok(json!({"cmd": "revoke_device", "id": device})).await;
    let late = json!({"cmd": "update_device", "id": device, "name": "again"});
    assert_eq!(host.err(late).await["code"], "invalid_transition");
    let stranger = json!({"cmd": "revoke_device", "id": aether_core::id::DeviceId::new()});
    assert_eq!(host.err(stranger).await["code"], "not_found");
    host.daemon.stop().await;
}

/// What a browser's push manager hands the page on `phone`.
fn subscription(phone: &Phone) -> Value {
    json!({
        "endpoint": "https://fcm.googleapis.com/fcm/send/abc",
        "p256dh": phone.public_key(),
        "auth": "AAAAAAAAAAAAAAAAAAAAAA",
    })
}

/// Whether the host is told `device` holds a push subscription.
async fn holds_subscription(host: &mut Host, device: &str) -> bool {
    let devices = host
        .ok(json!({"cmd": "list_devices", "include_revoked": true}))
        .await;
    let listed = devices.as_array().expect("a list");
    let found = listed.iter().find(|listed| listed["id"] == device);
    found.unwrap_or_else(|| panic!("{device} is not in {devices}"))["push"] == true
}

#[tokio::test]
async fn push_follows_the_settings_and_revoking_forgets_the_subscription() {
    let mut host = Host::enabled().await;
    let phone = Phone::new();
    let device = host.pair(&phone, "phone", "operator").await;
    let mut socket = host.connect(&phone, &device).await;
    let subscribe = json!({"cmd": "subscribe_push", "subscription": subscription(&phone)});
    let off = support::read_answer(socket.call(subscribe.clone()).await);
    assert_eq!(off.expect_err("push is off")["code"], "unavailable");

    let settings = json!({"enabled": true, "pairing": true, "push": true});
    host.configure(settings).await;
    let on = support::read_answer(socket.call(subscribe).await).expect("push is on");
    assert!(on["enabled"] == true && on["subscribed"] == true, "{on}");
    assert!(on["vapid_public_key"].is_string(), "{on}");
    assert!(holds_subscription(&mut host, &device).await);

    let mut host = Host::over(host.daemon.restart().await).await;
    let mut socket = host.connect(&phone, &device).await;
    let kept = socket.call(json!({"cmd": "get_push_config"})).await;
    let kept = support::read_answer(kept).expect("a device may read it");
    assert!(
        kept["enabled"] == true && kept["subscribed"] == true,
        "{kept}"
    );
    assert_eq!(kept["vapid_public_key"], on["vapid_public_key"], "one key");

    host.ok(json!({"cmd": "revoke_device", "id": device})).await;
    assert!(!holds_subscription(&mut host, &device).await);
    host.daemon.stop().await;
}

#[tokio::test]
async fn settings_and_devices_survive_a_restart() {
    let mut host = Host::enabled().await;
    let (kept, lost) = (Phone::new(), Phone::new());
    let kept_device = host.pair(&kept, "kept", "observer").await;
    let lost_device = host.pair(&lost, "lost", "operator").await;
    host.ok(json!({"cmd": "revoke_device", "id": lost_device}))
        .await;
    let before = host
        .ok(json!({"cmd": "list_devices", "include_revoked": true}))
        .await;

    let mut host = Host::over(host.daemon.restart().await).await;
    let status = host.ok(json!({"cmd": "get_remote"})).await;
    assert_eq!(
        (&status["revision"], &status["devices"]),
        (&json!(1), &json!(1)),
        "{status}"
    );
    assert!(
        status["settings"]["enabled"] == true && status["error"].is_null(),
        "{status}"
    );
    let after = host
        .ok(json!({"cmd": "list_devices", "include_revoked": true}))
        .await;
    assert_eq!(after, before, "the records are the ones enrolled");

    let mut socket = Socket::open(host.listening(), &host.origin()).await;
    let proof = kept.proof(&socket, &host.origin(), &kept_device);
    let welcome = socket
        .hello(Some(proof))
        .await
        .expect("the device is still enrolled");
    assert_eq!(welcome["device"]["role"], "observer", "{welcome}");
    assert_eq!(welcome["device"]["name"], "kept", "{welcome}");
    let mut revoked = Socket::open(host.listening(), &host.origin()).await;
    let proof = lost.proof(&revoked, &host.origin(), &lost_device);
    revoked.hello(Some(proof)).await.expect_err("still revoked");
    assert_eq!(revoked.closed().await.1, UNAUTHORIZED);
    host.daemon.stop().await;
}
