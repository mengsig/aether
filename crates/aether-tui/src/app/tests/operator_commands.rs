//! operator commands reducer regressions.
use super::*;

#[test]
fn compaction_notices_acknowledge_only_accepted_requests_and_surface_rejections() {
    let (mut app, id) = app_with_agent();
    let effects = app.run_command(SlashCommand::Compact);
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::Compact { agent }, then: Then::CompactionAccepted(target) }] if *agent == id && *target == id)
    );
    app.on_reply(Then::CompactionAccepted(id), Ok(Reply::Ok));
    assert_eq!(
        app.live_flash().unwrap(),
        aether_core::event::compaction_request_notice(&format!(
            "aether events --agent {id} --follow"
        ))
    );
    app.on_reply(
        Then::CompactionAccepted(id),
        Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "agent has ended",
        ))),
    );
    assert!(app.live_flash().unwrap().contains("agent has ended"));
    assert!(!app.live_flash().unwrap().contains("accepted"));
}

fn use_ascii(app: &mut App) {
    app.theme = crate::ui::theme::tokens::Theme::resolve(crate::ui::theme::tokens::ThemeSpec {
        ascii: true,
        ..app.theme.spec()
    });
}

#[test]
fn operator_commands_wait_for_replies_and_keep_unicode_in_ascii_mode() {
    use aether_core::memory::{MemoryScope, OperatorMemoryScope};
    let (mut app, id) = app_with_agent();
    use_ascii(&mut app);
    let original_name = app.selected_agent().unwrap().name.clone();
    let effects = app.run_command(SlashCommand::Rename("名前 café".into()));
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::UpdateAgent { id: target, patch , idempotency_key: None, }, then: Then::AgentRenamed(reply_id) }] if *target == id && *reply_id == id && patch.name.as_deref() == Some("名前 café"))
    );
    assert_eq!(app.selected_agent().unwrap().name, original_name);
    let mut renamed = app.selected_agent().unwrap().clone();
    renamed.name = "名前 café".into();
    let effects = app.on_reply(Then::AgentRenamed(id), Ok(Reply::Agent(renamed)));
    assert!(app.live_flash().unwrap().contains("名前 café"));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::GetTree { .. },
            ..
        }
    )));
    let effects = app.run_command(SlashCommand::OperatorRemember {
        text: "Keep Unicode 名前 intact.".into(),
        scope: commands::RememberScope::Agent,
    });
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::CreateMemory { spec , idempotency_key: None, }, then: Then::MemoryCreated { selected } }] if *selected == Some(id) && spec.scope == OperatorMemoryScope::Agent { id })
    );
    let mut record = memory_record(false);
    record.scope = MemoryScope::Agent { id };
    record.provenance = Default::default();
    let effects = app.on_reply(
        Then::MemoryCreated { selected: Some(id) },
        Ok(Reply::Memory(record)),
    );
    assert!(app.live_flash().unwrap().contains("operator memory"));
    assert!(app.live_flash().unwrap().is_ascii());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::Memories { .. },
            ..
        }
    )));
}

