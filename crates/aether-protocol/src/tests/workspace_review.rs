use super::*;
use aether_core::workspace_review::{PATCH_LIMIT, WorkspaceDiff, cap_patch, truncation_marker};
use serde_json::json;

fn diff(patch: &[u8]) -> WorkspaceDiff {
    let capped = cap_patch(patch, patch.len() as u64);
    WorkspaceDiff {
        agent: AgentId::new(),
        workspace: "/work/child".into(),
        source: Some("/work/repo".into()),
        isolated: true,
        ambiguous: false,
        integrated: false,
        files: Vec::new(),
        files_truncated: false,
        insertions: 0,
        deletions: 0,
        commits: Vec::new(),
        commits_truncated: false,
        patch: capped.text,
        patch_bytes: capped.bytes,
        patch_truncated: capped.truncated,
    }
}

#[test]
fn a_diff_reply_carries_at_most_the_patch_limit_and_a_marker() {
    assert_eq!(
        PATCH_LIMIT,
        256 * 1024,
        "the wire bound is part of the protocol"
    );
    let huge = vec![b'+'; 4 * PATCH_LIMIT];
    let reply = Reply::WorkspaceDiff(Box::new(diff(&huge)));
    let wire = serde_json::to_value(&reply).unwrap();
    assert_eq!(wire["reply"], "workspace_diff");
    let patch = wire["data"]["patch"].as_str().unwrap();
    let marker = truncation_marker(PATCH_LIMIT, huge.len() as u64);
    assert_eq!(patch.len(), PATCH_LIMIT + marker.len());
    assert!(patch.ends_with(&marker));
    assert_eq!(wire["data"]["patch_bytes"], huge.len() as u64);
    assert_eq!(wire["data"]["patch_truncated"], true);
    assert!(encode(&reply).unwrap().len() < PATCH_LIMIT + 1024);
    assert_eq!(serde_json::from_value::<Reply>(wire).unwrap(), reply);

    let small = Reply::WorkspaceDiff(Box::new(diff(b"+one line\n")));
    let wire = serde_json::to_value(&small).unwrap();
    assert_eq!(wire["data"]["patch"], "+one line\n");
    assert_eq!(wire["data"]["patch_truncated"], false);
}

#[test]
fn workspace_commands_need_operate_and_discard_defaults_to_unforced() {
    let agent = AgentId::new();
    let discard: Command =
        serde_json::from_value(json!({"cmd": "workspace_discard", "agent": agent})).unwrap();
    assert_eq!(
        discard,
        Command::WorkspaceDiscard {
            agent,
            force: false
        }
    );
    for command in [
        Command::WorkspaceDiff { agent },
        Command::WorkspaceIntegrate { agent },
        discard,
    ] {
        assert_eq!(command.access(), Access::Operate, "{}", command.name());
        assert_eq!(
            command.introduced(),
            Introduced::Feature(features::WORKSPACE_REVIEW)
        );
    }
}
