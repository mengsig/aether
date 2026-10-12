//! Images on `send_message`: additive on the wire, part of the retry digest.
use super::*;

fn screenshot(data: &str) -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: data.into(),
        name: Some("shot.png".into()),
    }
}

fn send(attachments: Vec<ImageAttachment>, key: Option<&str>) -> Command {
    Command::SendMessage {
        to: "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV".parse().expect("agent id"),
        content: "what is this?".into(),
        subtree: false,
        attachments,
        idempotency_key: key.map(str::to_owned),
    }
}

/// A message without images is on the wire what it was before the field
/// existed, and a frame written before it still decodes.
#[test]
fn a_text_only_message_keeps_its_wire_shape_and_an_old_frame_still_parses() {
    let line = serde_json::to_string(&send(Vec::new(), None)).expect("ser");
    assert_eq!(
        line,
        r#"{"cmd":"send_message","to":"agt_01ARZ3NDEKTSV4RRFFQ69G5FAV","content":"what is this?","subtree":false}"#
    );
    let old =
        r#"{"cmd":"send_message","to":"agt_01ARZ3NDEKTSV4RRFFQ69G5FAV","content":"what is this?"}"#;
    assert_eq!(
        serde_json::from_str::<Command>(old).expect("an old frame decodes"),
        send(Vec::new(), None)
    );
}

#[test]
fn a_message_with_images_round_trips_with_its_type_bytes_and_name() {
    let command = send(
        vec![
            screenshot("iVBORw0KGgo="),
            ImageAttachment {
                media_type: "image/jpeg".into(),
                data: "/9j/4AAQ".into(),
                name: None,
            },
        ],
        None,
    );
    let wire = serde_json::to_value(&command).expect("ser");
    assert_eq!(
        wire["attachments"],
        serde_json::json!([
            {"media_type": "image/png", "data": "iVBORw0KGgo=", "name": "shot.png"},
            {"media_type": "image/jpeg", "data": "/9j/4AAQ"},
        ])
    );
    assert_eq!(
        serde_json::from_value::<Command>(wire).expect("de"),
        command
    );
}

/// Megabytes of base64 must never reach a log line through `{:?}`.
#[test]
fn debug_output_names_an_image_without_its_payload() {
    let shown = format!("{:?}", send(vec![screenshot("iVBORw0KGgo=")], None));
    assert!(shown.contains("shot.png"), "{shown}");
    assert!(!shown.contains("iVBORw0KGgo="), "{shown}");
}

/// A retry must repeat the images: the same key with another image, or with
/// none, is different work. A text-only message keeps the digest it had.
#[test]
fn images_are_part_of_the_retry_fingerprint() {
    let fields = |command: Command| {
        command
            .keyed_request()
            .expect("carries a key")
            .expect("view")
            .fields
    };
    let with_image = fields(send(vec![screenshot("iVBORw0KGgo=")], Some("k-1")));
    assert_eq!(
        with_image,
        fields(send(vec![screenshot("iVBORw0KGgo=")], Some("k-1")))
    );
    assert_ne!(
        with_image,
        fields(send(vec![screenshot("R0lGODlhAQAB")], Some("k-1"))),
        "other bytes are other work"
    );
    let image = &with_image["attachments"][0];
    assert_eq!(image["media_type"], "image/png");
    assert_eq!(image["name"], "shot.png");
    assert_eq!(image["sha256"].as_str().map(str::len), Some(64));
    assert!(
        !with_image.to_string().contains("iVBORw0KGgo="),
        "the digest view carries a digest, not the payload"
    );

    assert_eq!(
        fields(send(Vec::new(), Some("k-1"))),
        serde_json::json!({
            "cmd": "send_message",
            "to": "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "content": "what is this?",
            "subtree": false,
        }),
        "a text-only view did not move, so earlier digests still match"
    );
}
