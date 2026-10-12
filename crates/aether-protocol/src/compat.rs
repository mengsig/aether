//! Serving the previous protocol version.
//!
//! This build speaks [`PROTOCOL_VERSION`] and also serves
//! [`MIN_PROTOCOL_VERSION`], one back, by rewriting the frames whose shape
//! changed. A client inside that window is translated at the edge, never
//! refused; one outside it is refused at hello with both bounds named.
//!
//! Only the daemon's answers need rewriting. Every field a newer version
//! added to a command carries `#[serde(default)]`, so an older client's
//! request is a strict subset of the current one and decodes unchanged -
//! `no_command_requires_a_field_an_older_client_would_not_send` holds that
//! true against the live schema.
//!
//! v18 adds images on a message. Stored state then carries `image_ref`
//! content blocks, which a v17 client cannot decode: it would lose the whole
//! event or transcript. Each block is rewritten into the line of text a
//! newer client prints for it, so the message still reads as sent.
//!
//! v17 adds the watcher's `operator_users`. A v16 client cannot represent
//! the list, and saving a configuration without it would silently take
//! operator standing away from every listed login, so a watcher reply
//! carrying it becomes an explicit compatibility error. Existing v16 replies
//! and events remain unchanged; the `operator` flag on an external trigger is
//! absent unless set, and a v16 client ignores it.
//!
//! v16 adds retention preview requests. v15 replies need no rewriting.
//! Previously, v15 added per-repository watcher allowlists. A v14 client cannot represent
//! one, and saving a configuration without it would silently widen who may
//! dispatch, so a watcher reply carrying one becomes an explicit
//! compatibility error. Existing v14 replies and events remain unchanged.

use crate::{ErrorCode, ErrorReply, Hello, PROTOCOL_VERSION, Reply, ServerFrame};
use aether_core::github::GithubWatcherConfig;
use aether_core::model::ContentBlock;
use serde_json::Value;

pub const ALLOWLIST_WATCHER_VERSION_ERROR: &str =
    "GitHub watcher allowed_users requires protocol 15; upgrade the Aether client";

pub const OPERATOR_USERS_WATCHER_VERSION_ERROR: &str =
    "GitHub watcher operator_users requires protocol 17; upgrade the Aether client";

/// Whether any repository narrows its requesters: the shape a v14 peer
/// cannot read or preserve.
pub fn has_watcher_allowlist(config: &GithubWatcherConfig) -> bool {
    config
        .repositories
        .iter()
        .any(|repo| repo.allowed_users.is_some())
}

/// Whether the watcher names operator logins: the shape a v16 peer cannot
/// read or preserve.
pub fn has_watcher_operator_users(config: &GithubWatcherConfig) -> bool {
    config.operator_users.is_some()
}

/// Oldest protocol version this build still serves. Saturating, and never
/// below 1: version 0 was never a thing to serve.
pub const MIN_PROTOCOL_VERSION: u32 = {
    let one_back = PROTOCOL_VERSION.saturating_sub(1);
    if one_back < 1 { 1 } else { one_back }
};

/// The version a connection will speak, or why the client is refused.
pub fn negotiate(hello: &Hello) -> Result<u32, ErrorReply> {
    let asked = hello.protocol_version;
    if (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&asked) {
        return Ok(asked);
    }
    Err(ErrorReply::new(
        ErrorCode::Incompatible,
        format!(
            "client protocol {asked} is not served: this daemon speaks \
             {MIN_PROTOCOL_VERSION} to {PROTOCOL_VERSION}"
        ),
    ))
}

