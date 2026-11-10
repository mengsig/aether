use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_during_a_held_call_resumes_the_turn_after_restart() {
    let home = Home::new();
    let first = Arc::new(ScriptedClient::new());
    let _held = first.hold_next_response();
    first.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, first.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while first.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the teaching call started");
    assert!(harness.daemon.runtime.turn_in_flight(created.agent));
    tokio::time::timeout(Duration::from_secs(5), harness.close())
        .await
        .expect("shutdown cancels the held call");

    let second = Arc::new(ScriptedClient::new());
    second.push(ScriptedClient::tool_call(ASK, capital_question()));
    let reopened = Harness::open(&home, second.clone()).await;
    let posed = tokio::time::timeout(Duration::from_secs(5), reopened.posed(&created.id))
        .await
        .expect("the interrupted turn resumes");
    assert!(
        posed.stalled.is_none(),
        "shutdown must not record a failure"
    );
    assert!(posed.open_question.is_some());
    assert_eq!(second.requests().len(), 1);
    reopened.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_preserves_judgment_events_and_evidence_before_resuming() {
    let home = Home::new();
    let first = Arc::new(ScriptedClient::new());
    let (harness, created) = hold_after_recall_judgment(&home, &first).await;
    tokio::time::timeout(Duration::from_secs(5), harness.close())
        .await
        .expect("shutdown cancels the held call");

    let second = Arc::new(ScriptedClient::new());
    second.push(ScriptedClient::text("Continue."));
    let reopened = Harness::open(&home, second.clone()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while second.requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        reopened.settled().await;
    })
    .await
    .expect("the pending turn resumes");
    assert_judgment_published_once(&home, &reopened, &created.id);
    assert!(
        reopened
            .session(&created.id)
            .await
            .unwrap()
            .stalled
            .is_none()
    );
    reopened.close().await;
}