#[test]
fn operator_commands_reject_missing_selection_and_surface_errors_without_stale_notices() {
    let (mut app, id) = app_with_agent();
    app.selected = None;
    assert!(
        app.run_command(SlashCommand::Rename("name".into()))
            .is_empty()
    );
    assert!(
        app.run_command(SlashCommand::OperatorRemember {
            text: "Keep this durable fact.".into(),
            scope: commands::RememberScope::Agent
        })
        .is_empty()
    );
    let global = app.run_command(SlashCommand::OperatorRemember {
        text: "Keep this durable fact.".into(),
        scope: commands::RememberScope::Global,
    });
    assert!(matches!(
        &global[..],
        [Effect::Request {
            command: Command::CreateMemory { .. },
            ..
        }]
    ));
    app.selected = Some(id);
    assert!(
        app.run_command(SlashCommand::OperatorRemember {
            text: "Keep this durable fact.".into(),
            scope: commands::RememberScope::Repository
        })
        .is_empty()
    );
    let root = std::path::PathBuf::from("/workspace/repository");
    app.tree
        .nodes
        .get_mut(&id)
        .unwrap()
        .agent
        .scope
        .workspace_root = Some(root.clone());
    let repository = app.run_command(SlashCommand::OperatorRemember {
        text: "Keep this durable fact.".into(),
        scope: commands::RememberScope::Repository,
    });
    assert!(
        matches!(&repository[..], [Effect::Request { command: Command::CreateMemory { spec , idempotency_key: None, }, .. }] if spec.scope == aether_core::memory::OperatorMemoryScope::Repository { root: Some(root) })
    );
    app.on_reply(
        Then::MemoryCreated { selected: Some(id) },
        Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "memory not admitted: redundant",
        ))),
    );
    assert!(app.live_flash().unwrap().contains("redundant"));
    let new_selection = AgentId::new();
    app.selected = Some(new_selection);
    app.flash("new selection notice");
    app.on_reply(
        Then::AgentRenamed(id),
        Ok(Reply::Agent(agent(id, "old request"))),
    );
    assert_eq!(app.live_flash().unwrap(), "new selection notice");
    // A stale mutation reply that *failed* must still surface, even
    // though the selection moved on: silence would hide a real error.
    app.on_reply(
        Then::MemoryCreated { selected: Some(id) },
        Err(ClientError::Disconnected),
    );
    let notice = app.live_flash().unwrap().to_owned();
    assert_ne!(notice, "new selection notice");
    assert!(notice.contains("disconnected from daemon"));
    assert!(
        notice.contains("admitting the operator memory"),
        "notice must name the original operation: {notice}"
    );
    assert!(
        notice.contains(&crate::ui::views::short_id(&app.theme, &id)),
        "notice must name the original target ({id}), not the new selection: {notice}"
    );
    assert!(
        !notice.contains(&crate::ui::views::short_id(&app.theme, &new_selection)),
        "a stale error must never be attributed to the new selection: {notice}"
    );
    assert_eq!(app.selected, Some(new_selection));
}

/// Renaming agent A, selecting B before the reply lands,
/// then failing must show the error (by A's id) without touching B's
/// selection, and must keep the daemon's Unicode text intact even under
/// the ASCII theme, which only substitutes its own authored chrome.
#[test]
fn stale_agent_rename_error_stays_visible_and_keeps_unicode_after_reselect() {
    let (mut app, agent_a) = app_with_agent();
    use_ascii(&mut app);
    let effects = app.run_command(SlashCommand::Rename("名前 二".into()));
    assert!(matches!(
        &effects[..],
        [Effect::Request { then: Then::AgentRenamed(id), .. }] if *id == agent_a
    ));
    let agent_b = AgentId::new();
    app.selected = Some(agent_b);
    let unicode_error = "拒否: 名前が無効です";
    let effects = app.on_reply(
        Then::AgentRenamed(agent_a),
        Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            unicode_error,
        ))),
    );
    let notice = app.live_flash().unwrap().to_owned();
    assert!(
        notice.contains(unicode_error),
        "the daemon's own error text must survive verbatim: {notice}"
    );
    assert!(
        notice.contains("renaming the agent"),
        "notice must name the original operation: {notice}"
    );
    assert!(
        notice.contains(&crate::ui::views::short_id(&app.theme, &agent_a)),
        "notice must name the original target (agent A), not the new selection: {notice}"
    );
    assert!(
        !notice.contains(&crate::ui::views::short_id(&app.theme, &agent_b)),
        "a stale error must never be attributed to the new selection: {notice}"
    );
    assert!(
        notice.contains('|'),
        "authored chrome must use the ascii separator under the ascii theme: {notice}"
    );
    assert!(
        !notice.contains('·'),
        "ascii theme must not leak the unicode separator: {notice}"
    );
    assert_eq!(
        app.selected,
        Some(agent_b),
        "reselecting must not be reverted by a stale reply"
    );
    assert!(
        effects.is_empty(),
        "a stale failure must not retry the mutation or refresh the tree"
    );
}

/// A same-selection (non-stale) failure must behave exactly
/// as before this fix - this pins that the new stale-error path did not
/// change the already-correct non-stale path.
#[test]
fn same_selection_operator_reply_error_is_unaffected_by_stale_handling() {
    let (mut app, id) = app_with_agent();
    let effects = app.on_reply(
        Then::AgentRenamed(id),
        Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "rename rejected: duplicate name",
        ))),
    );
    assert!(
        app.live_flash()
            .unwrap()
            .contains("rename rejected: duplicate name")
    );
    assert!(effects.is_empty());
    assert_eq!(app.selected, Some(id));
}
