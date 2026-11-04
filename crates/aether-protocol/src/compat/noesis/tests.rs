use super::*;
use crate::compat::encode_for;
use crate::noesis::{JobStats, Phase};
use aether_core::event::{Actor, EventDraft};
use aether_core::id::EventId;
use serde_json::json;

fn event(kind: EventKind) -> Event {
    Event::from_draft(
        EventDraft::new(Actor::System, kind),
        EventId::new(),
        4,
        aether_core::time::now(),
    )
}

fn phase_changed() -> EventKind {
    EventKind::NoesisPhaseChanged {
        session: "ns_1".into(),
        from: Phase::Probe,
        to: Phase::Plan,
    }
}

fn merged() -> EventKind {
    EventKind::NoesisGraphMerged {
        job: "j_1".into(),
        stats: JobStats {
            nodes: 3,
            chains: 2,
            dropped_chains: 0,
        },
    }
}

fn frame(kind: EventKind) -> ServerFrame {
    ServerFrame::Event {
        sub: 7,
        event: event(kind),
    }
}

fn decoded_kind(line: &str) -> EventKind {
    match crate::decode::<ServerFrame>(line).expect("a frame this build decodes") {
        ServerFrame::Event { event, .. } => event.kind,
        other => panic!("not an event frame: {other:?}"),
    }
}

#[test]
fn a_noesis_event_becomes_a_custom_event_named_by_its_dotted_type() {
    let mut withheld = frame(phase_changed());
    assert!(withhold_noesis_events(&mut withheld));
    let ServerFrame::Event { event, .. } = withheld else {
        panic!("still an event frame");
    };
    assert_eq!(
        event.kind,
        EventKind::Custom {
            name: "noesis.phase_changed".into(),
            payload: json!({"session": "ns_1", "from": "probe", "to": "plan"}),
        }
    );
    assert_eq!(event.seq, 4, "only the kind changes");
}

#[test]
fn every_noesis_event_survives_the_rewrite_with_its_fields() {
    let mut withheld = frame(merged());
    assert!(withhold_noesis_events(&mut withheld));
    let ServerFrame::Event { event, .. } = withheld else {
        panic!("still an event frame");
    };
    let EventKind::Custom { name, payload } = event.kind else {
        panic!("custom");
    };
    assert_eq!(name, "noesis.graph_merged");
    assert_eq!(payload["stats"]["nodes"], 3);
    assert_eq!(payload["job"], "j_1");
}

#[test]
fn a_protocol_25_client_and_a_client_without_the_feature_read_custom() {
    let original = frame(phase_changed());
    let v25 = encode_for(25, &original).expect("encode");
    assert!(
        matches!(decoded_kind(&v25), EventKind::Custom { name, .. } if name == "noesis.phase_changed")
    );

    let mut featureless = original.clone();
    assert!(withhold_noesis_events(&mut featureless));
    let line = encode_for(26, &featureless).expect("encode");
    assert_eq!(line, v25, "both paths write the same line");
    assert!(matches!(decoded_kind(&line), EventKind::Custom { .. }));
}

#[test]
fn a_client_with_the_feature_reads_the_typed_event() {
    let original = frame(phase_changed());
    let line = encode_for(26, &original).expect("encode");
    assert_eq!(decoded_kind(&line), phase_changed());
}

#[test]
fn other_frames_are_untouched_and_a_v25_line_is_byte_for_byte_as_before() {
    let mut other = frame(EventKind::AgentReady);
    let before = other.clone();
    assert!(!withhold_noesis_events(&mut other));
    assert_eq!(other, before);
    assert_eq!(
        encode_for(25, &before).expect("encode"),
        crate::encode(&before).expect("encode")
    );
    let mut reply = ServerFrame::Reply {
        id: 3,
        result: Ok(Reply::Ok),
    };
    assert!(!withhold_noesis_events(&mut reply));
}

#[test]
fn an_events_reply_rewrites_only_its_noesis_entries() {
    let mut reply = ServerFrame::Reply {
        id: 5,
        result: Ok(Reply::Events(vec![
            event(EventKind::AgentReady),
            event(phase_changed()),
            event(merged()),
        ])),
    };
    assert!(withhold_noesis_events(&mut reply));
    let ServerFrame::Reply {
        result: Ok(Reply::Events(events)),
        ..
    } = reply
    else {
        panic!("still an events reply");
    };
    let kinds: Vec<_> = events.iter().map(|event| event.kind.type_name()).collect();
    assert_eq!(kinds, ["agent.ready", "custom", "custom"]);
    assert_eq!(events[0].kind, EventKind::AgentReady);

    let line = encode_for(
        25,
        &ServerFrame::Reply {
            id: 5,
            result: Ok(Reply::Events(vec![event(phase_changed())])),
        },
    )
    .expect("encode");
    assert!(line.contains("\"name\":\"noesis.phase_changed\""), "{line}");
}
