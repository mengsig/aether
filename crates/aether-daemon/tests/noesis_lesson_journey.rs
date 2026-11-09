#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A learner's journey through Noesis over a real daemon and the faux model:
//! the Prober hands off to the Teacher, answers are graded and revealed only
//! once recorded, the lesson's history survives a restart, and an extraction
//! job's end and merge reach a web-style subscriber typed.

mod noesis_support;
#[path = "remote/phone.rs"]
mod phone;
mod support;

use std::time::{Duration, Instant};

use noesis_support::{Home, answer_of};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(25);
const GOAL: &str = "Learn Roman Gaul";
const CONFIDENCE: u8 = 70;
const THINKING: &str = "I remembered the Latin names from school.";
const CITIES: [&str; 3] = ["Lutetia", "Londinium", "Eboracum"];
const RIVERS: [&str; 3] = ["Seine", "Loire", "Rhone"];
/// The Prober's reply in the extraction journey: the only prose it reads.
const PROSE: &str = "A limit is the value a function approaches as its input nears a point.";

/// The features `web/packages/core/src/connection.ts` says hello with.
const WEB_FEATURES: &[&str] = &[
    "deltas",
    "artifact_bytes",
    "delta_order",
    "memory_v2",
    "noesis",
];

/// One posed choice question: its id, the shown value of its key, the label
/// the learner chose and its sealed explanation.
struct Asked {
    id: String,
    key: String,
    chosen: String,
    explanation: String,
}

/// A lesson handed to the Teacher with two questions answered, the session
/// as read while each was open, and what the learner then wrote.
struct Lesson {
    id: String,
    first_open: Value,
    second_open: Value,
    asked: [Asked; 2],
    learner_text: String,
}

/// The probe's hand-off: one strand assessed, so the Teacher takes over.
fn probe_done() -> Value {
    json!({"strands": [{"strand": "roman gaul", "floor": "knows Paris is old",
        "ceiling": "latin place names", "misconceptions": [], "start": "roman names"}],
        "summary": "Ready for Roman place names."})
}

/// A single-answer choice question whose key is the option at `right`.
fn choice(prompt: &str, labels: [&str; 3], right: usize, explanation: &str) -> Value {
    assert!(right < labels.len(), "the key is one of the options");
    assert!(!explanation.is_empty(), "a sealed explanation");
    let options: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| json!({"value": format!("opt{}", index + 1), "label": label}))
        .collect();
    json!({"kind": "choice", "prompt": prompt, "options": options,
        "answer_key": [format!("opt{}", right + 1)], "explanation": explanation})
}

/// The whole scripted lesson, in the order the model is asked: the Prober's
/// hand-off, the Teacher's two questions, and its replies after them.
fn script_lesson(home: &Home) -> [String; 2] {
    let explanations = [
        format!("Explained-{}", ulid::Ulid::generate()),
        format!("Explained-{}", ulid::Ulid::generate()),
    ];
    let first = choice(
        "What did the Romans call Paris?",
        CITIES,
        0,
        &explanations[0],
    );
    let second = choice(
        "Which river runs through Paris?",
        RIVERS,
        0,
        &explanations[1],
    );
    home.model
        .tool("I have what I need.", "noesis_probe_done", probe_done());
    home.model.tool("A first question.", "noesis_ask", first);
    home.model.tool("Another one.", "noesis_ask", second);
    home.model.text("Not quite.");
    home.model.text("Thanks for sharing that.");
    explanations
}

/// The shown (re-keyed) value of the open question's option labelled `label`.
fn shown_value(session: &Value, label: &str) -> String {
    let options = session["open_question"]["options"]
        .as_array()
        .unwrap_or_else(|| panic!("an open choice question: {session}"));
    let option = options
        .iter()
        .find(|option| option["label"] == label)
        .unwrap_or_else(|| panic!("an option labelled {label}: {session}"));
    option["value"]
        .as_str()
        .expect("an option value")
        .to_owned()
}

