use crate::{EngineEvent, EngineOutcome, GROUP_TERM_GRACE, run_process};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

async fn fixture(script: &str) -> (CancellationToken, crate::ProcessRun) {
    let cancel = CancellationToken::new();
    let run = run_process(
        "sh",
        vec!["-c".into(), script.into()],
        PathBuf::from("/"),
        &[],
        None,
        &[],
        "prompt\n".into(),
        true,
        Some("interrupt\n".into()),
        cancel.clone(),
        |line| {
            if line == "receipt" || line == "exhausted" {
                vec![EngineEvent::Done(EngineOutcome {
                    usage: aether_core::model::Usage {
                        output_tokens: 17,
                        ..Default::default()
                    },
                    cost_usd: Some(0.125),
                    stop_reason: Some("end_turn".into()),
                    duration_ms: Some(42),
                    session: Some("native-session".into()),
                    text: Some("final text".into()),
                    is_error: line == "exhausted",
                    error: (line == "exhausted").then(|| "Weekly limit reached".into()),
                    ..Default::default()
                })]
            } else if line == "initial_receipt" {
                vec![EngineEvent::Done(EngineOutcome::default())]
            } else if line == "accepted" {
                vec![EngineEvent::InputAccepted]
            } else if line == "usage" {
                vec![EngineEvent::UsageDelta {
                    usage: aether_core::model::Usage {
                        output_tokens: 17,
                        ..Default::default()
                    },
                }]
            } else {
                vec![EngineEvent::Note(line.into())]
            }
        },
    )
    .await
    .expect("spawn fixture");
    (cancel, run)
}