/// Encode one server frame as a client speaking `version` reads it. Every
/// shape this build changed for an older client in the window is rewritten
/// here, keyed on `version`, so the daemon's writer never has to know which.
///
pub fn encode_for(version: u32, frame: &ServerFrame) -> Result<String, serde_json::Error> {
    debug_assert!(
        (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version),
        "a connection only ever speaks a negotiated version, got {version}"
    );
    if version < 15
        && let ServerFrame::Reply {
            id,
            result: Ok(Reply::GithubWatcher(status)),
        } = frame
        && has_watcher_allowlist(&status.config)
    {
        return crate::encode(&ServerFrame::Reply {
            id: *id,
            result: Err(ErrorReply::new(
                ErrorCode::Incompatible,
                ALLOWLIST_WATCHER_VERSION_ERROR,
            )),
        });
    }
    if version < 17
        && let ServerFrame::Reply {
            id,
            result: Ok(Reply::GithubWatcher(status)),
        } = frame
        && has_watcher_operator_users(&status.config)
    {
        return crate::encode(&ServerFrame::Reply {
            id: *id,
            result: Err(ErrorReply::new(
                ErrorCode::Incompatible,
                OPERATOR_USERS_WATCHER_VERSION_ERROR,
            )),
        });
    }
    if version < 18 {
        return encode_without_image_refs(frame);
    }
    crate::encode(frame)
}

/// The wire tag of a stored image's content block.
const IMAGE_REF_TAG: &str = "image_ref";

/// Encode a frame for a client that predates images on a message. A frame
/// without one is encoded exactly as it always was.
fn encode_without_image_refs(frame: &ServerFrame) -> Result<String, serde_json::Error> {
    let line = crate::encode(frame)?;
    if !line.contains(IMAGE_REF_TAG) {
        return Ok(line);
    }
    let mut value: Value = serde_json::from_str(&line)?;
    replace_image_refs(&mut value);
    serde_json::to_string(&value)
}