async fn hold_after_recall_judgment(
    home: &Home,
    first: &Arc<ScriptedClient>,
) -> (Harness, NoesisSession) {
    first.push(ScriptedClient::tool_call(
        ASK,
        json!({
            "kind": "recall", "prompt": "What is 2 + 2?", "answer_key": "four"
        }),
    ));
    let harness = Harness::open(home, first.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    let question = posed.open_question.unwrap().id;
    first.push(ScriptedClient::tool_call(
        "noesis_judge_recall",
        json!({
            "question": question, "correct": true
        }),
    ));
    first.push(ScriptedClient::text("Continue."));
    let _held = first.hold_response_at(3);
    harness
        .send_json(json!({"cmd": "noesis_answer", "session": created.id,
        "question": question, "answer": {"text": "four", "dont_know": false},
        "idempotency_key": "recall-before-shutdown"}))
        .await
        .expect("answer");
    tokio::time::timeout(Duration::from_secs(2), async {
        while first.requests().len() < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("judgment completed and the next call is held");
    assert_eq!(
        harness.session(&created.id).await.unwrap().questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(true)
    );
    assert!(harness.daemon.runtime.turn_in_flight(created.agent));
    (harness, created)
}

fn assert_judgment_published_once(home: &Home, reopened: &Harness, session: &str) {
    let events = reopened.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
        EventKind::NoesisAnswerRecorded { session: event_session, result }
            if event_session == session && result.correct == Some(true)))
            .count(),
        1
    );
    let conn = rusqlite::Connection::open(home.data.path().join("noesis.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM evidence WHERE session = ?1 AND kind = 'probe_answer'",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "the judgment's learner evidence is appended once");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_sessions_answer_finishes_while_a_turn_is_held() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let release = client.hold_response_at(2);
    let _free = release.clone().drop_guard();
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let first = harness.create(json!({})).await;
    let first = harness.posed(&first.id).await;
    harness.settled().await;
    let second = harness.create(json!({})).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the second session reached its held call");
    assert!(harness.daemon.runtime.turn_in_flight(second.agent));
    let desk = {
        let state = harness.daemon.noesis.state.lock().await;
        let State::Running(live) = &*state else {
            panic!("service running")
        };
        live.desk.clone()
    };
    desk.lock().await.undelivered.insert(
        "agt_other".into(),
        [aether_noesis::teach::Input::Learner("still owed".into())].into(),
    );
    let conflict = tokio::time::timeout(
        Duration::from_secs(2),
        harness.send_json(json!({
            "cmd": "noesis_send", "session": second.id,
            "text": "Please continue.", "idempotency_key": "same-session-send"
        })),
    )
    .await
    .expect("the active session answers promptly")
    .expect_err("a turn cannot be changed while it runs");
    assert_eq!(conflict.reason.as_deref(), Some("conflict"));
    let answer = tokio::time::timeout(Duration::from_secs(2), harness.answer(&first, "Paris"))
        .await
        .expect("the answer did not wait for another session")
        .expect("the answer was recorded");
    assert!(matches!(answer, Reply::Ok));
    let recorded = harness
        .session(&first.id)
        .await
        .expect("the first session is visible");
    assert!(recorded.open_question.is_none());
    assert!(recorded.questions[0].answer.is_some());
    release.cancel();
    harness.posed(&second.id).await;
    harness.settled().await;
    assert_eq!(desk.lock().await.undelivered["agt_other"].len(), 1);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn independent_commands_finish_while_a_teaching_call_is_held() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    assert!(harness.daemon.runtime.turn_in_flight(first.agent));
    let status = harness
        .daemon
        .noesis
        .status(&harness.daemon.runtime)
        .await
        .expect("status");
    assert!(
        status.jobs_running >= 1,
        "the active teaching call is counted"
    );
    harness
        .client
        .push(ScriptedClient::tool_call(ASK, capital_question()));

    let create = json!({
        "cmd": "noesis_create_session", "goal": "Learn geometry", "mode": "teach",
        "idempotency_key": "independent-create"
    });
    let Reply::NoesisSession(second) =
        tokio::time::timeout(Duration::from_secs(2), harness.send_json(create.clone()))
            .await
            .expect("create did not wait for the turn")
            .expect("created")
    else {
        panic!("a session reply");
    };
    assert_ne!(first.id, second.id);
    let Reply::NoesisSession(replayed) = harness.send_json(create).await.expect("replayed") else {
        panic!("a session reply");
    };
    assert_eq!(second.id, replayed.id);
    assert_eq!(
        harness.session(&second.id).await.expect("visible").id,
        second.id
    );

    assert_extract_queued(&harness, &second.id).await;
    assert_configure_saves_promptly(&harness).await;
    assert_eq!(
        harness.client.requests().len(),
        1,
        "second turn remains queued"
    );

    gated.release.cancel();
    harness.posed(&first.id).await;
    harness.posed(&second.id).await;
    harness.settled().await;
    assert_eq!(harness.client.requests().len(), 2);
    harness.close().await;
}

async fn assert_extract_queued(harness: &Harness, session: &str) {
    let extract = json!({"cmd": "noesis_extract", "session": session});
    let Reply::NoesisJob { job } =
        tokio::time::timeout(Duration::from_secs(2), harness.send_json(extract))
            .await
            .expect("extract did not wait for the turn")
            .expect("queued")
    else {
        panic!("a job reply");
    };
    assert!(harness.events().iter().any(|event| matches!(
        &event.kind, EventKind::NoesisJobUpdated { job: announced } if announced.id == job.id
    )));
    assert_eq!(job.target, session);
}

async fn held_write(harness: &Harness, command: Value) -> Reply {
    let first = tokio::time::timeout(Duration::from_secs(2), harness.send_json(command.clone()))
        .await
        .expect("write did not wait for the teaching turn")
        .expect("write succeeded");
    let replay = tokio::time::timeout(Duration::from_secs(2), harness.send_json(command))
        .await
        .expect("receipt did not wait for the teaching turn")
        .expect("receipt replayed");
    assert_eq!(first, replay);
    first
}

async fn publish_notes(harness: &Harness) -> aether_core::id::ArtifactId {
    use aether_core::artifact::ArtifactKind;
    use aether_runtime::{CreateAgentSpec, artifacts::ArtifactPublication};
    let runtime = &harness.daemon.runtime;
    let (agent, task) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    let artifact = runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::File,
                name: "notes.md".into(),
                summary: "notes".into(),
                content_type: Some("text/markdown".into()),
                bytes: b"Alpha holds. Beta follows.".to_vec(),
                agent: agent.id,
                task: task.map(|task| task.id),
            },
            Actor::user(),
        )
        .await
        .expect("artifact");
    artifact.id
}

