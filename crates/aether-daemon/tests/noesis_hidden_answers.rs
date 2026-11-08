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
use std::collections::BTreeSet;

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
fn generic_reads(agent: &str, run: &str) -> Vec<Value> {
    vec![
        json!({"cmd": "get_transcript_page", "agent": agent}),
        json!({"cmd": "get_transcript", "agent": agent, "after": 0}),
        json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {"agent": agent}}),
        json!({"cmd": "get_agent", "id": agent}),
        json!({"cmd": "list_invocations", "agent": agent}),
        json!({"cmd": "get_inbox", "agent": agent, "after": 0}),
        json!({"cmd": "get_run_report", "run": run}),
    ]
}

/// The reads of the whole daemon, not of one agent.
fn daemon_reads(session: &str) -> Vec<Value> {
    vec![
        json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {}}),
        json!({"cmd": "list_agents", "query": {}}),
        json!({"cmd": "list_agents_page", "query": {}}),
        json!({"cmd": "get_tree", "include_archived": true}),
        json!({"cmd": "list_runs", "limit": 100}),
        json!({"cmd": "noesis_get_session", "session": session}),
        json!({"cmd": "noesis_list_sessions"}),
    ]
}

/// Every string held under `key` anywhere in `value`.
fn ids_named(value: &Value, key: &str, into: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (name, field) in map {
                if name == key
                    && let Some(id) = field.as_str()
                {
                    into.insert(id.to_owned());
                }
                ids_named(field, key, into);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| ids_named(item, key, into)),
        _ => {}
    }
}

/// The snapshot and invocation reads for every id `agent`'s events and
/// invocation list name; none is left unread.
async fn record_reads(home: &mut Home, agent: &str) -> Vec<Value> {
    let events = home
        .ok(json!({"cmd": "get_events", "after": 0, "limit": 10000, "query": {"agent": agent}}))
        .await;
    let listed = home
        .ok(json!({"cmd": "list_invocations", "agent": agent}))
        .await;
    assert!(
        listed.is_array(),
        "list_invocations answers a list: {listed}"
    );
    let (mut snapshots, mut invocations) = (BTreeSet::new(), BTreeSet::new());
    ids_named(&events, "snapshot", &mut snapshots);
    ids_named(&events, "invocation", &mut invocations);
    for invocation in listed.as_array().into_iter().flatten() {
        invocations.insert(
            invocation["id"]
                .as_str()
                .expect("an invocation id")
                .to_owned(),
        );
    }
    let snapshot_reads = snapshots
        .iter()
        .map(|id| json!({"cmd": "get_snapshot", "id": id}));
    let invocation_reads = invocations
        .iter()
        .map(|id| json!({"cmd": "get_invocation", "id": id}));
    snapshot_reads.chain(invocation_reads).collect()
}

async fn run_of(home: &mut Home, agent: &str) -> String {
    let detail = home.ok(json!({"cmd": "get_agent", "id": agent})).await;
    let run = detail["run"]
        .as_str()
        .unwrap_or_else(|| panic!("the agent's run: {detail}"));
    assert!(run.starts_with("run_"), "a run id: {run}");
    run.to_owned()
}

/// Every read of the Noesis agent, the control agent and the daemon.
async fn all_reads(home: &mut Home, session: &Value, control: &str) -> Vec<Value> {
    let noesis = session["agent"].as_str().expect("the session's agent");
    let id = session["id"].as_str().expect("a session id");
    let mut reads = daemon_reads(id);
    for agent in [noesis, control] {
        let run = run_of(home, agent).await;
        reads.extend(generic_reads(agent, &run));
        let records = record_reads(home, agent).await;
        let reads_both = ["get_snapshot", "get_invocation"]
            .iter()
            .all(|cmd| records.iter().any(|read| read["cmd"] == *cmd));
        assert!(
            agent != control || reads_both,
            "the control's snapshot and invocation reads run: {records:?}"
        );
        reads.extend(records);
    }
    reads
}