#[tokio::test]
async fn cancellation_is_the_only_terminal_event_and_preserves_native_receipt() {
    let (cancel, mut run) = fixture(
        "read -r prompt; echo ready; read -r control; [ \"$control\" = interrupt ] && echo receipt; exec sleep 30",
    ).await;
    let ready = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("fixture ready")
        .expect("ready event");
    assert!(matches!(ready, EngineEvent::Note(text) if text == "ready"));
    cancel.cancel();
    let first = tokio::time::timeout(GROUP_TERM_GRACE, run.events.recv())
        .await
        .expect("prompt cancellation")
        .expect("cancel event");
    let EngineEvent::Done(outcome) = first else {
        panic!("expected cancellation, got {first:?}");
    };
    assert!(outcome.is_error);
    assert_eq!(outcome.error.as_deref(), Some("cancelled"));
    assert_eq!(outcome.usage.output_tokens, 17);
    assert_eq!(outcome.cost_usd, Some(0.125));
    assert_eq!(outcome.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(outcome.duration_ms, Some(42));
    assert_eq!(outcome.session.as_deref(), Some("native-session"));
    assert_eq!(outcome.text.as_deref(), Some("final text"));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_preserves_native_exhaustion_in_the_terminal_receipt() {
    let (cancel, mut run) =
        fixture("read -r prompt; echo ready; read -r control; echo exhausted; exec sleep 30").await;
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("fixture ready")
        .expect("ready event");
    assert!(matches!(event, EngineEvent::Note(text) if text == "ready"));
    cancel.cancel();
    let event = tokio::time::timeout(Duration::from_secs(3), run.events.recv())
        .await
        .expect("native receipt")
        .expect("terminal event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_deref() == Some("Weekly limit reached")
            && outcome.usage.output_tokens == 17));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_drains_the_receipt_after_steering_resumes_work() {
    let (cancel, mut run) = fixture(
        "read -r prompt; echo initial_receipt; read -r steering; echo accepted; read -r control; [ \"$control\" = interrupt ] && echo receipt; exec sleep 30",
    ).await;
    run.stdin
        .as_ref()
        .expect("stdin open")
        .send("steering\n".into())
        .await
        .expect("queue steering");
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("first receipt")
        .expect("receipt event");
    assert!(matches!(event, EngineEvent::Done(outcome) if !outcome.is_error));
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("steering accepted")
        .expect("accepted event");
    assert_eq!(event, EngineEvent::InputAccepted);
    cancel.cancel();
    let event = tokio::time::timeout(Duration::from_secs(3), run.events.recv())
        .await
        .expect("interrupted receipt")
        .expect("cancel event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 17 && outcome.cost_usd == Some(0.125)
            && outcome.session.as_deref() == Some("native-session")));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_drains_resumed_usage_before_content_or_input_acknowledgement() {
    let (cancel, mut run) = fixture(
        "read -r prompt; echo initial_receipt; echo usage; read -r control; [ \"$control\" = interrupt ] && echo receipt; exec sleep 30",
    ).await;
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("initial receipt")
        .expect("receipt event");
    assert!(matches!(event, EngineEvent::Done(outcome) if !outcome.is_error));
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("resumed usage")
        .expect("usage event");
    assert!(matches!(event, EngineEvent::UsageDelta { usage } if usage.output_tokens == 17));
    cancel.cancel();
    let event = tokio::time::timeout(Duration::from_secs(3), run.events.recv())
        .await
        .expect("interrupted receipt")
        .expect("cancel event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 17 && outcome.cost_usd == Some(0.125)
            && outcome.session.as_deref() == Some("native-session")));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn interruption_accounting_arrives_before_the_terminal_cancellation() {
    let (cancel, mut run) = fixture(
        "read -r prompt; echo ready; read -r control; echo usage; echo receipt; exec sleep 30",
    )
    .await;
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("fixture ready")
        .expect("ready event");
    assert!(matches!(event, EngineEvent::Note(text) if text == "ready"));
    cancel.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = run.events.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("process reaped");
    assert!(
        matches!(&events[..], [EngineEvent::UsageDelta { usage }, EngineEvent::Done(outcome)]
        if usage.output_tokens == 17 && outcome.usage.output_tokens == 17
            && outcome.is_error && outcome.error.as_deref() == Some("cancelled"))
    );
}

#[tokio::test]
async fn an_unresponsive_interrupt_still_closes_the_stream() {
    let (cancel, mut run) = fixture("read -r prompt; echo ready; exec sleep 30").await;
    tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("ready timeout")
        .expect("ready event");
    cancel.cancel();
    let event = tokio::time::timeout(Duration::from_secs(3), run.events.recv())
        .await
        .expect("bounded native interrupt")
        .expect("cancel event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 0));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("interrupt and TERM fallback must be bounded")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_after_a_terminal_receipt_does_not_emit_another_done() {
    let (cancel, mut run) = fixture("read -r prompt; echo receipt; exec sleep 30").await;
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("native receipt")
        .expect("receipt event");
    assert!(matches!(event, EngineEvent::Done(outcome) if !outcome.is_error));
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_while_parsing_a_receipt_keeps_the_terminal_outcome_cancelled() {
    let cancel = CancellationToken::new();
    let parser_cancel = cancel.clone();
    let mut run = run_process(
        "sh",
        vec![
            "-c".into(),
            "read -r prompt; echo receipt; exec sleep 30".into(),
        ],
        PathBuf::from("/"),
        &[],
        None,
        &[],
        "prompt\n".into(),
        true,
        Some("interrupt\n".into()),
        cancel,
        move |_| {
            // Deterministically cancel after stdout wins select, before
            // its parsed receipt reaches the event channel.
            parser_cancel.cancel();
            vec![EngineEvent::Done(EngineOutcome {
                usage: aether_core::model::Usage {
                    output_tokens: 17,
                    ..Default::default()
                },
                ..Default::default()
            })]
        },
    )
    .await
    .expect("spawn fixture");
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("receipt timeout")
        .expect("receipt event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 17));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn cancellation_while_a_receipt_waits_for_channel_capacity_is_terminal() {
    let cancel = CancellationToken::new();
    let (parsed, received) = tokio::sync::oneshot::channel();
    let mut parsed = Some(parsed);
    let mut run = run_process(
        "sh",
        vec!["-c".into(),
            "read -r count; i=0; while [ \"$i\" -lt \"$count\" ]; do echo fill; i=$((i+1)); done; echo receipt; exec sleep 30".into()],
        PathBuf::from("/"),
        &[],
        None,
        &[],
        String::new(),
        true,
        Some("interrupt\n".into()),
        cancel.clone(),
        move |line| {
            if line == "receipt" {
                parsed.take().expect("one receipt").send(()).expect("parser signal");
                vec![EngineEvent::Done(EngineOutcome {
                    usage: aether_core::model::Usage {
                        output_tokens: 17,
                        ..Default::default()
                    },
                    ..Default::default()
                })]
            } else {
                vec![EngineEvent::Note(line.into())]
            }
        },
    )
    .await
    .expect("spawn fixture");
    let capacity = run.events.max_capacity();
    run.stdin
        .as_ref()
        .expect("stdin open")
        .send(format!("{capacity}\n"))
        .await
        .expect("fill event channel");
    tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .expect("receipt parsed")
        .expect("parser signal received");
    assert_eq!(
        run.events.len(),
        capacity,
        "receipt must wait for channel capacity"
    );
    cancel.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = run.events.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("process reaped");
    assert_eq!(events.len(), capacity + 1);
    assert!(matches!(events.last(), Some(EngineEvent::Done(outcome))
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 17));
}

#[tokio::test]
async fn cancellation_reaps_the_process_when_the_drain_fills_an_unread_channel() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let pid_file = directory.path().join("pid");
    let cancel = CancellationToken::new();
    let mut run = run_process(
        "sh",
        vec!["-c".into(),
            "read -r count; echo $$ > \"$1\"; echo ready; read -r control; i=0; while [ \"$i\" -lt \"$count\" ]; do echo fill; i=$((i+1)); done; echo receipt; exec sleep 30".into(),
            "fixture".into(), pid_file.to_string_lossy().into_owned()],
        PathBuf::from("/"),
        &[],
        None,
        &[],
        String::new(),
        true,
        Some("interrupt\n".into()),
        cancel.clone(),
        |line| {
            if line == "receipt" {
                vec![EngineEvent::Done(EngineOutcome {
                    usage: aether_core::model::Usage {
                        output_tokens: 17,
                        ..Default::default()
                    },
                    ..Default::default()
                })]
            } else {
                vec![EngineEvent::Note(line.into())]
            }
        },
    )
    .await
    .expect("spawn fixture");
    let capacity = run.events.max_capacity();
    run.stdin
        .as_ref()
        .expect("stdin open")
        .send(format!("{capacity}\n"))
        .await
        .expect("fill drain channel");
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("fixture ready")
        .expect("ready event");
    assert!(matches!(event, EngineEvent::Note(text) if text == "ready"));
    let pid = std::fs::read_to_string(pid_file)
        .expect("fixture pid")
        .trim()
        .parse::<i32>()
        .expect("pid number");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
                Ok(()) => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(nix::errno::Errno::ESRCH) => break,
                Err(error) => panic!("could not check fixture process: {error}"),
            }
        }
    })
    .await
    .expect("cancellation must reap the process without channel progress");
    let events = tokio::time::timeout(Duration::from_secs(3), async {
        let mut events = Vec::new();
        while let Some(event) = run.events.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("terminal outcome after backpressure clears");
    assert_eq!(events.len(), capacity + 1);
    assert!(matches!(events.last(), Some(EngineEvent::Done(outcome))
        if outcome.is_error && outcome.error.as_deref() == Some("cancelled")
            && outcome.usage.output_tokens == 17));
}

#[tokio::test]
async fn interrupt_support_does_not_keep_stdin_open() {
    let (_cancel, mut run) = fixture("while read -r line; do :; done; echo receipt").await;
    drop(run.stdin.take());
    let event = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
        .await
        .expect("stdin EOF reached fixture")
        .expect("receipt event");
    assert!(matches!(event, EngineEvent::Done(outcome) if outcome.usage.output_tokens == 17));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), run.events.recv())
            .await
            .expect("normal process exit")
            .is_none()
    );
}