/// Turn every `image_ref` block under `value` into a text block. Depth is
/// bounded: the value was parsed, and the parser stops at 128 levels.
fn replace_image_refs(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(replace_image_refs),
        Value::Object(fields) => {
            let tagged = fields.get("type").and_then(Value::as_str) == Some(IMAGE_REF_TAG);
            if tagged && let Some(text) = image_ref_text(value) {
                *value = text;
                return;
            }
            if let Value::Object(fields) = value {
                fields.values_mut().for_each(replace_image_refs);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The text block standing in for `value`, when it is a stored image's block
/// and not arbitrary JSON (a tool's input) that happens to carry the tag.
fn image_ref_text(value: &Value) -> Option<Value> {
    match serde_json::from_value::<ContentBlock>(value.clone()) {
        Ok(ContentBlock::ImageRef(image)) => {
            serde_json::to_value(ContentBlock::text(image.label())).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Reply, Welcome};
    use aether_core::agent::{Agent, AgentStatus, BlockReason};
    use aether_core::github::{GithubRepositoryConfig, GithubWatcherStatus};
    use aether_core::inbox::{InboxItem, InboxKind};

    #[test]
    fn v15_watcher_allowlists_are_preserved_in_the_served_window() {
        let mut status = GithubWatcherStatus::default();
        let mut repository = GithubRepositoryConfig::automatic("o/r".into());
        repository.allowed_users = Some(vec!["octocat".into()]);
        status.config.repositories.push(repository);
        let frame = ServerFrame::Reply {
            id: 7,
            result: Ok(Reply::GithubWatcher(status)),
        };
        for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
            assert_eq!(
                crate::decode::<ServerFrame>(&encode_for(version, &frame).unwrap()).unwrap(),
                frame
            );
        }
        let mut old = Hello::new("v14", "test");
        old.protocol_version = 14;
        assert!(negotiate(&old).is_err());
    }

    #[test]
    fn v17_watcher_operator_users_are_preserved_in_the_served_window() {
        let mut status = GithubWatcherStatus::default();
        status.config.operator_users = Some(vec!["octocat".into()]);
        let frame = ServerFrame::Reply {
            id: 7,
            result: Ok(Reply::GithubWatcher(status)),
        };
        for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
            assert_eq!(
                crate::decode::<ServerFrame>(&encode_for(version, &frame).unwrap()).unwrap(),
                frame
            );
        }
        let mut old = Hello::new("v16", "test");
        old.protocol_version = 16;
        assert!(negotiate(&old).is_err());
    }

    /// The trigger's `operator` flag is additive: unset it is not written, so
    /// a v16 client sees the shape it knows; set, it rides along and a v16
    /// client that ignores it reads an ordinary external trigger.
    #[test]
    fn a_triggers_operator_flag_is_additive_for_both_versions() {
        let item = |operator: bool| InboxItem {
            id: aether_core::id::InboxItemId::new(),
            agent_id: aether_core::id::AgentId::new(),
            seq: 1,
            kind: InboxKind::ExternalTrigger {
                source: "github".into(),
                payload: serde_json::json!({"text": "@aether"}),
                operator,
            },
            delivered_at: "2026-09-09T11:00:00Z".parse().expect("timestamp"),
        };
        for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
            let external = reply(Reply::Inbox(vec![item(false)]));
            let encoded = encode_for(version, &external).unwrap();
            assert!(!encoded.contains("\"operator\""), "{encoded}");
            assert_eq!(crate::decode::<ServerFrame>(&encoded).unwrap(), external);
            let granted = reply(Reply::Inbox(vec![item(true)]));
            let encoded = encode_for(version, &granted).unwrap();
            assert!(encoded.contains("\"operator\":true"), "{encoded}");
            assert_eq!(crate::decode::<ServerFrame>(&encoded).unwrap(), granted);
        }
    }
    /// Every field a version adds to a command carries `#[serde(default)]`,
    /// so an older client's frame stays a strict subset of the current one
    /// and decodes unchanged. That rule is stated in this module's doc and in
    /// ARCHITECTURE, and until now it was tested by one hand-written frame.
    ///
    /// This pins it mechanically: the set of fields each command *requires*.
    /// A new field without a default lands here as a new member and fails,
    /// because that is the frame an N-1 client would be refused on. When a
    /// command legitimately gains a required field, `PROTOCOL_VERSION` moves
    /// with it and this table is updated deliberately. New command tags are
    /// additive: older clients never send them, so pin their fields here
    /// without changing the requirements of existing commands.
    #[test]
    fn no_command_requires_a_field_an_older_client_would_not_send() {
        // (cmd tag, the fields the wire requires beyond `cmd` itself)
        const REQUIRED: &[(&str, &[&str])] = &[
            ("add_approval_rule", &["decision", "match", "scope"]),
            ("archive", &["target"]),
            ("auth_status", &[]),
            ("cancel", &["agent"]),
            ("compact", &["agent"]),
            ("configure_github_watcher", &["config", "expected_revision"]),
            ("create_agent", &["role"]),
            ("create_memory", &["spec"]),
            ("create_schedule", &["spec"]),
            ("create_task", &["objective"]),
            ("engine_tool", &["agent", "input", "name", "token"]),
            ("engine_tools", &["agent", "token"]),
            ("export_run", &["path", "run"]),
            ("forget_memory", &["id"]),
            ("get_agent", &["id"]),
            ("get_artifact", &["id"]),
            ("get_checkpoint", &["id"]),
            ("get_events", &["after", "limit", "query"]),
            ("get_github_watcher", &[]),
            ("get_inbox", &["after", "agent"]),
            ("get_invocation", &["id"]),
            ("get_run_report", &["run"]),
            ("get_schedule", &["id"]),
            ("get_snapshot", &["id"]),
            ("get_task", &["id"]),
            ("get_transcript", &["after", "agent"]),
            ("get_tree", &[]),
            (
                "hello",
                &["client_name", "client_version", "protocol_version"],
            ),
            ("import_run", &["path"]),
            ("interrupt", &["agent"]),
            ("latest_checkpoint", &["agent"]),
            ("list_agents", &["query"]),
            ("list_approval_rules", &[]),
            ("list_approvals", &[]),
            ("list_artifacts", &["query"]),
            ("list_invocations", &["agent"]),
            ("list_models", &[]),
            ("list_runs", &["limit"]),
            ("list_schedules", &[]),
            ("list_tasks", &["query"]),
            ("mcp_servers", &[]),
            ("memories", &["query"]),
            ("pause", &["agent"]),
            ("pin_memory", &["id", "pinned"]),
            ("ping", &[]),
            ("preview_retention", &[]),
            ("providers", &[]),
            (
                "publish_artifact",
                &["content_type", "name", "summary", "task", "text"],
            ),
            ("read_artifact", &["id", "max_bytes"]),
            ("replay", &["agent"]),
            ("reply_to_question", &["content", "question_seq", "to"]),
            ("resolve_approval", &["decision", "id"]),
            ("restart", &["agent"]),
            ("restore_memory", &["id"]),
            ("resume", &["agent"]),
            ("revoke_approval_rule", &["id"]),
            ("schedule_history", &["id", "limit"]),
            ("send_message", &["content", "to"]),
            ("set_approvals", &["agent", "policy"]),
            ("shutdown", &[]),
            ("status", &[]),
            ("subscribe", &["deltas", "query"]),
            ("sync_repository_knowledge", &[]),
            ("trigger", &["agent", "source"]),
            ("unsubscribe", &["id"]),
            ("update_agent", &["id", "patch"]),
            ("update_schedule", &["action", "id"]),
            ("wake", &["agent"]),
        ];

        let live = command_requirements();
        let pinned: std::collections::BTreeMap<String, Vec<String>> = REQUIRED
            .iter()
            .map(|(tag, fields)| {
                (
                    (*tag).to_owned(),
                    fields.iter().map(|f| (*f).to_owned()).collect(),
                )
            })
            .collect();
        assert_eq!(
            live, pinned,
            "a command's required fields changed. A field a newer version adds must carry \
             #[serde(default)] so an N-1 client's frame still decodes; if it genuinely cannot, \
             bump PROTOCOL_VERSION and update this table with it."
        );
    }

    /// Each command tag and the fields the schema says its frame must carry,
    /// with the `cmd` discriminant itself removed. Mirrors what
    /// `scripts/generate_clients.py` does to build the command builders, so
    /// the gate and the generated clients read the schema the same way.
    fn command_requirements() -> std::collections::BTreeMap<String, Vec<String>> {
        let schema = crate::schema::schema();
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .expect("the schema defines its types");
        let variants = defs
            .get("ClientFrame")
            .and_then(|frame| frame.get("oneOf"))
            .and_then(Value::as_array)
            .expect("ClientFrame is a union of command variants");
        let mut out = std::collections::BTreeMap::new();
        for variant in variants {
            let tag = variant
                .pointer("/properties/cmd/const")
                .and_then(Value::as_str)
                .expect("every command variant is tagged")
                .to_owned();
            let mut required: Vec<String> = required_of(variant);
            // A variant that flattens a struct in requires that struct's
            // fields too.
            if let Some(name) = variant.get("$ref").and_then(Value::as_str)
                && let Some(target) = defs.get(name.rsplit('/').next().unwrap_or(name))
            {
                required.extend(required_of(target));
            }
            required.retain(|field| field != "cmd");
            required.sort();
            required.dedup();
            out.insert(tag, required);
        }
        out
    }

    fn required_of(schema: &Value) -> Vec<String> {
        schema
            .get("required")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_window_is_this_version_and_one_back() {
        let mut hello = Hello::new("t", "0");
        assert_eq!(negotiate(&hello), Ok(PROTOCOL_VERSION));
        hello.protocol_version = MIN_PROTOCOL_VERSION;
        assert_eq!(negotiate(&hello), Ok(MIN_PROTOCOL_VERSION));
    }

    #[test]
    fn a_newer_or_older_client_is_refused_with_both_bounds() {
        // `saturating_sub`: MIN_PROTOCOL_VERSION is itself floored at 1, so a
        // plain `- 1` would underflow if the window ever starts there. When
        // it does, the low case collapses onto MIN_PROTOCOL_VERSION itself -
        // inside the window, not a case this test is about - so it is
        // skipped rather than asserted as a refusal.
        let too_low = MIN_PROTOCOL_VERSION.saturating_sub(1);
        let cases = [too_low, PROTOCOL_VERSION + 1]
            .into_iter()
            .filter(|&asked| asked != MIN_PROTOCOL_VERSION);
        for asked in cases {
            let mut hello = Hello::new("t", "0");
            hello.protocol_version = asked;
            let err = negotiate(&hello).expect_err("outside the window");
            assert_eq!(err.code, ErrorCode::Incompatible);
            assert!(
                err.message.contains(&MIN_PROTOCOL_VERSION.to_string())
                    && err.message.contains(&PROTOCOL_VERSION.to_string()),
                "the error must name the window: {}",
                err.message
            );
        }
    }

    /// Frames the rewrite must not touch: an ordinary error and two replies
    /// reach an N-1 client as they are.
    #[test]
    fn ordinary_frames_are_unchanged_for_both_versions() {
        for frame in [
            ServerFrame::Reply {
                id: 2,
                result: Err(ErrorReply::not_found("agent")),
            },
            ServerFrame::Reply {
                id: 3,
                result: Ok(Reply::Subscribed { id: 1, last_seq: 9 }),
            },
            ServerFrame::Reply {
                id: 4,
                result: Ok(Reply::Paused {
                    agents: Vec::new(),
                    mid_turn: Vec::new(),
                }),
            },
        ] {
            // Through a `Value`, so compare the JSON and not the key order:
            // the rewrite path rebuilds the line from a map.
            let untouched: Value =
                serde_json::from_str(&crate::encode(&frame).expect("encode")).expect("json");
            for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
                let translated: Value =
                    serde_json::from_str(&encode_for(version, &frame).expect("encode"))
                        .expect("json");
                assert_eq!(translated, untouched, "v{version}: {frame:?}");
            }
        }
    }

    /// An older client sends no `subtree`; the field's default is what lets
    /// its request through with no rewrite of its own.
    #[test]
    fn an_older_clients_pause_frame_still_decodes() {
        let frame: crate::ClientFrame =
            crate::decode(r#"{"req":4,"cmd":"pause","agent":"agt_01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#)
                .expect("an older pause frame decodes");
        assert!(matches!(
            frame.command,
            crate::Command::Pause { subtree: false, .. }
        ));
    }

    #[test]
    fn authentication_refusals_keep_their_meaning_for_both_versions() {
        let frame = ServerFrame::Reply {
            id: 1,
            result: Err(ErrorReply::new(ErrorCode::Unauthorized, "bad token")),
        };
        for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
            let line = encode_for(version, &frame).expect("encode");
            let back: ServerFrame = crate::decode(&line).expect("decode");
            assert_eq!(back, frame);
        }
    }

    /// An older client sends no `idempotency_key`; the field's default is
    /// what lets its frame through with no rewrite of its own.
    #[test]
    fn an_older_clients_creating_frames_still_decode() {
        fn command(line: &str) -> crate::Command {
            let agent = "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV";
            crate::decode::<crate::ClientFrame>(&line.replace("AGENT", agent))
                .expect("an older frame decodes")
                .command
        }

        assert!(matches!(
            command(r#"{"req":1,"cmd":"send_message","to":"AGENT","content":"hi"}"#),
            crate::Command::SendMessage {
                idempotency_key: None,
                ..
            }
        ));
        assert!(matches!(
            command(r#"{"req":2,"cmd":"create_agent","role":"worker"}"#),
            crate::Command::CreateAgent(req) if req.idempotency_key.is_none()
        ));
        assert!(matches!(
            command(r#"{"req":3,"cmd":"create_task","objective":"ship"}"#),
            crate::Command::CreateTask(req) if req.idempotency_key.is_none()
        ));
        assert!(matches!(
            command(r#"{"req":4,"cmd":"trigger","agent":"AGENT","source":"ci"}"#),
            crate::Command::Trigger {
                idempotency_key: None,
                ..
            }
        ));
    }

    /// The key rides on the frame as a plain optional field, so a v8 client's
    /// request is the older one plus that field and nothing else.
    #[test]
    fn a_keyed_frame_carries_the_key_and_nothing_else_moves() {
        let line = r#"{"req":1,"cmd":"send_message","to":"agt_01ARZ3NDEKTSV4RRFFQ69G5FAV","content":"hi","idempotency_key":"outbox-7"}"#;
        let frame: crate::ClientFrame = crate::decode(line).expect("decode");
        let crate::Command::SendMessage {
            content,
            subtree,
            idempotency_key,
            ..
        } = &frame.command
        else {
            panic!("send_message");
        };
        assert_eq!(content, "hi");
        assert!(!subtree);
        assert_eq!(idempotency_key.as_deref(), Some("outbox-7"));
    }

    /// The handshake reports the version in effect and the window around it,
    /// so a client can see it was translated rather than matched.
    #[test]
    fn the_welcome_reports_the_window() {
        let welcome = Welcome {
            protocol_version: MIN_PROTOCOL_VERSION,
            daemon_version: "t".into(),
            instance_id: "i".into(),
            schema_version: 1,
            pid: 1,
            features: Vec::new(),
            daemon_protocol_version: Some(PROTOCOL_VERSION),
            min_protocol_version: Some(MIN_PROTOCOL_VERSION),
            idle_timeout_secs: None,
        };
        let json = serde_json::to_value(&welcome).expect("ser");
        assert_eq!(json["protocol_version"], MIN_PROTOCOL_VERSION);
        assert_eq!(json["daemon_protocol_version"], PROTOCOL_VERSION);
        assert_eq!(json["min_protocol_version"], MIN_PROTOCOL_VERSION);
        // An older daemon sends neither; they must not be required.
        let bare = r#"{"protocol_version":1,"daemon_version":"t","instance_id":"i","schema_version":1,"pid":1}"#;
        let back: Welcome = serde_json::from_str(bare).expect("de");
        assert_eq!(back.daemon_protocol_version, None);
    }

    fn reply(result: Reply) -> ServerFrame {
        ServerFrame::Reply {
            id: 7,
            result: Ok(result),
        }
    }

    fn exhausted() -> BlockReason {
        BlockReason::Exhausted {
            model: aether_core::model::ModelRef::new("anthropic", "opus"),
            class: aether_core::agent::ExhaustionClass::UsageLimit,
            since: "2026-09-09T11:00:00Z".parse().expect("timestamp"),
            reported_reset: None,
            next_probe: "2026-09-09T12:00:00Z".parse().expect("timestamp"),
            probes: 1,
            detail: "5-hour limit reached".into(),
        }
    }

    fn suspended_agent() -> Agent {
        let mut agent: Agent = serde_json::from_value(serde_json::json!({
            "id": "agt_01ARZ3NDEKTSV4RRFFQ69G5FAV", "role": "worker", "name": "w",
            "scope": {}, "model": {"provider": "anthropic", "model": "opus"},
            "status": {"state": "ready"}, "config": {}, "stats": {},
            "run": "run_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "created_at": "2026-09-09T11:00:00Z", "updated_at": "2026-09-09T11:00:00Z"
        }))
        .expect("an agent row");
        agent.status = AgentStatus::Blocked {
            reason: exhausted(),
        };
        agent
    }

    fn suspended_item() -> InboxItem {
        InboxItem {
            id: aether_core::id::InboxItemId::new(),
            agent_id: aether_core::id::AgentId::new(),
            seq: 3,
            kind: InboxKind::ChildSuspended {
                agent: aether_core::id::AgentId::new(),
                detail: "usage limit".into(),
            },
            delivered_at: "2026-09-09T11:00:00Z".parse().expect("timestamp"),
        }
    }

    #[test]
    fn status_since_is_optional_in_both_served_protocol_versions() {
        for since in [None, Some("2026-09-14T10:00:00Z".parse().unwrap())] {
            let mut agent = suspended_agent();
            agent.status_since = since;
            let frame = reply(Reply::Agent(agent));
            for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
                let encoded = encode_for(version, &frame).unwrap();
                assert_eq!(encoded.contains("status_since"), since.is_some());
                assert_eq!(crate::decode::<ServerFrame>(&encoded).unwrap(), frame);
            }
        }
        let schema = schemars::schema_for!(Agent);
        let value = serde_json::to_value(schema).unwrap();
        assert!(value["properties"].get("status_since").is_some());
        assert!(
            !value["required"]
                .as_array()
                .unwrap()
                .contains(&Value::String("status_since".into()))
        );
        assert_eq!((MIN_PROTOCOL_VERSION, PROTOCOL_VERSION), (17, 18));
    }

    #[test]
    fn served_versions_keep_existing_exhaustion_shapes_unchanged() {
        let frames = [
            reply(Reply::Agent(suspended_agent())),
            reply(Reply::Inbox(vec![suspended_item()])),
        ];
        for frame in frames {
            for version in [MIN_PROTOCOL_VERSION, PROTOCOL_VERSION] {
                let encoded = encode_for(version, &frame).expect("encode");
                let decoded: ServerFrame = crate::decode(&encoded).expect("decode");
                assert_eq!(decoded, frame);
            }
        }
        assert_eq!(MIN_PROTOCOL_VERSION, 17);
        let mut hello = Hello::new("v16", "test");
        hello.protocol_version = 16;
        assert!(negotiate(&hello).is_err());
    }

    fn stored_image() -> aether_core::attachment::AttachmentRef {
        aether_core::attachment::AttachmentRef {
            id: aether_core::attachment::AttachmentId::of(b"png bytes"),
            media_type: "image/png".into(),
            size_bytes: 1_258_291,
            name: Some("shot.png".into()),
        }
    }

    fn message_with_image() -> ServerFrame {
        reply(Reply::Inbox(vec![InboxItem {
            id: aether_core::id::InboxItemId::new(),
            agent_id: aether_core::id::AgentId::new(),
            seq: 4,
            kind: InboxKind::Message {
                message_id: aether_core::id::MessageId::new(),
                from: aether_core::event::Actor::user(),
                content: vec![
                    ContentBlock::text("what is this?"),
                    ContentBlock::ImageRef(stored_image()),
                ],
            },
            delivered_at: "2026-10-12T11:00:00Z".parse().expect("timestamp"),
        }]))
    }

    /// A v17 client cannot decode `image_ref`. It reads the line a newer
    /// client prints instead of losing the whole reply.
    #[test]
    fn a_v17_client_reads_a_stored_image_as_its_line_of_text() {
        let frame = message_with_image();
        let current = encode_for(18, &frame).expect("encode");
        assert_eq!(
            crate::decode::<ServerFrame>(&current).expect("decode"),
            frame
        );
        assert!(current.contains("\"image_ref\""), "{current}");

        let old = encode_for(17, &frame).expect("encode");
        assert!(!old.contains("image_ref"), "{old}");
        let ServerFrame::Reply {
            id: 7,
            result: Ok(Reply::Inbox(items)),
        } = crate::decode(&old).expect("a v17 client decodes it")
        else {
            panic!("the reply keeps its shape: {old}");
        };
        let InboxKind::Message { content, .. } = &items[0].kind else {
            panic!("still a message");
        };
        assert_eq!(
            content,
            &[
                ContentBlock::text("what is this?"),
                ContentBlock::text("[image: shot.png, 1.2 MB]"),
            ]
        );
    }

    /// Only a real stored-image block is rewritten: JSON that merely carries
    /// the tag (a tool's input) reaches a v17 client untouched.
    #[test]
    fn json_that_only_looks_like_a_stored_image_is_left_alone() {
        let lookalike = serde_json::json!({"type": "image_ref", "note": "not a block"});
        let mut value =
            serde_json::json!({"input": lookalike.clone(), "list": [lookalike.clone()]});
        replace_image_refs(&mut value);
        assert_eq!(value["input"], lookalike);
        assert_eq!(value["list"][0], lookalike);

        let mut real = serde_json::json!({"nested": [serde_json::to_value(
            ContentBlock::ImageRef(stored_image())
        )
        .expect("block")]});
        replace_image_refs(&mut real);
        assert_eq!(
            real["nested"][0],
            serde_json::json!({"type": "text", "text": "[image: shot.png, 1.2 MB]"})
        );
    }
}