async fn seed_writable_rows(harness: &Harness) -> String {
    use aether_noesis::learner;
    use aether_protocol::noesis::ConceptKind;
    let state = harness.daemon.noesis.state.lock().await;
    let State::Running(live) = &*state else {
        panic!("running")
    };
    let desk = live.desk.lock().await;
    let conn = desk.noesis.as_ref().expect("open").store().connection();
    let concept = learner::ensure_concept(
        conn,
        ConceptKind::Topic,
        "Limits",
        "Limits",
        chrono::Utc::now(),
    )
    .expect("concept");
    conn.execute("INSERT INTO propositions(id, name, statement, created_at) VALUES ('p_test', 'Test', 'Test holds', 'now')", []).expect("proposition");
    conn.execute("INSERT INTO sources(id, name, artifact, imported_at) VALUES ('s_test', 'Test', 'a_test', 'now')", []).expect("source");
    concept.id
}

async fn assert_import_and_cancel(harness: &Harness) {
    use aether_core::noesis::{JobKind, JobState};
    let job = {
        let state = harness.daemon.noesis.state.lock().await;
        let State::Running(live) = &*state else {
            panic!("running")
        };
        let desk = live.desk.lock().await;
        let conn = desk.noesis.as_ref().expect("open").store().connection();
        aether_noesis::extract::jobs::create(conn, JobKind::ExtractSource, "s_test")
            .expect("queued job")
    };
    let cancelled = held_write(
        harness,
        json!({"cmd":"noesis_cancel_job", "job":job.id, "idempotency_key":"held-cancel"}),
    )
    .await;
    assert!(
        matches!(cancelled, Reply::NoesisJob { job: ref cancelled } if cancelled.id == job.id && cancelled.state == JobState::Cancelled)
    );
    assert!(harness.events().iter().any(|event| matches!(&event.kind, EventKind::NoesisJobUpdated { job: announced } if announced.id == job.id && announced.state == JobState::Cancelled)));
    let artifact = publish_notes(harness).await;
    let imported = held_write(harness, json!({"cmd":"noesis_import_source", "name":"Notes", "artifact":artifact, "idempotency_key":"held-import"})).await;
    let Reply::NoesisImport { source, job } = imported else {
        panic!("import reply")
    };
    assert_eq!(job.target, source);
    assert!(harness.events().iter().any(|event| matches!(&event.kind, EventKind::NoesisJobUpdated { job: announced } if announced.id == job.id)));
}