/// What both readers get back from every read, plus every frame their
/// subscriptions pushed meanwhile, as one text per reader.
async fn read_everything(
    home: &mut Home,
    readers: &mut Readers,
    session: &Value,
    control: &str,
) -> [String; 2] {
    let mut texts = [String::new(), String::new()];
    for frame in all_reads(home, session, control).await {
        let local = home.host.conn.call(frame.clone()).await;
        assert!(
            answer_of(&local).is_ok(),
            "the operator reads {frame}: {local}"
        );
        texts[0].push_str(&local.to_string());
        let remote = readers.observer.call(frame.clone()).await;
        assert!(
            remote["frame"] == "reply",
            "the observer is answered {frame}: {remote}"
        );
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

/// Subscribed frames arrive after they are committed or streamed; ask until
/// both readers' subscriptions have pushed a frame `wanted` accepts.
async fn await_pushed(
    home: &mut Home,
    readers: &mut Readers,
    what: &str,
    wanted: impl Fn(&Value) -> bool,
) {
    for _ in 0..400 {
        let local = home.host.conn.pushed.iter().any(&wanted);
        let remote = readers.observer.pushed.iter().any(&wanted);
        if local && remote {
            return;
        }
        home.host.conn.call(json!({"cmd": "ping"})).await;
        readers.observer.call(json!({"cmd": "ping"})).await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the subscriptions never pushed {what}");
}

/// Both live subscriptions carry `agent`'s streamed deltas.
async fn await_delta(home: &mut Home, readers: &mut Readers, agent: &str) {
    let streamed = |frame: &Value| {
        frame["frame"] == "delta" && frame["agent"] == agent && frame["delta"]["delta"] == "text"
    };
    await_pushed(home, readers, "a text delta", streamed).await;
}

/// Create an ordinary agent whose records exercise the generic read paths.
async fn create_control(home: &mut Home, name: &str) -> String {
    let created = home
        .ok(
            json!({"cmd": "create_agent", "role": "worker", "name": name, "objective": "Say hello.",
            "model": {"provider": "mock", "model": "mock-1"}, "start": true}),
        )
        .await;
    let agent = created["id"].as_str().expect("an agent id");
    assert!(agent.starts_with("agt_"), "an agent id: {created}");
    agent.to_owned()
}

/// Wait until the control has streamed and can no longer consume scripted replies.
async fn await_control(home: &mut Home, readers: &mut Readers, agent: &str, asked: usize) {
    assert!(agent.starts_with("agt_"), "an agent id: {agent}");
    home.requests_reach(asked + 1).await;
    await_delta(home, readers, agent).await;
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let detail = home.ok(json!({"cmd": "get_agent", "id": agent})).await;
            let state = detail["status"]["state"].as_str().expect("an agent state");
            match state {
                "completed" => return,
                "created" | "ready" | "running" | "waiting" => {}
                _ => panic!("the control did not complete: {detail}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the control completes in time");
}

/// An ordinary agent whose streamed turns prove both subscriptions carry live
/// deltas, and whose records prove the snapshot and invocation reads run.
async fn start_control(home: &mut Home, readers: &mut Readers, name: &str) -> String {
    let asked = home.model.requests().len();
    let agent = create_control(home, name).await;
    await_control(home, readers, &agent, asked).await;
    agent
}

#[tokio::test(flavor = "multi_thread")]
async fn control_waits_for_the_second_model_response() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut home = Home::start("").await;
        let mut readers = Readers::pair(&mut home).await;
        subscribe(&mut home, &mut readers, None).await;
        home.model.text("First reply.");
        let gate = home
            .model
            .hold(noesis_support::faux::Reply::Text("Second reply.".into()));
        let asked = home.model.requests().len();
        let agent = create_control(&mut home, "held-control").await;
        home.requests_reach(asked + 2).await;
        await_delta(&mut home, &mut readers, &agent).await;
        {
            let wait = await_control(&mut home, &mut readers, &agent, asked);
            tokio::pin!(wait);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(200), &mut wait)
                    .await
                    .is_err(),
                "the control cannot finish while its second model response is held"
            );
            gate.notify_one();
            wait.await;
        }
        let detail = home.ok(json!({"cmd": "get_agent", "id": agent})).await;
        assert_eq!(detail["status"]["state"], "completed", "{detail}");
    })
    .await
    .expect("the held control test finishes in time");
}

/// A new streamed turn after a reconnect, from a second control agent.
async fn stream_again(home: &mut Home, readers: &mut Readers) {
    start_control(home, readers, "control-again").await;
}

/// Read everything fresh, then again over new connections that replay the
/// whole history and follow live, after `live` made new activity.
async fn check_every_reader(
    home: &mut Home,
    readers: &mut Readers,
    case: &Case<'_>,
    live: impl AsyncFnOnce(&mut Home),
) {
    let marker = |frame: &Value| frame.to_string().contains(case.marker);
    await_pushed(home, readers, case.marker, marker).await;
    let fresh = read_everything(home, readers, case.session, case.control).await;
    assert_hidden(&fresh, case.secrets, case.tool, "on a fresh subscription");
    readers.reconnect(home).await;
    subscribe(home, readers, Some(0)).await;
    await_pushed(home, readers, case.marker, marker).await;
    live(home).await;
    stream_again(home, readers).await;
    let again = read_everything(home, readers, case.session, case.control).await;
    assert_hidden(&again, case.secrets, case.tool, "after a reconnect");
}

