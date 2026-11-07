//! When each command arrived, so a client can refuse one an older daemon
//! cannot decode: one exhaustive match with no wildcard arm, so a new command
//! must say what announces it (docs/protocol.md has the history).

use super::{Command, features};

/// The protocol version every command of the 1.0.0 release decodes under.
const FIRST_VERSION: u32 = 17;

/// What a daemon must announce before a client may send a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Introduced {
    /// The protocol version that added it: a daemon speaking an older one
    /// cannot decode it.
    Version(u32),
    /// Added within a version, so only a daemon whose `Welcome` lists this
    /// feature serves it.
    Feature(&'static str),
}

impl Command {
    /// What announces this command on a connection.
    #[rustfmt::skip]
    pub fn introduced(&self) -> Introduced {
        match self {
            Self::RefreshProviderUsage { .. } => Introduced::Feature(features::PROVIDER_USAGE_REFRESH),
            Self::AcceptMemory { .. } | Self::RejectMemory { .. } => {
                Introduced::Feature(features::MEMORY_PROPOSALS)
            }
            Self::AliasRepository { .. } | Self::ListRepositoryAliases => {
                Introduced::Feature(features::REPOSITORY_ALIASES)
            }
            Self::MemoriesInContext { .. } => Introduced::Feature(features::MEMORY_CONTEXT),
            Self::RecordMemoryFeedback { .. } | Self::RetractMemoryFeedback { .. }
            | Self::MemoryFeedback { .. } => Introduced::Feature(features::MEMORY_FEEDBACK),
            Self::MemoryRetrievalExplain { .. } | Self::ConsolidateMemories { .. }
            | Self::MemoryExport { .. } | Self::MemoryImport { .. }
            | Self::MemoryMigrationStatus => Introduced::Feature(features::MEMORY_V2),
            Self::StorageStatus | Self::StorageGc { .. } => Introduced::Feature(features::STORAGE),
            Self::WorkspaceDiff { .. } | Self::WorkspaceIntegrate { .. }
            | Self::WorkspaceDiscard { .. } => Introduced::Feature(features::WORKSPACE_REVIEW),
            Self::UpdateAgent { subtree: true, .. } => Introduced::Feature(features::SUBTREE_UPDATE),
            Self::ListAgentsPage { .. } => Introduced::Feature(features::AGENTS_PAGING),
            Self::GetTree { after, limit, .. } if after.is_some() || limit.is_some() => {
                Introduced::Feature(features::TREE_PAGES)
            }
            Self::GetTree { role_prefix, root, .. } if role_prefix.is_some() || root.is_some() => {
                Introduced::Feature(features::AGENTS_PAGING)
            }
            Self::GetTranscriptPage { .. } => Introduced::Feature(features::TRANSCRIPT_PAGING),
            Self::UsageBreakdown { .. } => Introduced::Feature(features::USAGE_BREAKDOWN),
            Self::NoesisStatus | Self::NoesisConfigure { .. } | Self::NoesisCreateSession { .. }
            | Self::NoesisSend { .. } | Self::NoesisAnswer { .. } | Self::NoesisGetSession { .. }
            | Self::NoesisListSessions { .. } | Self::NoesisDeleteSession { .. }
            | Self::NoesisGraph { .. } | Self::NoesisDeleteProposition { .. }
            | Self::NoesisDeleteSource { .. } | Self::NoesisExtract { .. }
            | Self::NoesisImportSource { .. } | Self::NoesisJobs { .. }
            | Self::NoesisCancelJob { .. } | Self::NoesisLearner { .. }
            | Self::NoesisLearnerEvidence { .. } | Self::NoesisCorrectLearner { .. } => {
                Introduced::Feature(features::NOESIS)
            }
            Self::ConfigureRemote { settings, .. } if settings.mounts.is_some() => {
                Introduced::Feature(features::REMOTE_MOUNTS)
            }
            Self::AppendNote { level, .. } if !level.is_info() => Introduced::Version(24),
            Self::AppendNote { .. } => Introduced::Feature(features::NOTES),
            Self::PostNotification { .. } | Self::ListNotifications { .. }
            | Self::GetNotification { .. } | Self::MarkNotificationRead { .. }
            | Self::ResolveNotification { .. } | Self::AnswerNotification { .. } => {
                Introduced::Feature(features::NOTIFICATIONS)
            }

            Self::GetAttention | Self::CompletePath { .. } | Self::ListWorkspaces { .. }
            | Self::ValidateWorkspace { .. } | Self::GetGitSummary { .. } | Self::GetRemote
            | Self::ConfigureRemote { .. } | Self::StartPairing { .. } | Self::GetPairing { .. }
            | Self::ConfirmPairing { .. } | Self::CancelPairing { .. }
            | Self::ListDevices { .. } | Self::RevokeDevice { .. } | Self::UpdateDevice { .. }
            | Self::GetPushConfig | Self::SubscribePush { .. } | Self::UnsubscribePush
            | Self::TestPush | Self::ResolveSubmission { .. } => Introduced::Version(18),

            Self::Hello(_) | Self::Status | Self::PreviewRetention | Self::Shutdown | Self::Ping
            | Self::CreateAgent(_) | Self::GetAgent { .. } | Self::ListAgents { .. }
            | Self::GetTree { .. } | Self::UpdateAgent { .. } | Self::ReplyToQuestion { .. }
            | Self::SendMessage { .. } | Self::Cancel { .. } | Self::Interrupt { .. }
            | Self::GetGithubWatcher | Self::ConfigureGithubWatcher { .. }
            | Self::CreateSchedule { .. } | Self::GetSchedule { .. } | Self::ListSchedules { .. }
            | Self::UpdateSchedule { .. } | Self::ScheduleHistory { .. } | Self::Trigger { .. }
            | Self::Restart { .. } | Self::Wake { .. } | Self::Pause { .. }
            | Self::Resume { .. } | Self::Archive { .. } | Self::SetApprovals { .. }
            | Self::Compact { .. } | Self::EngineTool { .. } | Self::EngineTools { .. }
            | Self::CreateTask(_) | Self::GetTask { .. } | Self::ListTasks { .. }
            | Self::ListApprovals | Self::ResolveApproval { .. } | Self::ListApprovalRules { .. }
            | Self::AddApprovalRule(_) | Self::RevokeApprovalRule { .. } | Self::Subscribe { .. }
            | Self::Unsubscribe { .. } | Self::GetEvents { .. } | Self::GetTranscript { .. }
            | Self::GetInbox { .. } | Self::GetSnapshot { .. } | Self::GetInvocation { .. }
            | Self::ListInvocations { .. } | Self::GetCheckpoint { .. }
            | Self::LatestCheckpoint { .. } | Self::PublishArtifact(_) | Self::GetArtifact { .. }
            | Self::ListArtifacts { .. } | Self::ReadArtifact { .. } | Self::Memories { .. }
            | Self::CreateMemory { .. } | Self::ForgetMemory { .. } | Self::RestoreMemory { .. }
            | Self::PinMemory { .. } | Self::SyncRepositoryKnowledge { .. } | Self::ListRuns { .. }
            | Self::GetRunReport { .. } | Self::ExportRun { .. } | Self::ImportRun { .. }
            | Self::Replay { .. } | Self::ListModels | Self::AuthStatus | Self::Providers
            | Self::McpServers { .. } => Introduced::Version(FIRST_VERSION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PROTOCOL_VERSION, compat::MIN_PROTOCOL_VERSION};
    use serde_json::json;

    fn command(wire: serde_json::Value) -> Command {
        serde_json::from_value(wire.clone()).unwrap_or_else(|error| panic!("{wire}: {error}"))
    }

    /// The commands docs/protocol.md lists under version 18, read back from
    /// the wire so a renamed tag fails here rather than silently passing.
    #[test]
    fn remote_control_commands_need_protocol_18() {
        let device = crate::remote::DeviceId::new();
        for wire in [
            json!({"cmd": "get_attention"}),
            json!({"cmd": "complete_path", "prefix": "/src", "kind": "directory"}),
            json!({"cmd": "list_workspaces"}),
            json!({"cmd": "get_remote"}),
            json!({"cmd": "get_push_config"}),
            json!({"cmd": "unsubscribe_push"}),
            json!({"cmd": "test_push"}),
            json!({"cmd": "revoke_device", "id": device}),
        ] {
            let command = command(wire);
            assert_eq!(
                command.introduced(),
                Introduced::Version(18),
                "{}",
                command.name()
            );
        }
    }

    #[test]
    fn commands_added_within_a_version_name_the_feature_that_announces_them() {
        let memory = aether_core::id::MemoryId::new();
        for (wire, feature) in [
            (
                json!({"cmd": "accept_memory", "id": memory}),
                features::MEMORY_PROPOSALS,
            ),
            (
                json!({"cmd": "list_repository_aliases"}),
                features::REPOSITORY_ALIASES,
            ),
            (json!({"cmd": "storage_status"}), features::STORAGE),
            (
                json!({"cmd": "memory_feedback", "memory": memory}),
                features::MEMORY_FEEDBACK,
            ),
            (
                json!({"cmd": "memory_import", "artifact": "", "dry_run": true}),
                features::MEMORY_V2,
            ),
            (
                json!({"cmd": "memory_migration_status"}),
                features::MEMORY_V2,
            ),
            (
                json!({"cmd": "workspace_discard", "agent": aether_core::id::AgentId::new()}),
                features::WORKSPACE_REVIEW,
            ),
            (
                json!({"cmd": "append_note", "agent": aether_core::id::AgentId::new(), "text": "hi"}),
                features::NOTES,
            ),
            (
                json!({"cmd": "list_notifications"}),
                features::NOTIFICATIONS,
            ),
            (
                json!({"cmd": "answer_notification", "id": aether_core::id::NotificationId::new(), "action": "ship"}),
                features::NOTIFICATIONS,
            ),
        ] {
            let command = command(wire);
            assert_eq!(
                command.introduced(),
                Introduced::Feature(feature),
                "{}",
                command.name()
            );
        }
    }

    /// Only the subtree form is new: a daemon that ignored the field would
    /// raise the named agent alone and leave its children on the old limit.
    #[test]
    fn a_subtree_update_needs_the_subtree_update_feature() {
        let id = aether_core::id::AgentId::new();
        let wide = json!({"cmd": "update_agent", "id": id, "patch": {}, "subtree": true});
        assert_eq!(
            command(wide).introduced(),
            Introduced::Feature(features::SUBTREE_UPDATE)
        );
        let plain = json!({"cmd": "update_agent", "id": id, "patch": {}});
        assert_eq!(
            command(plain).introduced(),
            Introduced::Version(FIRST_VERSION)
        );
    }

    /// Every protocol 26 read model names the feature that announces it,
    /// and `get_tree` needs one only when a new filter is set.
    #[test]
    fn protocol_26_read_models_name_their_features() {
        let agent = aether_core::id::AgentId::new();
        for (wire, feature) in [
            (
                json!({"cmd": "list_agents_page", "query": {}}),
                features::AGENTS_PAGING,
            ),
            (
                json!({"cmd": "get_tree", "run": null, "root": agent}),
                features::AGENTS_PAGING,
            ),
            (
                json!({"cmd": "get_tree", "run": null, "role_prefix": "x"}),
                features::AGENTS_PAGING,
            ),
            (
                json!({"cmd": "get_tree", "run": null, "after": agent}),
                features::TREE_PAGES,
            ),
            (
                json!({"cmd": "get_tree", "run": null, "limit": 10}),
                features::TREE_PAGES,
            ),
            (
                json!({"cmd": "get_transcript_page", "agent": agent}),
                features::TRANSCRIPT_PAGING,
            ),
            (
                json!({"cmd": "usage_breakdown", "by": "model"}),
                features::USAGE_BREAKDOWN,
            ),
            (
                json!({"cmd": "configure_remote", "expected_revision": 1,
                       "settings": {"mounts": []}}),
                features::REMOTE_MOUNTS,
            ),
        ] {
            let command = command(wire);
            assert_eq!(
                command.introduced(),
                Introduced::Feature(feature),
                "{}",
                command.name()
            );
            assert_eq!(
                command.access(),
                expected_access(&command),
                "{}",
                command.name()
            );
        }
        let plain = json!({"cmd": "get_tree", "run": null});
        assert_eq!(
            command(plain).introduced(),
            Introduced::Version(FIRST_VERSION)
        );
        let unmounted = json!({"cmd": "configure_remote", "expected_revision": 1, "settings": {}});
        assert_eq!(command(unmounted).introduced(), Introduced::Version(18));
    }

    fn expected_access(command: &Command) -> crate::Access {
        match command {
            Command::ConfigureRemote { .. } => crate::Access::RemoteAdmin,
            _ => crate::Access::Observe,
        }
    }

    #[test]
    fn a_leveled_note_needs_protocol_24() {
        let agent = aether_core::id::AgentId::new();
        for level in ["applied", "refused"] {
            let wire = json!({"cmd": "append_note", "agent": agent, "text": "hi", "level": level});
            assert_eq!(
                command(wire).introduced(),
                Introduced::Version(24),
                "{level}"
            );
        }
        let info = json!({"cmd": "append_note", "agent": agent, "text": "hi", "level": "info"});
        assert_eq!(
            command(info).introduced(),
            Introduced::Feature(features::NOTES)
        );
    }

    #[test]
    fn the_original_commands_decode_on_every_served_version() {
        for wire in [
            json!({"cmd": "status"}),
            json!({"cmd": "ping"}),
            json!({"cmd": "list_approvals"}),
            json!({"cmd": "send_message", "to": aether_core::id::AgentId::new(), "content": "hi"}),
        ] {
            let command = command(wire);
            let Introduced::Version(version) = command.introduced() else {
                panic!("{} is versioned", command.name());
            };
            assert!(version <= MIN_PROTOCOL_VERSION, "{}", command.name());
        }
        const { assert!(FIRST_VERSION <= MIN_PROTOCOL_VERSION && 18 <= PROTOCOL_VERSION) };
    }
}
