//! F6 through the daemon: a bundle needs both switches, is refused over
//! 32 KiB, keeps its status labels into the Prober's and the Teacher's model
//! requests, and makes nothing connect to archond.

use aether_core::model::ModelRequest;
use aether_protocol::noesis::{EngineeringExtraction, ExtractionPatch};

use super::*;
use crate::noesis::teaching::ENGINEERING_CONTEXT_DISABLED;

/// The rows of `session`'s evidence bundle in `noesis.db` (N3).
pub(super) fn bundle_rows(home: &Home, session: &str) -> i64 {
    let path = home.data.path().join("noesis.db");
    let conn = rusqlite::Connection::open(path).expect("noesis.db");
    conn.query_row(
        "SELECT count(*) FROM evidence_items WHERE session = ?1",
        [session],
        |row| row.get(0),
    )
    .expect("count")
}

fn engineering_on(edit: impl FnOnce(&mut NoesisSettingsPatch)) -> NoesisSettingsPatch {
    teaching_on(|patch| {
        patch.engineering_context = Some(true);
        edit(patch);
    })
}

/// A bundle whose references point at `archond`, a guard address.
fn bundle(archond: &str) -> Value {
    let at = |path: &str| format!("http://{archond}/archon/{path}");
    json!({"context": {"title": "Ship the cache", "items": [
        {"ref": {"kind": "item", "id": "it_1", "url": at("items/it_1")},
         "excerpt": "The cache is keyed by the tree hash.", "status": "verified"},
        {"ref": {"kind": "stage", "id": "review", "url": at("items/it_1/stages/review")},
         "excerpt": "A reviewer passed the eviction rule.", "status": "reviewed"},
        {"ref": {"kind": "agent", "id": "agt_7", "url": at("agents/agt_7")},
         "excerpt": "The agent guesses the TTL is five minutes.", "status": "preliminary"}
    ]}})
}

fn excerpt_of(size: usize) -> Value {
    json!({"context": {"title": "Big", "items": [
        {"ref": {"kind": "file", "id": "notes.md"}, "excerpt": "x".repeat(size), "status": "preliminary"}
    ]}})
}

fn create_command(extra: Value) -> Value {
    let mut command = json!({
        "cmd": "noesis_create_session", "goal": "Understand the cache", "mode": "teach",
        "idempotency_key": format!("create-{}", ulid::Ulid::generate()),
    });
    merge(&mut command, extra);
    command
}

fn request_text(request: &ModelRequest) -> String {
    let blocks = request.messages.iter().flat_map(|message| &message.content);
    let texts = blocks.filter_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    });
    texts.collect::<Vec<_>>().join("\n")
}

/// Each status label comes right before the excerpt it qualifies.
fn assert_labelled(text: &str) {
    let pairs = [
        ("verified", "keyed by the tree hash"),
        ("reviewed", "passed the eviction rule"),
        ("preliminary", "TTL is five minutes"),
    ];
    for (status, excerpt) in pairs {
        let at = text
            .find(excerpt)
            .unwrap_or_else(|| panic!("`{excerpt}` reached the model"));
        let label = text[..at]
            .rfind("\"status\": ")
            .expect("a label precedes it");
        assert!(
            text[label..].starts_with(&format!("\"status\": \"{status}\"")),
            "{status}"
        );
    }
}

async fn requests_at_least(harness: &Harness, count: usize) -> Vec<ModelRequest> {
    for _ in 0..500 {
        let requests = harness.client.requests();
        if requests.len() >= count {
            return requests;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("fewer than {count} model requests were made");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_is_refused_and_nothing_sent_unless_both_switches_are_on() {
    let home = Home::new();
    let harness = Harness::open(&home, Arc::new(ScriptedClient::new())).await;
    let off = harness
        .send_json(create_command(bundle("127.0.0.1:9")))
        .await
        .expect_err("off");
    assert_eq!(
        (off.code, off.reason.as_deref()),
        (ErrorCode::Unavailable, Some("noesis_disabled"))
    );

    harness.configure(teaching_on(|_| {})).await;
    let refused = harness
        .send_json(create_command(bundle("127.0.0.1:9")))
        .await
        .expect_err("refused");
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(
        refused.reason.as_deref(),
        Some(ENGINEERING_CONTEXT_DISABLED)
    );
    let list = harness
        .send_json(json!({"cmd": "noesis_list_sessions"}))
        .await
        .expect("list");
    assert!(matches!(list, Reply::NoesisSessions { sessions, .. } if sessions.is_empty()));
    harness.settled().await;
    assert!(
        harness.client.requests().is_empty(),
        "no model call was made"
    );
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_takes_a_32_kib_bundle_and_refuses_one_byte_more() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client).await;
    harness.configure(engineering_on(|_| {})).await;
    let over = harness
        .send_json(create_command(excerpt_of(32 * 1024 + 1)))
        .await
        .expect_err("over");
    assert_eq!(over.code, ErrorCode::InvalidArgument, "{}", over.message);
    assert!(over.message.contains("32768"), "{}", over.message);

    let created = harness.create(excerpt_of(32 * 1024)).await;
    assert_eq!(bundle_rows(&home, &created.id), 1);
    harness.posed(&created.id).await;
    harness.settled().await;
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_labels_reach_the_teacher_and_nothing_connects_to_archond() {
    let archond = std::net::TcpListener::bind("127.0.0.1:0").expect("guard");
    archond.set_nonblocking(true).expect("non-blocking");
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    let strands = json!({"strands": [{"strand": "caching", "floor": "knows keys",
        "ceiling": "eviction", "misconceptions": [], "start": "cache keys"}]});
    client.push(ScriptedClient::tool_call("noesis_probe_done", strands));
    client.push(ScriptedClient::text("Here is the plan we will follow."));
    let harness = Harness::open(&home, client).await;
    harness
        .configure(engineering_on(|patch| {
            patch.extraction = Some(ExtractionPatch {
                sessions: None,
                engineering: Some(EngineeringExtraction::Auto),
            });
        }))
        .await;
    let address = archond.local_addr().expect("address").to_string();
    let created = harness.create(bundle(&address)).await;
    let requests = requests_at_least(&harness, 2).await;
    assert_labelled(&request_text(&requests[0]));
    let teacher = requests
        .iter()
        .skip(1)
        .map(request_text)
        .find(|t| t.contains("Prober's assessment"));
    assert_labelled(&teacher.expect("the Teacher's request carries the hand-off"));

    let jobs = harness
        .send_json(json!({"cmd": "noesis_jobs"}))
        .await
        .expect("jobs");
    let Reply::NoesisJobs { jobs } = jobs else {
        panic!("jobs")
    };
    assert!(
        jobs.iter().any(|job| job.target == created.id
            && job.kind == aether_core::noesis::JobKind::ExtractEngineering),
        "{jobs:?}"
    );
    harness.settled().await;
    harness.close().await;
    let accepted = archond.accept();
    assert!(
        matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connected to the archond stand-in: {accepted:?}"
    );
}