/// The session once a question other than `previous` is open.
async fn open_question_after(home: &mut Home, id: &str, previous: &Value) -> Value {
    let posed = home
        .session_until(id, "a new open question", |s| {
            !s["open_question"].is_null() && &s["open_question"]["id"] != previous
        })
        .await;
    assert!(
        posed["open_question"]["id"].is_string(),
        "an open question has an id: {posed}"
    );
    posed
}

/// Answer `open`'s question with the option labelled `chosen`; its key is
/// the option labelled `right`.
async fn answer_open(
    home: &mut Home,
    id: &str,
    open: &Value,
    (right, chosen): (&str, &str),
    explanation: &str,
    key: &str,
) -> Asked {
    let question = open["open_question"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("an open question: {open}"))
        .to_owned();
    let key_value = shown_value(open, right);
    let chosen_value = shown_value(open, chosen);
    let frame = json!({"cmd": "noesis_answer", "session": id, "question": question,
        "answer": {"choices": [chosen_value], "dont_know": false,
            "confidence": CONFIDENCE, "thinking": THINKING},
        "idempotency_key": key});
    home.ok(frame).await;
    Asked {
        id: question,
        key: key_value,
        chosen: chosen.to_owned(),
        explanation: explanation.to_owned(),
    }
}

/// Run the scripted lesson: hand-off, one right and one wrong answer, then a
/// message from the learner, each turn finished before this returns.
async fn graded_lesson(home: &mut Home) -> Lesson {
    let asked = home.model.requests().len();
    let explanations = script_lesson(home);
    let created = home.create_session("teach", GOAL).await;
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a session id: {created}"))
        .to_owned();
    let first_open = open_question_after(home, &id, &Value::Null).await;
    let first = answer_open(
        home,
        &id,
        &first_open,
        ("Lutetia", "Lutetia"),
        &explanations[0],
        "answer-1",
    )
    .await;
    let second_open = open_question_after(home, &id, &first_open["open_question"]["id"]).await;
    let second = answer_open(
        home,
        &id,
        &second_open,
        ("Seine", "Loire"),
        &explanations[1],
        "answer-2",
    )
    .await;
    home.session_until(&id, "the second answer's result", |s| {
        s["open_question"].is_null() && !s["questions"][1]["result"].is_null()
    })
    .await;
    home.requests_reach(asked + 4).await;
    let learner_text = format!("Roman roads met there, I think {}", ulid::Ulid::generate());
    let send = json!({"cmd": "noesis_send", "session": id, "text": learner_text, "idempotency_key": "send-1"});
    home.ok(send).await;
    home.requests_reach(asked + 5).await;
    // A Noesis read waits for the turn in flight, so the lesson is settled.
    let settled = home.session(&id).await;
    assert!(settled["open_question"].is_null(), "{settled}");
    Lesson {
        id,
        first_open,
        second_open,
        asked: [first, second],
        learner_text,
    }
}

/// While `asked` is open its answer, result, key and explanation stay off
/// the wire; `history` questions came before it.
fn assert_open_and_sealed(session: &Value, asked: &Asked, history: usize) {
    let questions = session["questions"]
        .as_array()
        .unwrap_or_else(|| panic!("the session's questions: {session}"));
    assert_eq!(questions.len(), history + 1, "{session}");
    let open = &questions[history];
    assert_eq!(open["question"]["id"], asked.id.as_str(), "{session}");
    assert!(
        open["answer"].is_null() && open["result"].is_null(),
        "an open question has no answer or result: {open}"
    );
    assert_eq!(session["open_question"]["id"], asked.id.as_str());
    assert!(
        !session.to_string().contains(&asked.explanation),
        "the explanation is on the wire before the answer: {session}"
    );
}

/// `recorded` holds the learner's answer as given and the revealed result.
fn assert_graded(recorded: &Value, asked: &Asked, correct: bool) {
    assert_eq!(recorded["question"]["id"], asked.id.as_str(), "{recorded}");
    let answer = &recorded["answer"];
    assert_eq!(
        answer["chosen"],
        json!([asked.chosen]),
        "the chosen label: {answer}"
    );
    assert_eq!(answer["confidence"], CONFIDENCE, "{answer}");
    assert_eq!(answer["thinking"], THINKING, "{answer}");
    assert_eq!(answer["dont_know"], false, "{answer}");
    let result = &recorded["result"];
    assert_eq!(result["question"], asked.id.as_str(), "{result}");
    assert_eq!(result["correct"], correct, "graded: {result}");
    assert_eq!(
        result["answer_key"],
        json!([asked.key]),
        "the key revealed: {result}"
    );
    assert_eq!(
        result["explanation"],
        asked.explanation.as_str(),
        "{result}"
    );
}