#[tokio::test]
async fn oversized_unterminated_jsonl_fails_and_reaps_the_process() {
    let (_, mut run) =
        fixture("read -r prompt; head -c 16777217 /dev/zero | tr '\\0' x; exec sleep 30").await;
    let event = tokio::time::timeout(Duration::from_secs(20), run.events.recv())
        .await
        .expect("oversized line must fail promptly")
        .expect("failure event");
    assert!(matches!(event, EngineEvent::Done(outcome)
        if outcome.is_error && outcome.error.as_ref().unwrap().contains("JSONL record exceeds")));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), run.events.recv())
            .await
            .expect("process reaped")
            .is_none()
    );
}

#[tokio::test]
async fn stdout_read_errors_after_a_receipt_keep_the_success_and_reap() {
    for output in [
        "printf '\\377\\n'",
        "head -c 16777217 /dev/zero | tr '\\0' x",
    ] {
        let script = format!("read -r prompt; echo receipt; {output}; exec sleep 30");
        let (_cancel, mut run) = fixture(&script).await;
        let receipt = run.events.recv().await.expect("receipt");
        assert!(matches!(receipt, EngineEvent::Done(outcome) if !outcome.is_error));
        let note = tokio::time::timeout(Duration::from_secs(20), run.events.recv())
            .await
            .expect("stdout error timeout")
            .expect("diagnostic");
        assert!(matches!(note, EngineEvent::Note(note) if note.contains("stdout read error")));
        assert!(
            tokio::time::timeout(Duration::from_secs(5), run.events.recv())
                .await
                .expect("reaped process")
                .is_none()
        );
    }
}
