#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! ARCHITECTURE.md §g, "answers stay hidden on every read path": with a
//! question open, or a problem active, neither the operator's local client
//! nor an observer device reads the answer key, explanation or solution on
//! any generic path, fresh or after a reconnect, nor does the daemon log;
//! only the Noesis reveal after the answer or `conclude` carries them.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use noesis_support::{Home, SEALED, answer_of, log_text};
use phone::{Phone, Socket};
use serde_json::{Value, json};

/// The two connections that read: the operator's own, and an observer's.
struct Readers {
    phone: Phone,
    device: String,
    observer: Socket,
}

impl Readers {
    async fn pair(home: &mut Home) -> Readers {
        home.host
            .configure(json!({"enabled": true, "pairing": true}))
            .await;
        let phone = Phone::new();
        let device = home.host.pair(&phone, "observer", "observer").await;
        let observer = home.host.connect(&phone, &device).await;
        Readers {
            phone,
            device,
            observer,
        }
    }

    /// Both sides drop their connection and come back with new ones.
    async fn reconnect(&mut self, home: &mut Home) {
        home.host.conn = support::RawConn::connect(&home.host.daemon.socket)
            .await
            .expect("reconnect");
        home.host
            .conn
            .hello(aether_protocol::PROTOCOL_VERSION)
            .await
            .expect("welcome");
        self.observer = home.host.connect(&self.phone, &self.device).await;
    }
}

/// Subscribe both readers, live with deltas, from `after` (`None`: from now).
async fn subscribe(home: &mut Home, readers: &mut Readers, after: Option<u64>) {
    let frame = json!({"cmd": "subscribe", "after": after, "deltas": true, "query": {}});
    let local = home.host.conn.call(frame.clone()).await;
    answer_of(&local).expect("the local client subscribes");
    let remote = readers.observer.call(frame).await;
    answer_of(&remote).expect("the observer subscribes");
}

/// Every generic read of `agent` and `run`, as `frame`s to send.
fn generic_reads(agent: &str, run: &str, session: &str) -> Vec<Value> {
    vec![
        json!({"cmd": "get_transcript_page", "agent": agent}),
        json!({"cmd": "get_transcript", "agent": agent, "after": 0}),
        json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {}}),
        json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {"agent": agent}}),
        json!({"cmd": "get_agent", "id": agent}),
        json!({"cmd": "list_agents", "query": {}}),
        json!({"cmd": "list_agents_page", "query": {}}),
        json!({"cmd": "get_tree", "include_archived": true}),
        json!({"cmd": "list_invocations", "agent": agent}),
        json!({"cmd": "get_inbox", "agent": agent, "after": 0}),
        json!({"cmd": "get_run_report", "run": run}),
        json!({"cmd": "list_runs", "limit": 100}),
        json!({"cmd": "noesis_get_session", "session": session}),
        json!({"cmd": "noesis_list_sessions"}),
    ]
}

/// What both readers get back from every generic read, plus every frame
/// their subscriptions pushed meanwhile, as one text per reader.
async fn read_everything(home: &mut Home, readers: &mut Readers, session: &Value) -> [String; 2] {
    let agent = session["agent"].as_str().expect("the session's agent");
    let id = session["id"].as_str().expect("a session id");
    let detail = home.ok(json!({"cmd": "get_agent", "id": agent})).await;
    let run = detail["agent"]["run"]
        .as_str()
        .or_else(|| detail["run"].as_str())
        .unwrap_or_else(|| panic!("the agent's run: {detail}"))
        .to_owned();
    let mut texts = [String::new(), String::new()];
    let mut reads = generic_reads(agent, &run, id);
    let invocations = home
        .ok(json!({"cmd": "list_invocations", "agent": agent}))
        .await;
    for invocation in invocations["invocations"].as_array().into_iter().flatten() {
        reads.push(json!({"cmd": "get_invocation", "id": invocation["id"]}));
    }
    for frame in reads {
        let local = home.host.conn.call(frame.clone()).await;
        assert!(
            answer_of(&local).is_ok(),
            "the operator reads {frame}: {local}"
        );
        texts[0].push_str(&local.to_string());
        let remote = readers.observer.call(frame.clone()).await;
        texts[1].push_str(&remote.to_string());
    }
    texts[0].push_str(&Value::from(std::mem::take(&mut home.host.conn.pushed)).to_string());
    texts[1].push_str(&Value::from(std::mem::take(&mut readers.observer.pushed)).to_string());
    texts
}

/// Neither reader, nor the daemon log, carries any of `secrets`; the
/// operator did read the sealed call, so the check is not vacuous.
fn assert_hidden(texts: &[String; 2], secrets: &[&str], sealed_tool: &str, when: &str) {
    for (reader, text) in ["the local client", "the observer"].iter().zip(texts) {
        for secret in secrets {
            assert!(!text.contains(secret), "{reader} read {secret:?} {when}");
        }
    }
    assert!(
        texts[0].contains(sealed_tool) && texts[0].contains(SEALED),
        "the operator's reads show the sealed {sealed_tool} call {when}"
    );
    let log = log_text();
    for secret in secrets {
        assert!(
            !log.contains(secret),
            "the daemon log holds {secret:?} {when}"
        );
    }
}