/// Journey 1 (grading): the Teacher's questions are answered over the
/// protocol; before an answer no key or explanation is on the wire, after it
/// `noesis_get_session` holds the answer, the grade and the revealed key.
#[tokio::test(flavor = "multi_thread")]
async fn j1_answers_are_graded_and_revealed_only_once_recorded() {
    let mut home = Home::start("").await;
    home.enable().await;
    let lesson = graded_lesson(&mut home).await;
    let first_text = lesson.first_open.to_string();
    assert!(
        !first_text.contains("answer_key"),
        "no answer key is on the wire before the first answer: {first_text}"
    );
    assert_open_and_sealed(&lesson.first_open, &lesson.asked[0], 0);
    assert_open_and_sealed(&lesson.second_open, &lesson.asked[1], 1);
    assert_graded(&lesson.second_open["questions"][0], &lesson.asked[0], true);

    let session = home.session(&lesson.id).await;
    let questions = session["questions"]
        .as_array()
        .unwrap_or_else(|| panic!("the session's questions: {session}"));
    assert_eq!(
        questions.len(),
        2,
        "both questions are in the history: {session}"
    );
    assert_graded(&questions[0], &lesson.asked[0], true);
    assert_graded(&questions[1], &lesson.asked[1], false);
}

/// The session read of a restarted daemon, once Noesis serves it again.
async fn session_once_served(home: &mut Home, id: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let frame = json!({"cmd": "noesis_get_session", "session": id});
        let answer = home.host.conn.call(frame).await;
        let error = match answer_of(&answer) {
            Ok(session) => return session,
            Err(error) => error,
        };
        assert_eq!(
            error["code"], "unavailable",
            "the read was refused: {answer}"
        );
        assert!(
            Instant::now() < deadline,
            "Noesis never served the session after the restart: {answer}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Every entry of `agent`'s transcript, oldest first, from one page.
async fn transcript(home: &mut Home, agent: &str) -> Vec<Value> {
    let frame = json!({"cmd": "get_transcript_page", "agent": agent, "limit": 1000});
    let page = home.ok(frame).await;
    assert_eq!(
        page["has_before"], false,
        "one page holds the lesson: {page}"
    );
    let entries = page["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("a transcript page: {page}"));
    assert!(!entries.is_empty(), "{agent} has a transcript: {page}");
    entries.clone()
}

/// The text blocks of a transcript entry, joined.
fn entry_text(entry: &Value) -> String {
    let blocks = entry["message"]["content"].as_array();
    let blocks = blocks.unwrap_or_else(|| panic!("an entry's content: {entry}"));
    let texts: Vec<&str> = blocks
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    texts.join("\n")
}

/// `agent`'s internal seqs name the user-role inputs Noesis wrote (its
/// opening or hand-off), never the learner's text; returns its transcript.
async fn assert_internal_inputs(home: &mut Home, agent: &Value, learner_text: &str) -> Vec<Value> {
    let id = agent["agent"]
        .as_str()
        .unwrap_or_else(|| panic!("a session agent's id: {agent}"));
    let internal: Vec<u64> = agent["internal"]
        .as_array()
        .unwrap_or_else(|| panic!("internal seqs: {agent}"))
        .iter()
        .map(|seq| seq.as_u64().expect("a seq"))
        .collect();
    assert!(!internal.is_empty(), "Noesis wrote an input for {agent}");
    let entries = transcript(home, id).await;
    for seq in &internal {
        let entry = entries.iter().find(|entry| entry["seq"] == *seq);
        let entry =
            entry.unwrap_or_else(|| panic!("seq {seq} is in {id}'s transcript: {entries:?}"));
        assert_eq!(entry["message"]["role"], "user", "{entry}");
        let text = entry_text(entry);
        assert!(text.contains(&format!("Learning goal: {GOAL}")), "{entry}");
        assert!(
            !text.contains(learner_text),
            "the learner's text is internal: {entry}"
        );
    }
    if agent["role"] == "teacher" {
        let handoff = internal
            .iter()
            .filter_map(|seq| entries.iter().find(|e| e["seq"] == *seq));
        let handoffs: Vec<String> = handoff.map(entry_text).collect();
        assert!(
            handoffs
                .iter()
                .any(|text| text.contains("Prober's assessment")),
            "the Teacher's internal input is the hand-off: {handoffs:?}"
        );
    }
    entries
}

/// Journey 2 (transcript survival): after a restart over the same state the
/// session still lists the Prober then the Teacher with their internal seqs,
/// the same graded questions, and each agent's transcript reads.
#[tokio::test(flavor = "multi_thread")]
async fn j2_the_lesson_s_agents_questions_and_transcripts_survive_a_restart() {
    let mut home = Home::start("").await;
    home.enable().await;
    let lesson = graded_lesson(&mut home).await;
    let before = home.session(&lesson.id).await;
    assert_eq!(
        lesson.first_open["phase"], "plan",
        "handed off before the first question"
    );
    let mut home = home.restart().await;
    let after = session_once_served(&mut home, &lesson.id).await;
    assert_eq!(
        after["questions"], before["questions"],
        "the questions survive: {after}"
    );
    assert_eq!(
        after["agents"], before["agents"],
        "the agents survive: {after}"
    );
    assert_graded(&after["questions"][0], &lesson.asked[0], true);
    assert_graded(&after["questions"][1], &lesson.asked[1], false);

    let agents = after["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("the session's agents: {after}"));
    let roles: Vec<&Value> = agents.iter().map(|agent| &agent["role"]).collect();
    assert_eq!(
        roles,
        [&json!("prober"), &json!("teacher")],
        "in the order they ran: {after}"
    );
    assert_eq!(
        agents[1]["agent"], after["agent"],
        "the Teacher runs the session now"
    );
    assert_ne!(
        agents[0]["agent"], agents[1]["agent"],
        "the hand-off made a new agent"
    );
    assert_internal_inputs(&mut home, &agents[0], &lesson.learner_text).await;
    let teacher = assert_internal_inputs(&mut home, &agents[1], &lesson.learner_text).await;
    let internal = agents[1]["internal"].as_array().expect("internal seqs");
    let said = teacher
        .iter()
        .find(|entry| entry_text(entry).contains(&lesson.learner_text));
    let said = said.unwrap_or_else(|| panic!("the learner's message survives: {teacher:?}"));
    assert_eq!(said["message"]["role"], "user", "{said}");
    assert!(
        !internal.contains(&said["seq"]),
        "the learner's own message is not internal: {said}"
    );
}

/// A connection that says hello with `features` and subscribes as the web
/// client does: from the last seq it knows, every event, no deltas.
async fn subscribed(home: &mut Home, features: &[&str]) -> support::RawConn {
    let after = home.last_seq().await;
    let mut conn = support::RawConn::connect(&home.host.daemon.socket)
        .await
        .expect("connect");
    let mut hello = support::hello_frame(aether_protocol::PROTOCOL_VERSION, None);
    hello["features"] = json!(features);
    support::read_answer(conn.call(hello).await).expect("welcome");
    let subscribe = json!({"cmd": "subscribe", "after": after, "query": {}, "deltas": false});
    answer_of(&conn.call(subscribe).await).expect("subscribed");
    conn
}

/// The kinds of the events `conn`'s subscription has pushed.
fn pushed_kinds(conn: &support::RawConn) -> Vec<Value> {
    conn.pushed
        .iter()
        .filter(|frame| frame["frame"] == "event")
        .map(|frame| frame["event"]["kind"].clone())
        .collect()
}

/// Ask until `conn`'s subscription has pushed an event whose kind `wanted` accepts.
async fn await_kind(
    conn: &mut support::RawConn,
    what: &str,
    wanted: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = pushed_kinds(conn).into_iter().find(|kind| wanted(kind)) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "the subscription never pushed {what}: {:?}",
            pushed_kinds(conn)
        );
        conn.call(json!({"cmd": "ping"})).await;
        tokio::time::sleep(POLL).await;
    }
}

/// A teach session whose Prober wrote [`PROSE`]; its id and agent.
async fn session_to_extract(home: &mut Home) -> (String, String) {
    let asked = home.model.requests().len();
    home.model.text(PROSE);
    let created = home.create_session("teach", "Learn limits").await;
    let (Some(id), Some(agent)) = (created["id"].as_str(), created["agent"].as_str()) else {
        panic!("the created session names its id and agent: {created}");
    };
    assert!(
        id.starts_with("ns_") && agent.starts_with("agt_"),
        "{created}"
    );
    let session = (id.to_owned(), agent.to_owned());
    home.requests_reach(asked + 1).await;
    session
}

/// The extractor's message ref for the Prober's [`PROSE`] reply: `agent:seq`.
async fn prose_anchor(home: &mut Home, agent: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let entries = transcript(home, agent).await;
        let prose = entries.iter().find(|entry| {
            entry["message"]["role"] == "assistant" && entry_text(entry).contains(PROSE)
        });
        if let Some(entry) = prose {
            let seq = entry["seq"].as_u64();
            let seq = seq.unwrap_or_else(|| panic!("an entry's seq: {entry}"));
            return format!("{agent}:{seq}");
        }
        assert!(
            Instant::now() < deadline,
            "the Prober's reply never reached its transcript: {entries:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The extractor agent's `complete_task` call: one proposition anchored on
/// the Prober's reply, as the extraction prompt asks for.
fn extraction_reply(name: &str, anchor: &str) -> Value {
    assert!(anchor.contains(':'), "an anchor is agent:seq: {anchor}");
    let proposition = json!({"key": "limit", "name": name, "statement": PROSE,
        "aliases": [], "anchor": anchor});
    json!({"summary": "Extracted one proposition.",
        "structured": {"propositions": [proposition], "chains": []}})
}

fn is_ended(state: &Value) -> bool {
    matches!(state.as_str(), Some("succeeded" | "failed" | "cancelled"))
}

/// No `noesis.*` event reached a `noesis`-feature subscriber disguised as `custom`.
fn assert_none_disguised(conn: &support::RawConn) {
    let kinds = pushed_kinds(conn);
    assert!(!kinds.is_empty(), "the subscription pushed events");
    let disguised: Vec<&Value> = kinds
        .iter()
        .filter(|kind| kind["type"] == "custom")
        .filter(|kind| {
            kind["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("noesis."))
        })
        .collect();
    assert!(
        disguised.is_empty(),
        "typed for a noesis client: {disguised:?}"
    );
}

/// Journey 3 (graph refresh): an extraction job on the faux model ends; a
/// web-style `noesis` subscriber receives the ended `noesis.job_updated` and
/// the `noesis.graph_merged` typed, and the graph then holds the proposition.
#[tokio::test(flavor = "multi_thread")]
async fn j3_an_ended_extraction_reaches_a_noesis_subscriber_typed_and_the_graph_shows_it() {
    let mut home = Home::start("").await;
    home.enable().await;
    let (id, agent) = session_to_extract(&mut home).await;
    let anchor = prose_anchor(&mut home, &agent).await;
    let name = format!("Limit-{}", ulid::Ulid::generate());
    home.model
        .tool("", "complete_task", extraction_reply(&name, &anchor));
    let mut web = subscribed(&mut home, WEB_FEATURES).await;
    let extract = json!({"cmd": "noesis_extract", "session": id, "idempotency_key": "extract-1"});
    let started = home.ok(extract).await;
    let job = started["job"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a queued job: {started}"))
        .to_owned();

    let ended = await_kind(&mut web, "the job's end", |kind| {
        kind["type"] == "noesis.job_updated"
            && kind["job"]["id"] == job.as_str()
            && is_ended(&kind["job"]["state"])
    })
    .await;
    assert_eq!(
        ended["job"]["state"], "succeeded",
        "the job ended well: {ended}"
    );
    assert_eq!(ended["job"]["stats"]["nodes"], 1, "{ended}");
    let merged = await_kind(&mut web, "the merge", |kind| {
        kind["type"] == "noesis.graph_merged" && kind["job"] == job.as_str()
    })
    .await;
    assert_eq!(
        merged["stats"]["nodes"], 1,
        "one proposition merged: {merged}"
    );
    assert_none_disguised(&web);

    let graph = home.ok(json!({"cmd": "noesis_graph"})).await;
    let nodes = graph["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("the graph's nodes: {graph}"));
    let node = nodes.iter().find(|node| node["name"] == name.as_str());
    let node = node.unwrap_or_else(|| panic!("the graph holds {name}: {graph}"));
    assert_eq!(node["statement"], PROSE, "{node}");
    assert_eq!(
        node["provenance"][0]["source"],
        id.as_str(),
        "from the session: {node}"
    );
    assert_eq!(
        node["provenance"][0]["message_ref"],
        anchor.as_str(),
        "{node}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recall_judgment_survives_restart_and_duplicate_calls() {
    let mut home = Home::start("").await;
    home.enable().await;
    home.model
        .tool("Ready to teach.", "noesis_probe_done", probe_done());
    home.model.tool(
        "A question.",
        "noesis_ask",
        json!({
            "kind": "recall", "intent": "predict", "prompt": "What is the capital of France?",
            "answer_key": "Paris", "explanation": "Paris is the capital.",
            "concepts": ["Learn geography"]
        }),
    );
    let created = home.create_session("teach", "Learn geography").await;
    let id = created["id"].as_str().unwrap().to_owned();
    let open = home
        .session_until(&id, "recall question", |s| {
            s["open_question"]["id"].is_string()
        })
        .await;
    assert!(!open.to_string().contains("Paris"));
    let question = open["open_question"]["id"].as_str().unwrap().to_owned();
    home.model.tool("", "noesis_judge_recall", json!({"question": question, "correct": false, "misconception": "Confuses France with England"}));
    home.model.tool(
        "",
        "noesis_judge_recall",
        json!({"question": question, "correct": true}),
    );
    home.model.text("Let's review.");
    let answer = json!({"cmd": "noesis_answer", "session": id, "question": question,
        "answer": {"text": "London", "dont_know": false, "confidence": 90}, "idempotency_key": "answer-recall"});
    home.ok(answer.clone()).await;
    let judged = home
        .session_until(&id, "recall judgment", |s| {
            s["questions"][0]["result"]["correct"] == false
        })
        .await;
    assert_eq!(
        judged["questions"][0]["result"]["misconception"],
        "Confuses France with England"
    );
    home.ok(answer).await;
    let requests = home.requests_reach(5).await;
    assert!(
        requests
            .iter()
            .any(|request| request.to_string().contains("London"))
    );
    wait_for_learner_update(&mut home).await;
    home = home.restart().await;
    let restored = home.session(&id).await;
    assert_recall_evidence(&mut home).await;
    assert_eq!(restored["questions"][0]["result"]["correct"], false);
    assert_eq!(
        restored["questions"][0]["result"]["misconception"],
        "Confuses France with England"
    );
}

async fn wait_for_learner_update(home: &mut Home) {
    for _ in 0..400 {
        let learner = home
            .ok(json!({"cmd": "noesis_learner", "query": "Learn geography"}))
            .await;
        if learner["concepts"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("recall evidence was not published before restart");
}

async fn assert_recall_evidence(home: &mut Home) {
    let learner = home
        .ok(json!({"cmd": "noesis_learner", "query": "Learn geography"}))
        .await;
    let concept = learner["concepts"][0]["concept"]["id"]
        .as_str()
        .expect("a learner concept");
    let evidence = home
        .ok(json!({"cmd": "noesis_learner_evidence", "concept": concept}))
        .await;
    let evidence = evidence["evidence"].as_array().expect("an evidence page");
    assert_eq!(
        evidence
            .iter()
            .filter(|item| item["kind"] == "predict_answer" && item["outcome"] == "incorrect")
            .count(),
        1
    );
    assert_eq!(
        evidence
            .iter()
            .filter(|item| item["kind"] == "misconception_observed"
                && item["note"] == "Confuses France with England")
            .count(),
        1
    );
}
