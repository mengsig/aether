use super::*;

const AGENT: &str = "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MESSAGE: &str = "msg_01ARZ3NDEKTSV4RRFFQ69G5FAV";
const HORIZON: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV:41";

fn lost_send() -> Command {
    Command::SendMessage {
        to: AGENT.parse().expect("agent id"),
        content: "ship it".into(),
        subtree: false,
        idempotency_key: Some("outbox-7".into()),
    }
}

/// The request is the original keyed command, whole, under `command`: what
/// the web client and the SSH client both write, pinned field for field.
#[test]
fn a_resolution_request_wraps_the_original_command_and_its_horizon() {
    let line = format!(
        r#"{{"req":9,"cmd":"resolve_submission","command":{{"cmd":"send_message","to":"{AGENT}","content":"ship it","subtree":false,"idempotency_key":"outbox-7"}},"horizon":"{HORIZON}"}}"#
    );
    let frame: ClientFrame = decode(&line).expect("decode");
    assert_eq!(
        frame.command,
        Command::ResolveSubmission {
            command: Box::new(lost_send()),
            horizon: Some(HORIZON.into()),
        }
    );
    assert_eq!(encode(&frame).expect("encode").trim_end(), line);
    assert!(
        frame.command.keyed_request().is_none(),
        "asking is not itself a keyed submission"
    );

    let unhorizoned: ClientFrame = decode(&format!(
        r#"{{"req":9,"cmd":"resolve_submission","command":{{"cmd":"send_message","to":"{AGENT}","content":"ship it","idempotency_key":"outbox-7"}}}}"#
    ))
    .expect("the horizon is optional");
    assert!(matches!(
        unhorizoned.command,
        Command::ResolveSubmission { horizon: None, .. }
    ));
}

#[test]
fn the_three_outcomes_encode_as_documented() {
    let executed = Reply::SubmissionResolved(SubmissionResolution::Executed {
        reply: Box::new(Reply::MessageSent {
            message: MESSAGE.parse().expect("message id"),
        }),
        horizon: HORIZON.into(),
    });
    let at: Timestamp = "2026-10-05T08:00:00Z".parse().expect("timestamp");
    let absent = Reply::SubmissionResolved(SubmissionResolution::AbsentSafe {
        horizon: HORIZON.into(),
        retained_since: at,
    });
    let unknown = Reply::SubmissionResolved(SubmissionResolution::Unknown {
        reason: UnprovenAbsence::Forgotten,
        horizon: HORIZON.into(),
        retained_since: at,
    });
    let data = |reply: &Reply| serde_json::to_value(reply).expect("encode")["data"].clone();
    assert_eq!(
        data(&executed),
        serde_json::json!({
            "outcome": "executed",
            "reply": {"reply": "message_sent", "data": {"message": MESSAGE}},
            "horizon": HORIZON,
        })
    );
    assert_eq!(
        data(&absent),
        serde_json::json!({
            "outcome": "absent_safe",
            "horizon": HORIZON,
            "retained_since": "2026-10-05T08:00:00Z",
        })
    );
    assert_eq!(
        data(&unknown),
        serde_json::json!({
            "outcome": "unknown",
            "reason": "forgotten",
            "horizon": HORIZON,
            "retained_since": "2026-10-05T08:00:00Z",
        })
    );
    for reply in [executed, absent, unknown] {
        let tagged = serde_json::to_value(&reply).expect("encode");
        assert_eq!(tagged["reply"], "submission_resolved");
        let frame = ServerFrame::Reply {
            id: 9,
            result: Ok(reply),
        };
        let line = encode(&frame).expect("encode");
        assert_eq!(decode::<ServerFrame>(&line).expect("decode"), frame);
    }
}

#[test]
fn every_reason_an_absence_is_unproven_has_a_stable_name() {
    let names: Vec<String> = [
        UnprovenAbsence::NoHorizon,
        UnprovenAbsence::ForeignHorizon,
        UnprovenAbsence::HorizonAhead,
        UnprovenAbsence::Forgotten,
    ]
    .iter()
    .map(|reason| serde_json::to_value(reason).expect("encode"))
    .map(|name| name.as_str().expect("a string").to_owned())
    .collect();
    assert_eq!(
        names,
        [
            "no_horizon",
            "foreign_horizon",
            "horizon_ahead",
            "forgotten"
        ]
    );
}

/// A welcome from before the ledger has no horizon, and a client reads that
/// as "this host cannot resolve a lost send".
#[test]
fn the_welcome_horizon_is_optional_and_absent_when_unset() {
    let old = serde_json::json!({
        "protocol_version": 17, "daemon_version": "0.1.0", "instance_id": "i",
        "schema_version": 17, "pid": 1,
    });
    let mut welcome: Welcome = serde_json::from_value(old.clone()).expect("decode");
    assert_eq!(welcome.submission_horizon, None);
    let encoded = serde_json::to_value(&welcome).expect("encode");
    assert!(encoded.get("submission_horizon").is_none(), "{encoded}");
    welcome.submission_horizon = Some(HORIZON.into());
    assert_eq!(
        serde_json::to_value(&welcome).expect("encode")["submission_horizon"],
        HORIZON
    );
}