/// One sealed case: whose secrets, posed by which call, announced by which event.
struct Case<'a> {
    session: &'a Value,
    control: &'a str,
    secrets: &'a [&'a str],
    tool: &'a str,
    marker: &'a str,
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
    let control = start_control(&mut home, &mut readers, "control").await;
    let question = json!({"kind": "recall", "prompt": "What was the Roman name of the capital of France?",
        "answer_key": [key], "explanation": explanation});
    home.model.tool("A first question.", "noesis_ask", question);

    let created = home.create_session("teach", "Learn French history").await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let posed = home
        .session_until(&id, "a question", |s| !s["open_question"].is_null())
        .await;
    let secrets = [key.as_str(), explanation.as_str()];
    let case = Case {
        session: &posed,
        control: &control,
        secrets: &secrets,
        tool: "noesis",
        marker: "noesis.question_posed",
    };
    check_every_reader(&mut home, &mut readers, &case, async |_: &mut Home| {}).await;

    let question_id = posed["open_question"]["id"].clone();
    let answer = json!({"cmd": "noesis_answer", "session": id, "question": question_id,
        "answer": {"text": "Lutetia", "dont_know": false}, "idempotency_key": "answer-1"});
    home.model.tool(
        "",
        "noesis_judge_recall",
        json!({"question": question_id, "correct": true}),
    );
    home.model.text("Noted.");
    let asked = home.model.requests().len();
    home.ok(answer).await;
    home.requests_reach(asked + 2).await;
    let recorded = noesis_events(&mut home, "noesis.answer_recorded").await;
    assert_eq!(recorded.len(), 1, "one answer recorded: {recorded:?}");
    let revealed = recorded[0].to_string();
    assert!(
        revealed.contains(&key) && revealed.contains(&explanation),
        "revealed once answered: {revealed}"
    );
    let history = home.session(&id).await;
    let result = &history["questions"][0]["result"];
    assert_eq!(
        (&result["answer_key"], &result["explanation"]),
        (&json!(key), &json!(explanation)),
        "the session's history reveals the answered question: {history}"
    );
    let transcript = home
        .ok(json!({"cmd": "get_transcript_page", "agent": posed["agent"]}))
        .await;
    let transcript = transcript.to_string();
    assert!(transcript.contains(&key), "the answered key is delivered");
    assert!(
        !transcript.contains(&explanation),
        "the explanation is not delivered to the teacher"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn h2_an_active_problem_s_solution_is_hidden_from_every_reader_until_conclude() {
    let solution = format!("Solved-by-{}", ulid::Ulid::generate());
    let mut home = Home::start("").await;
    let mut readers = Readers::pair(&mut home).await;
    home.enable().await;
    subscribe(&mut home, &mut readers, None).await;
    let control = start_control(&mut home, &mut readers, "control").await;
    let problem = json!({"statement": "Differentiate x to the fifth power.", "mode": "recall",
        "targets": [], "solution": solution});
    home.model.tool("Try this one.", "noesis_problem", problem);
    home.model.text("Over to you.");

    let asked = home.model.requests().len();
    let created = home
        .create_session("practice", "Practise derivatives")
        .await;
    let id = created["id"].as_str().expect("a session id").to_owned();
    let active = home
        .session_until(&id, "a problem", |s| !s["active_problem"].is_null())
        .await;
    home.requests_reach(asked + 2).await;
    let secrets = [solution.as_str()];
    let case = Case {
        session: &active,
        control: &control,
        secrets: &secrets,
        tool: "noesis",
        marker: "noesis.problem_posed",
    };
    let hint = async |home: &mut Home| {
        let asked = home.model.requests().len();
        home.model.text("Keep going: what does the power rule say?");
        let send = json!({"cmd": "noesis_send", "session": id, "text": "I am not sure.", "idempotency_key": "send-0"});
        home.ok(send).await;
        home.requests_reach(asked + 1).await;
        let still = home.session(&id).await;
        assert!(
            !still["active_problem"].is_null(),
            "the problem is still active: {still}"
        );
    };
    check_every_reader(&mut home, &mut readers, &case, hint).await;

    conclude_and_reveal(&mut home, &id, &active, &solution).await;
}

/// The learner solves the problem: the conclusion reveals the solution, and
/// the transcript stays sealed.
async fn conclude_and_reveal(home: &mut Home, id: &str, active: &Value, solution: &str) {
    let send = json!({"cmd": "noesis_send", "session": id, "text": "Is it five x to the fourth?", "idempotency_key": "send-1"});
    let conclude = json!({"outcome": "solved", "rung": 0, "retest": false});
    home.model.tool("Correct.", "noesis_conclude", conclude);
    home.model.text("Well done.");
    home.ok(send).await;
    home.session_until(id, "the conclusion", |s| s["active_problem"].is_null())
        .await;
    let concluded = noesis_events(home, "noesis.practice_concluded").await;
    assert_eq!(concluded.len(), 1, "one conclusion: {concluded:?}");
    assert_eq!(
        concluded[0]["kind"]["payload"]["solution"], solution,
        "revealed at conclude: {}",
        concluded[0]
    );
    let history = home.session(id).await;
    assert_eq!(
        history["problems"][0]["conclusion"]["solution"], solution,
        "the session's history reveals the concluded problem: {history}"
    );
    let transcript = home
        .ok(json!({"cmd": "get_transcript_page", "agent": active["agent"]}))
        .await;
    assert!(
        !transcript.to_string().contains(solution),
        "the transcript stays sealed after conclude"
    );
}