async fn assert_correction_and_deletions(harness: &Harness, concept: &str) {
    use aether_protocol::noesis::ConceptStateKind;
    let corrected = held_write(harness, json!({"cmd":"noesis_correct_learner", "concept":concept, "state":"fragile", "reason":"shaky", "idempotency_key":"held-correct"})).await;
    assert!(
        matches!(corrected, Reply::ConceptState(ref state) if state.state == ConceptStateKind::Fragile)
    );
    assert!(harness.events().iter().any(|event| matches!(&event.kind, EventKind::NoesisLearnerUpdated { concepts } if concepts.iter().any(|id| id == concept))));
    assert_eq!(held_write(harness, json!({"cmd":"noesis_delete_proposition", "proposition":"p_test", "idempotency_key":"held-proposition"})).await, Reply::Ok);
    assert_eq!(held_write(harness, json!({"cmd":"noesis_delete_source", "source":"s_test", "graph":"preserve", "idempotency_key":"held-source"})).await, Reply::Ok);
    let state = harness.daemon.noesis.state.lock().await;
    let State::Running(live) = &*state else {
        panic!("running")
    };
    let desk = live.desk.lock().await;
    let conn = desk.noesis.as_ref().expect("open").store().connection();
    let count: i64 = conn.query_row("SELECT (SELECT count(*) FROM propositions WHERE id = 'p_test') + (SELECT count(*) FROM sources WHERE id = 's_test')", [], |row| row.get(0)).expect("deleted rows");
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn graph_and_learner_writes_finish_during_a_held_turn() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    let concept = seed_writable_rows(&harness).await;
    assert_import_and_cancel(&harness).await;
    assert_correction_and_deletions(&harness, &concept).await;
    assert!(harness.daemon.runtime.turn_in_flight(first.agent));
    gated.release.cancel();
    harness.posed(&first.id).await;
    harness.settled().await;
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deletion_conflicts_only_with_the_held_session() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    let other = harness.create(json!({})).await;
    let active = json!({"cmd":"noesis_delete_session", "session":first.id, "graph":"preserve", "idempotency_key":"held-active-delete"});
    let error = tokio::time::timeout(Duration::from_secs(2), harness.send_json(active))
        .await
        .expect("conflict did not wait")
        .expect_err("active session is protected");
    assert_eq!(error.reason.as_deref(), Some("conflict"));
    assert_eq!(
        harness
            .session(&first.id)
            .await
            .expect("active session remains")
            .id,
        first.id
    );
    let delete = json!({"cmd":"noesis_delete_session", "session":other.id, "graph":"preserve", "idempotency_key":"held-other-delete"});
    assert_eq!(held_write(&harness, delete).await, Reply::Ok);
    assert_eq!(
        harness.session(&other.id).await.expect_err("deleted").code,
        ErrorCode::NotFound
    );
    gated.release.cancel();
    harness.posed(&first.id).await;
    harness.settled().await;
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_timeout_retries_automatically_and_poses_the_question() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(Err(ModelError::new(
        aether_core::model::ModelErrorClass::Timeout,
        "no progress",
    )));
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    assert!(posed.stalled.is_none());
    assert_eq!(client.requests().len(), 2);
    assert!(!harness.daemon.runtime.turn_in_flight(created.agent));
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn writing_and_liveness_last_through_multiple_model_calls() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(
        "noesis_graph_search",
        json!({"query": "geometry"}),
    ));
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let release = client.hold_response_at(2);
    let _release_guard = release.clone().drop_guard();
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the next model call started");
    assert!(harness.daemon.runtime.turn_in_flight(created.agent));
    let mut starts = 0;
    let mut finishes = 0;
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Delta { agent, delta, .. } = message
            && agent == created.agent
        {
            starts += usize::from(matches!(
                delta,
                aether_runtime::DeltaKind::TurnStarted { .. }
            ));
            finishes += usize::from(matches!(delta, aether_runtime::DeltaKind::TurnFinished));
        }
    }
    assert_eq!((starts, finishes), (1, 0));
    release.cancel();
    harness.posed(&created.id).await;
    harness.settled().await;
    assert!(!harness.daemon.runtime.turn_in_flight(created.agent));
    let mut finished = false;
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Delta {
            agent,
            delta: aether_runtime::DeltaKind::TurnFinished,
            ..
        } = message
        {
            finished |= agent == created.agent;
        }
    }
    assert!(finished, "the writing stream closes when the turn ends");
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_timeouts_stall_the_session_and_retry_can_recover() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    for _ in 0..2 {
        client.push(Err(ModelError::new(
            aether_core::model::ModelErrorClass::Timeout,
            "no progress",
        )));
    }
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    harness.settled().await;
    assert_eq!(client.requests().len(), 2, "one automatic retry");
    let stalled = harness.session(&created.id).await.unwrap();
    assert_eq!(
        stalled.stalled.unwrap().reason,
        aether_core::noesis::StallReason::EngineStalled
    );
    assert!(!harness.daemon.runtime.turn_in_flight(created.agent));
    assert!(
        harness
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::NoesisSessionStalled { .. }))
    );

    harness
        .send_json(json!({"cmd": "noesis_retry_turn", "session": created.id}))
        .await
        .expect("retry");
    let resumed = harness.posed(&created.id).await;
    harness.settled().await;
    assert!(resumed.stalled.is_none());
    assert_eq!(client.requests().len(), 3);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_engine_failure_retries_automatically() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(Err(ModelError::new(
        aether_core::model::ModelErrorClass::Unknown,
        "engine failed",
    )));
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    assert!(posed.stalled.is_none());
    assert_eq!(client.requests().len(), 2);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_streams_retry_and_keep_reported_usage() {
    for invalid_input in [false, true] {
        let home = Home::new();
        let client = Arc::new(ScriptedClient::new());
        let usage = StreamEvent::Usage {
            usage: Usage {
                cost_usd: 0.05,
                ..Usage::default()
            },
        };
        let mut events = vec![usage];
        if invalid_input {
            events.push(StreamEvent::BlockStart {
                index: 0,
                block: aether_core::model::BlockStart::ToolUse {
                    id: "bad".into(),
                    name: ASK.into(),
                    input: None,
                },
            });
            events.push(StreamEvent::ToolInputDelta {
                index: 0,
                partial_json: "{".into(),
            });
        } else {
            events.push(StreamEvent::TextDelta {
                index: 7,
                text: "orphan".into(),
            });
        }
        client.push(Ok(events));
        client.push(ScriptedClient::tool_call(ASK, capital_question()));
        let harness = Harness::open(&home, client.clone()).await;
        harness.configure(teaching_on(|_| {})).await;
        let created = harness.create(json!({})).await;
        let posed = tokio::time::timeout(Duration::from_secs(5), harness.posed(&created.id))
            .await
            .expect("malformed stream is retried");
        harness.settled().await;
        assert!(posed.stalled.is_none());
        assert_eq!(client.requests().len(), 2);
        let Reply::NoesisStatus(status) = harness.send(Command::NoesisStatus).await.unwrap() else {
            panic!("status");
        };
        assert!(
            status.spend_today.interactive.cost_usd >= 0.05,
            "reported usage is charged"
        );
        harness.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_failure_stalls_visibly_and_retry_can_recover() {
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    for _ in 0..2 {
        client.push(Err(ModelError::new(
            aether_core::model::ModelErrorClass::Unknown,
            "engine failed",
        )));
    }
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let harness = Harness::open(&home, client.clone()).await;
    harness.configure(teaching_on(|_| {})).await;
    let created = harness.create(json!({})).await;
    harness.settled().await;
    let stalled = harness.session(&created.id).await.unwrap();
    assert_eq!(
        stalled.stalled.unwrap().reason,
        aether_core::noesis::StallReason::EngineStalled
    );
    assert_eq!(client.requests().len(), 2, "one automatic retry");
    assert!(!harness.daemon.runtime.turn_in_flight(created.agent));
    assert!(
        harness
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::NoesisSessionStalled { .. }))
    );
    harness
        .send_json(json!({"cmd": "noesis_retry_turn", "session": created.id}))
        .await
        .expect("retry");
    let resumed = harness.posed(&created.id).await;
    harness.settled().await;
    assert!(resumed.stalled.is_none());
    assert_eq!(client.requests().len(), 3);
    harness.close().await;
}

