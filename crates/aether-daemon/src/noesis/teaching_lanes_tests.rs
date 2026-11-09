use super::*;

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
async fn engine_failure_stalls_visibly_and_retry_can_recover() {
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
    harness.settled().await;
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
    assert_eq!(client.requests().len(), 2);
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