/// Subscribed frames arrive after the event is committed; ask until both
/// readers' subscriptions have pushed something carrying `marker`.
async fn await_pushed(home: &mut Home, readers: &mut Readers, marker: &str) {
    for _ in 0..400 {
        let local = home
            .host
            .conn
            .pushed
            .iter()
            .any(|f| f.to_string().contains(marker));
        let remote = readers
            .observer
            .pushed
            .iter()
            .any(|f| f.to_string().contains(marker));
        if local && remote {
            return;
        }
        home.host.conn.call(json!({"cmd": "ping"})).await;
        readers.observer.call(json!({"cmd": "ping"})).await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the subscriptions never pushed {marker}");
}

/// Read everything fresh, then again over new connections that replay the
/// whole history and follow live, and check each time.
async fn check_every_reader(
    home: &mut Home,
    readers: &mut Readers,
    session: &Value,
    secrets: &[&str],
    tool: &str,
    marker: &str,
) {
    await_pushed(home, readers, marker).await;
    let fresh = read_everything(home, readers, session).await;
    assert_hidden(&fresh, secrets, tool, "on a fresh subscription");
    readers.reconnect(home).await;
    subscribe(home, readers, Some(0)).await;
    await_pushed(home, readers, marker).await;
    let again = read_everything(home, readers, session).await;
    assert_hidden(&again, secrets, tool, "after a reconnect");
}

/// The Noesis events named `kind` (they travel as `custom`), as the operator reads them, once one exists.
async fn noesis_events(home: &mut Home, kind: &str) -> Vec<Value> {
    let mut seen = Value::Null;
    for _ in 0..400 {
        let page = home
            .ok(json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {}}))
            .await;
        let events = page.as_array().expect("an event page is a list");
        let found: Vec<Value> = events
            .iter()
            .filter(|event| event["kind"]["name"] == kind)
            .cloned()
            .collect();
        if !found.is_empty() {
            return found;
        }
        seen = page;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let shown: String = seen.to_string().chars().take(3000).collect();
    panic!("no {kind} event was recorded; the last page: {shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn h1_an_open_question_s_key_is_hidden_from_every_reader_until_answered() {
    let key = format!("Lutetia-{}", ulid::Ulid::generate());
    let explanation = format!("Explained-{}", ulid::Ulid::generate());
    let mut home = Home::start("").await;
    let mut readers = Readers::pair(&mut home).await;
    home.enable().await;
    subscribe(&mut home, &mut readers, None).await;
    let question = json!({"kind": "recall", "prompt": "What was the Roman name of the capital of France?",
        "answer_key": [key], "explanation": explanation});
    home.model.tool("A first question.", "noesis_ask", question);

    let created = home.create_session("teach", "Learn French history").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let posed = home
        .session_until(&id, "a question", |s| !s["open_question"].is_null())
        .await;
    let secrets = [key.as_str(), explanation.as_str()];
    check_every_reader(
        &mut home,
        &mut readers,
        &posed,
        &secrets,
        "noesis",
        "noesis.question_posed",
    )
    .await;

    let question_id = posed["open_question"]["id"].clone();
    let answer = json!({"cmd": "noesis_answer", "session": id, "question": question_id,
        "answer": {"text": "Lutetia", "dont_know": false}, "idempotency_key": "answer-1"});
    home.ok(answer).await;
    let recorded = noesis_events(&mut home, "noesis.answer_recorded").await;
    assert_eq!(recorded.len(), 1, "one answer recorded: {recorded:?}");
    let revealed = recorded[0].to_string();
    assert!(
        revealed.contains(&key) && revealed.contains(&explanation),
        "revealed once answered: {revealed}"
    );
    let transcript = home
        .ok(json!({"cmd": "get_transcript_page", "agent": posed["agent"]}))
        .await;
    assert!(
        !transcript.to_string().contains(&key),
        "the transcript stays sealed after the answer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn h2_an_active_problem_s_solution_is_hidden_from_every_reader_until_conclude() {
    let solution = format!("Solved-by-{}", ulid::Ulid::generate());
    let mut home = Home::start("").await;
    let mut readers = Readers::pair(&mut home).await;
    home.enable().await;
    subscribe(&mut home, &mut readers, None).await;
    let problem = json!({"statement": "Differentiate x to the fifth power.", "mode": "recall",
        "targets": [], "solution": solution});
    home.model.tool("Try this one.", "noesis_problem", problem);
    home.model.text("Over to you.");

    let created = home
        .create_session("practice", "Practise derivatives")
        .await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let active = home
        .session_until(&id, "a problem", |s| !s["active_problem"].is_null())
        .await;
    home.requests_reach(2).await;
    let secrets = [solution.as_str()];
    check_every_reader(
        &mut home,
        &mut readers,
        &active,
        &secrets,
        "noesis",
        "noesis.problem_posed",
    )
    .await;

    let send = json!({"cmd": "noesis_send", "session": id, "text": "Is it five x to the fourth?", "idempotency_key": "send-1"});
    let conclude = json!({"outcome": "solved", "rung": 0, "retest": false});
    home.model.tool("Correct.", "noesis_conclude", conclude);
    home.model.text("Well done.");
    home.ok(send).await;
    home.session_until(&id, "the conclusion", |s| s["active_problem"].is_null())
        .await;
    let concluded = noesis_events(&mut home, "noesis.practice_concluded").await;
    assert_eq!(concluded.len(), 1, "one conclusion: {concluded:?}");
    assert_eq!(
        concluded[0]["kind"]["payload"]["solution"],
        solution.as_str(),
        "revealed at conclude: {}",
        concluded[0]
    );
    let transcript = home
        .ok(json!({"cmd": "get_transcript_page", "agent": active["agent"]}))
        .await;
    assert!(
        !transcript.to_string().contains(&solution),
        "the transcript stays sealed after conclude"
    );
}
