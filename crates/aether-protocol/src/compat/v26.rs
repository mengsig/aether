//! What protocol 26 added that an older client, or one not listing
//! `features::DELTA_ORDER`, must not see: frames reach it as before, byte for byte.

use crate::{Delta, Reply, ServerFrame};

/// The frame a client before v26 reads, when it differs from `frame`.
pub(super) fn older_frame(version: u32, frame: &ServerFrame) -> Option<ServerFrame> {
    if version >= 26 {
        return None;
    }
    let mut older = frame.clone();
    let changed = withhold_delta_order(&mut older) | withhold_mounts(&mut older);
    changed.then_some(older)
}

/// Drop the delta numbering and tool call id from `frame`, for a client that
/// did not list `features::DELTA_ORDER`. Whether anything was dropped.
pub fn withhold_delta_order(frame: &mut ServerFrame) -> bool {
    let ServerFrame::Delta {
        delta,
        invocation_seq,
        ..
    } = frame
    else {
        return false;
    };
    let numbered = invocation_seq.take().is_some();
    let identified = match delta {
        Delta::ToolInput { tool_use_id, .. } => tool_use_id.take().is_some(),
        _ => false,
    };
    debug_assert!(matches!(
        frame,
        ServerFrame::Delta {
            invocation_seq: None,
            ..
        }
    ));
    numbered || identified
}

/// Remote settings without their mounts, which a client before v26 would
/// drop on its next save were it shown them. Its save keeps them: absent
/// mounts leave the saved ones in place.
fn withhold_mounts(frame: &mut ServerFrame) -> bool {
    let ServerFrame::Reply {
        result: Ok(Reply::Remote(status)),
        ..
    } = frame
    else {
        return false;
    };
    status.settings.mounts.take().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::encode_for;
    use crate::read_models::{UsageBreakdown, UsageRow};
    use crate::remote::{MountKind, RemoteMount, RemoteSettings, RemoteStatus};
    use aether_core::id::{AgentId, InvocationId};

    const AGENT: &str = "agt_00000000000000000000000001";
    const INVOCATION: &str = "inv_00000000000000000000000002";

    fn delta(delta: Delta, invocation_seq: Option<u64>) -> ServerFrame {
        ServerFrame::Delta {
            sub: 3,
            agent: AGENT.parse::<AgentId>().unwrap(),
            invocation: INVOCATION.parse::<InvocationId>().unwrap(),
            delta,
            invocation_seq,
        }
    }

    fn tool_input(tool_use_id: Option<&str>) -> Delta {
        Delta::ToolInput {
            name: "bash".into(),
            partial_json: "{\"cmd\":".into(),
            tool_use_id: tool_use_id.map(str::to_owned),
        }
    }

    fn remote(mounts: Option<Vec<RemoteMount>>) -> ServerFrame {
        ServerFrame::Reply {
            id: 9,
            result: Ok(Reply::Remote(RemoteStatus {
                revision: 4,
                settings: RemoteSettings {
                    enabled: true,
                    mounts,
                    ..RemoteSettings::default()
                },
                address: "127.0.0.1:8377".into(),
                ..RemoteStatus::default()
            })),
        }
    }

    fn archon_mount() -> RemoteMount {
        RemoteMount {
            prefix: "/archon".into(),
            kind: MountKind::Archon,
            target: "http://127.0.0.1:8378".into(),
        }
    }

    /// The lines a protocol 25 daemon wrote for these frames, kept verbatim.
    const V25_TOOL_INPUT: &str = r#"{"frame":"delta","sub":3,"agent":"agt_00000000000000000000000001","invocation":"inv_00000000000000000000000002","delta":{"delta":"tool_input","name":"bash","partial_json":"{\"cmd\":"}}"#;
    const V25_TEXT: &str = r#"{"frame":"delta","sub":3,"agent":"agt_00000000000000000000000001","invocation":"inv_00000000000000000000000002","delta":{"delta":"text","text":"hi"}}"#;
    const V25_REMOTE: &str = r#"{"frame":"reply","id":9,"result":{"Ok":{"reply":"remote","data":{"revision":4,"settings":{"enabled":true,"pairing":false,"public_origins":[],"push":false},"address":"127.0.0.1:8377","allow_remote":false,"devices":0,"connections":0,"push_previews":false}}}}"#;

    fn line(version: u32, frame: &ServerFrame) -> String {
        let line = encode_for(version, frame).unwrap();
        line.trim_end().to_owned()
    }

    #[test]
    fn a_protocol_25_client_reads_every_new_shape_byte_for_byte_as_before() {
        let numbered_tool = delta(tool_input(Some("toolu_1")), Some(7));
        let numbered_text = delta(Delta::Text { text: "hi".into() }, Some(8));
        assert_eq!(line(25, &numbered_tool), V25_TOOL_INPUT);
        assert_eq!(line(25, &numbered_text), V25_TEXT);
        assert_eq!(line(25, &remote(Some(vec![archon_mount()]))), V25_REMOTE);
        // What the daemon builds when nothing new is set is the old line too.
        assert_eq!(line(25, &delta(tool_input(None), None)), V25_TOOL_INPUT);
        assert_eq!(line(25, &remote(None)), V25_REMOTE);
        assert_eq!(line(26, &remote(None)), V25_REMOTE);
    }

    #[test]
    fn a_v26_client_without_delta_order_reads_deltas_as_before() {
        let mut frame = delta(tool_input(Some("toolu_1")), Some(7));
        assert!(withhold_delta_order(&mut frame));
        assert_eq!(line(26, &frame), V25_TOOL_INPUT);
        assert!(
            !withhold_delta_order(&mut frame),
            "nothing left to withhold"
        );
        let mut text = delta(Delta::Text { text: "hi".into() }, Some(8));
        assert!(withhold_delta_order(&mut text));
        assert_eq!(line(26, &text), V25_TEXT);
    }

    #[test]
    fn a_v26_client_with_delta_order_reads_the_numbering_and_tool_id() {
        let frame = delta(tool_input(Some("toolu_1")), Some(7));
        let line = line(26, &frame);
        assert!(line.contains(r#""invocation_seq":7"#), "{line}");
        assert!(line.contains(r#""tool_use_id":"toolu_1""#), "{line}");
        assert_eq!(crate::decode::<ServerFrame>(&line).unwrap(), frame);
    }

    #[test]
    fn a_v26_client_reads_the_mounts_and_new_replies_unchanged() {
        let frame = remote(Some(vec![archon_mount()]));
        let line = line(26, &frame);
        assert!(
            line.contains(r#""mounts":[{"prefix":"/archon","kind":"archon""#),
            "{line}"
        );
        assert_eq!(crate::decode::<ServerFrame>(&line).unwrap(), frame);
        let breakdown = ServerFrame::Reply {
            id: 2,
            result: Ok(Reply::UsageBreakdown(UsageBreakdown {
                rows: vec![UsageRow {
                    key: "anthropic".into(),
                    usage: aether_core::model::Usage::default(),
                    invocations: 1,
                }],
                total: aether_core::model::Usage::default(),
                truncated: false,
            })),
        };
        for version in [25, 26] {
            let encoded = encode_for(version, &breakdown).unwrap();
            assert_eq!(crate::decode::<ServerFrame>(&encoded).unwrap(), breakdown);
        }
    }
}