async fn assert_configure_saves_promptly(harness: &Harness) {
    let change = Command::NoesisConfigure {
        patch: NoesisSettingsPatch {
            budget: Some(BudgetPatch {
                interactive_usd_daily: Some(9.0),
                ..Default::default()
            }),
            ..Default::default()
        },
        idempotency_key: None,
    };
    let reply = tokio::time::timeout(Duration::from_secs(2), harness.send(change))
        .await
        .expect("configure did not wait for the turn")
        .expect("settings saved");
    let Reply::NoesisStatus(status) = reply else {
        panic!("status")
    };
    assert_eq!(status.settings.budget.interactive_usd_daily, 9.0);
    assert_ne!(status.effective.budget.interactive_usd_daily, 9.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_turn_waits_for_saved_settings_before_it_starts() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    assert_configure_saves_promptly(&harness).await;
    harness
        .client
        .push(ScriptedClient::tool_call(ASK, capital_question()));
    let second = tokio::time::timeout(Duration::from_secs(2), harness.create(json!({})))
        .await
        .expect("session created while settings wait");
    assert_eq!(harness.client.requests().len(), 1);
    gated.release.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        harness.posed(&first.id).await;
        harness.posed(&second.id).await;
        harness.settled().await;
    })
    .await
    .expect("the saved settings and queued turn completed");
    let status = harness
        .daemon
        .noesis
        .status(&harness.daemon.runtime)
        .await
        .unwrap();
    assert!(!status.settings_pending);
    assert_eq!(status.effective.budget.interactive_usd_daily, 9.0);
    assert_eq!(harness.client.requests().len(), 2);
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_reenable_starts_after_the_disable_grace_expires() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    harness
        .configure(NoesisSettingsPatch {
            enabled: Some(false),
            disable: Some(DisablePatch {
                grace_seconds: Some(1),
            }),
            ..Default::default()
        })
        .await;
    let reply = tokio::time::timeout(
        Duration::from_secs(2),
        harness.send(Command::NoesisConfigure {
            patch: NoesisSettingsPatch {
                enabled: Some(true),
                budget: Some(BudgetPatch {
                    interactive_usd_daily: Some(9.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
            idempotency_key: None,
        }),
    )
    .await
    .expect("re-enable answered")
    .expect("saved");
    let Reply::NoesisStatus(saved) = reply else {
        panic!("status")
    };
    assert!(saved.settings_pending && saved.settings.enabled);
    wait_for_reenabled_settings(&harness).await;
    assert!(gated.aborted.load(Ordering::SeqCst));
    let Reply::NoesisSessions { sessions, .. } = harness
        .send_json(json!({"cmd": "noesis_list_sessions"}))
        .await
        .expect("service is usable")
    else {
        panic!("sessions")
    };
    assert!(sessions.iter().any(|session| session.id == first.id));
    harness.close().await;
}

async fn wait_for_reenabled_settings(harness: &Harness) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = harness
                .daemon
                .noesis
                .status(&harness.daemon.runtime)
                .await
                .unwrap();
            if !status.settings_pending && status.effective.enabled {
                assert_eq!(status.effective.budget.interactive_usd_daily, 9.0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("saved settings became effective");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_policy_change_refuses_a_new_session_before_creating_it() {
    let home = Home::new();
    let (harness, gated, first) = held_call(&home).await;
    let _free = gated.release.clone().drop_guard();
    let changed = Command::NoesisConfigure {
        patch: NoesisSettingsPatch {
            billing: Some(BillingPatch {
                allow_api: Some(false),
            }),
            ..Default::default()
        },
        idempotency_key: None,
    };
    tokio::time::timeout(Duration::from_secs(2), harness.send(changed))
        .await
        .unwrap()
        .expect("saved");
    let create = json!({"cmd": "noesis_create_session", "mode": "teach", "goal": "Learn geometry"});
    let error = tokio::time::timeout(Duration::from_secs(2), harness.send_json(create))
        .await
        .unwrap()
        .expect_err("model settings pending");
    assert_eq!(error.reason.as_deref(), Some("conflict"));
    let Reply::NoesisSessions { sessions, .. } = harness
        .send_json(json!({"cmd": "noesis_list_sessions"}))
        .await
        .unwrap()
    else {
        panic!("sessions")
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, first.id);
    gated.release.cancel();
    harness.settled().await;
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bundled_creation_queues_engineering_extraction_during_a_turn() {
    use aether_protocol::noesis::{EngineeringExtraction, ExtractionPatch};
    let home = Home::new();
    let client = Arc::new(ScriptedClient::new());
    client.push(ScriptedClient::tool_call(ASK, capital_question()));
    let gated = Arc::new(GatedClient::new(client.clone()));
    let harness = Harness::open_with(&home, client, gated.clone()).await;
    harness
        .configure(teaching_on(|patch| {
            patch.engineering_context = Some(true);
            patch.extraction = Some(ExtractionPatch {
                engineering: Some(EngineeringExtraction::Auto),
                ..Default::default()
            });
        }))
        .await;
    let first = harness.create(json!({})).await;
    gated.wait_entered().await;
    let _free = gated.release.clone().drop_guard();
    harness
        .client
        .push(ScriptedClient::tool_call(ASK, capital_question()));
    let bundle = json!({"context": {"title": "Release notes", "items": [{
        "ref": {"kind": "pr", "id": "401"},
        "excerpt": "Wire the teaching service.", "status": "reviewed"
    }]}});
    let second = tokio::time::timeout(Duration::from_secs(2), harness.create(bundle))
        .await
        .expect("bundled create did not wait for the turn");
    assert_ne!(first.id, second.id);
    let Reply::NoesisJobs { jobs } = harness
        .send_json(json!({"cmd": "noesis_jobs"}))
        .await
        .expect("jobs")
    else {
        panic!("a jobs reply")
    };
    assert!(jobs.iter().any(|job| job.target == second.id
        && job.kind == aether_core::noesis::JobKind::ExtractEngineering));
    gated.release.cancel();
    harness.posed(&first.id).await;
    harness.posed(&second.id).await;
    harness.settled().await;
    harness.close().await;
}
