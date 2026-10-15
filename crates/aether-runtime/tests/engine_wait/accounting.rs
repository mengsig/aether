use super::*;

#[tokio::test]
async fn operator_cancellation_keeps_receipt_after_the_synthetic_cancel() {
    let measured = Usage {
        input_tokens: 12_345,
        output_tokens: 67,
        ..Default::default()
    };
    let mut h = harness_with(
        Duration::from_secs(30),
        OnKill::Report(vec![
            cancelled_with(Usage::default()),
            cancelled_with(measured),
        ]),
    )
    .await;
    let worker = start_coordinator(&h);
    let turn = turn_of(&mut h, worker).await;
    h.runtime
        .cancel_agent(worker, Some("operator stop".into()), Actor::user())
        .expect("cancel worker");
    let ended = wait_until(h.store.as_ref(), worker, |agent| {
        agent.stats.usage.total_tokens() == measured.total_tokens()
    })
    .await;
    assert_eq!(ended.status.kind(), AgentStatusKind::Cancelled);
    assert!(turn.observed.cancelled());
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn terminal_reporting_keeps_final_usage_without_allowing_more_work() {
    let measured = Usage {
        input_tokens: 12_345,
        output_tokens: 67,
        ..Default::default()
    };
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            end_usage: Some(measured),
            ..Default::default()
        },
    )
    .await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            objective: Some("finish and report usage".into()),
            start: true,
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;
    let (reply, is_error) = call(&h, &turn, "complete_task", json!({"summary": "done"})).await;
    assert!(!is_error, "{reply}");
    assert!(
        !turn.observed.cancelled(),
        "receipt reporting was cancelled immediately"
    );
    assert!(
        !turn.observed.stopped(),
        "the native final response must not be interrupted"
    );
    let (reply, is_error) = call(&h, &turn, "delegate", json!({
        "model": "claude-code/haiku", "effort": "low", "objective": "must not start", "wait": false
    })).await;
    assert!(
        is_error,
        "new work was admitted after terminal completion: {reply}"
    );
    assert!(reply.contains(TURN_IS_OVER));
    assert!(
        h.store
            .agents(&AgentFilter {
                parent: Some(worker),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    turn.end
        .send(())
        .expect("engine still has its reporting window");
    let ended = wait_until(h.store.as_ref(), worker, |agent| agent.status.is_terminal()).await;
    assert_eq!(ended.stats.usage.input_tokens, measured.input_tokens);
    assert_eq!(ended.stats.usage.output_tokens, measured.output_tokens);
    assert!(!turn.observed.cancelled());
    assert_eq!(
        custom(h.store.as_ref(), worker, "engine.parked").unwrap()["stopped_by"],
        "engine"
    );
    h.runtime.shutdown().await;
}

/// `codex/default` has no registry price until a session names its model, so
/// the engine's own totals are the only cost signal there is. A turn the runtime had to stop must still keep what
/// the engine measured before it went, and record it as a floor.
#[tokio::test]
async fn a_stopped_turn_keeps_the_usage_the_engine_measured() {
    let measured = Usage {
        input_tokens: 120_000,
        output_tokens: 4_000,
        ..Default::default()
    };
    let mut h = harness_with(
        Duration::from_millis(200),
        OnKill::Report(vec![cancelled_with(measured)]),
    )
    .await;
    let coordinator = start_agent(
        &h,
        CreateAgentSpec {
            model: Some(ModelRef::new("codex", "default")),
            ..coordinator_spec()
        },
    );
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    let parked = wait_for_park(&h, coordinator).await;
    assert!(first.observed.cancelled());
    assert!(
        first.observed.stopped(),
        "Codex ignores a closed stdin; it has to be told to stop"
    );
    assert_eq!(
        parked.stats.usage.total_tokens(),
        measured.total_tokens(),
        "the stopped turn was booked at zero tokens"
    );
    assert!(
        !parked.stats.usage.cost_complete,
        "an unpriced provider's total is a floor, never a confident sum"
    );
    h.runtime.shutdown().await;
}

/// The generic adapter's cancel outcome is a synthetic one: `cancelled`, and
/// no usage at all. A reap must not let it replace the result the engine
/// actually measured, or an unpriced provider's turn books zero.
#[tokio::test]
async fn the_reap_keeps_a_measured_outcome_over_a_synthetic_zero_one() {
    let measured = Usage {
        input_tokens: 90_000,
        output_tokens: 3_000,
        ..Default::default()
    };
    let mut h = harness_with(
        Duration::from_millis(200),
        OnKill::Report(vec![
            EngineOutcome {
                usage: measured,
                text: Some("what it had done".into()),
                ..Default::default()
            },
            cancelled_with(Usage::default()),
        ]),
    )
    .await;
    let coordinator = start_agent(
        &h,
        CreateAgentSpec {
            model: Some(ModelRef::new("codex", "default")),
            ..coordinator_spec()
        },
    );
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    let parked = wait_for_park(&h, coordinator).await;
    assert!(first.observed.cancelled());
    assert_eq!(
        parked.stats.usage.total_tokens(),
        measured.total_tokens(),
        "the zero-usage cancel outcome overwrote what the engine measured"
    );
    let record = custom(h.store.as_ref(), coordinator, "engine.parked").expect("recorded");
    assert_eq!(
        record["error"], "cancelled",
        "the late word's reason is still worth keeping"
    );
    assert_eq!(
        record["is_error"], true,
        "the flag must travel with the error it names, not the measured half"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn the_reap_keeps_cancellation_when_a_successful_receipt_arrives_later() {
    let measured = Usage {
        input_tokens: 90_000,
        output_tokens: 3_000,
        ..Default::default()
    };
    let mut h = harness_with(
        Duration::from_millis(200),
        OnKill::Report(vec![
            cancelled_with(Usage::default()),
            EngineOutcome {
                usage: measured,
                text: Some("what it had done".into()),
                ..Default::default()
            },
        ]),
    )
    .await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;
    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");
    let parked = wait_for_park(&h, coordinator).await;
    assert!(first.observed.cancelled());
    assert_eq!(parked.stats.usage.total_tokens(), measured.total_tokens());
    let record = custom(h.store.as_ref(), coordinator, "engine.parked").expect("recorded");
    assert_eq!(record["error"], "cancelled");
    assert_eq!(record["is_error"], true);
    h.runtime.shutdown().await;
}

/// An adapter that force-killed the process when its own wind-down grace
/// expired did not have the engine stop of its own accord. Recording that as
/// `"engine"` would hide exactly the engine an operator is looking for.
#[tokio::test]
async fn an_adapter_force_kill_is_not_recorded_as_the_engine_stopping_itself() {
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
        Script {
            end_forced: true,
            ..Default::default()
        },
    )
    .await;
    let coordinator = start_coordinator(&h);
    let first = turn_of(&mut h, coordinator).await;

    let (reply, is_error) = call(&h, &first, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!is_error, "{reply}");

    let _ = first.end.send(());
    wait_for_park(&h, coordinator).await;
    assert!(
        !first.observed.cancelled(),
        "the runtime stopped it, so this proves nothing about the adapter"
    );
    assert_eq!(
        custom(h.store.as_ref(), coordinator, "engine.parked").expect("recorded")["stopped_by"],
        json!("adapter")
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn park_timer_killing_an_in_flight_request_is_interrupted() {
    let mut h = harness_with(
        Duration::from_millis(200),
        OnKill::Report(vec![cancelled_with(Usage::default())]),
    )
    .await;
    let worker = start_agent(
        &h,
        CreateAgentSpec {
            objective: Some("finish while accounting drains".into()),
            start: true,
            model: Some(ModelRef::new("claude-code", "haiku")),
            config: Some(AgentConfig {
                approval: ApprovalPolicy::none(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let turn = turn_of(&mut h, worker).await;
    let events = turn.events.upgrade().expect("live engine");
    events
        .send(EngineEvent::UsageDelta {
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
                ..Default::default()
            },
        })
        .await
        .expect("first request usage");
    let (reply, is_error) = call(&h, &turn, "complete_task", json!({"summary":"done"})).await;
    assert!(!is_error, "{reply}");
    events
        .send(EngineEvent::Delta(
            aether_core::model::StreamEvent::MessageStart {
                provider_message_id: None,
                model: None,
            },
        ))
        .await
        .expect("request started after completion");
    let ended = wait_until(h.store.as_ref(), worker, |a| a.status.is_terminal()).await;
    assert!(
        turn.observed.cancelled(),
        "park timer must kill the in-flight request"
    );
    assert_eq!(ended.stats.usage.total_tokens(), 110);
    assert!(!ended.stats.usage.cost_complete);
    assert_eq!(
        ended.stats.usage.cost_gap,
        Some(aether_core::model::CostGap::Interrupted)
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn native_park_stop_of_an_in_flight_request_is_interrupted() {
    let mut h = harness(Duration::from_secs(30)).await;
    let worker = start_coordinator(&h);
    let turn = turn_of(&mut h, worker).await;
    let events = turn.events.upgrade().expect("live engine");
    events
        .send(EngineEvent::UsageDelta {
            usage: Usage {
                input_tokens: 100,
                output_tokens: 10,
                ..Default::default()
            },
        })
        .await
        .expect("first request usage");
    let (reply, error) = call(&h, &turn, "wait_for", json!({"timeout_seconds": 3600})).await;
    assert!(!error, "{reply}");
    assert!(turn.observed.stopped());
    assert!(!turn.observed.cancelled());
    events
        .send(EngineEvent::Delta(
            aether_core::model::StreamEvent::MessageStart {
                provider_message_id: None,
                model: None,
            },
        ))
        .await
        .expect("next request in flight");
    turn.end
        .send(())
        .expect("native closing result after interrupt");
    let ended = wait_for_park(&h, worker).await;
    assert!(!turn.observed.cancelled());
    assert_eq!(ended.stats.usage.total_tokens(), 110);
    assert!(!ended.stats.usage.cost_complete);
    assert_eq!(
        ended.stats.usage.cost_gap,
        Some(aether_core::model::CostGap::Interrupted)
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn park_stop_after_closing_receipt_stays_complete() {
    let measured = Usage {
        input_tokens: 100,
        output_tokens: 10,
        ..Default::default()
    };
    let mut h = harness_full(
        Duration::from_millis(200),
        OnKill::Report(vec![EngineOutcome {
            closing_accounting: true,
            usage: measured,
            ..Default::default()
        }]),
        Script {
            background: Some("still-running".into()),
            ..Default::default()
        },
    )
    .await;
    let mut spec = coordinator_spec();
    spec.config.as_mut().expect("configured agent").long_lived = false;
    let worker = start_agent(&h, spec);
    let turn = turn_of(&mut h, worker).await;
    let (reply, error) = call(&h, &turn, "complete_task", json!({"summary": "done"})).await;
    assert!(!error, "{reply}");
    assert!(!turn.observed.stopped());
    turn.events
        .upgrade()
        .expect("live engine")
        .send(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            usage: measured,
            ..Default::default()
        }))
        .await
        .expect("receipt before the park timer fires");
    let ended = wait_until(h.store.as_ref(), worker, |a| a.status.is_terminal()).await;
    assert!(
        turn.observed.cancelled(),
        "the park timer stopped the engine"
    );
    assert_eq!(ended.stats.usage.total_tokens(), 110);
    assert!(ended.stats.usage.cost_complete);
    assert_eq!(ended.stats.usage.cost_gap, None);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn terminal_park_closing_receipt_stays_complete() {
    let mut h = harness_full(
        Duration::from_secs(30),
        OnKill::Silent,
        Script {
            end_usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 10,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await;
    let mut spec = coordinator_spec();
    spec.config.as_mut().expect("configured agent").long_lived = false;
    let worker = start_agent(&h, spec);
    let turn = turn_of(&mut h, worker).await;
    let (reply, error) = call(&h, &turn, "complete_task", json!({"summary": "done"})).await;
    assert!(!error, "{reply}");
    assert!(!turn.observed.stopped());
    turn.end.send(()).expect("native terminal receipt");
    let ended = wait_until(h.store.as_ref(), worker, |a| a.status.is_terminal()).await;
    assert!(!turn.observed.cancelled());
    assert_eq!(ended.stats.usage.total_tokens(), 110);
    assert!(ended.stats.usage.cost_complete);
    assert_eq!(ended.stats.usage.cost_gap, None);
    h.runtime.shutdown().await;
}
