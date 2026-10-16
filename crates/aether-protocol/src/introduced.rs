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
            Self::StorageStatus | Self::StorageGc { .. } => Introduced::Feature(features::STORAGE),
            Self::WorkspaceDiff { .. } | Self::WorkspaceIntegrate { .. }
            | Self::WorkspaceDiscard { .. } => Introduced::Feature(features::WORKSPACE_REVIEW),
            Self::AppendNote { .. } => Introduced::Feature(features::NOTES),

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
                json!({"cmd": "workspace_discard", "agent": aether_core::id::AgentId::new()}),
                features::WORKSPACE_REVIEW,
            ),
            (
                json!({"cmd": "append_note", "agent": aether_core::id::AgentId::new(), "text": "hi"}),
                features::NOTES,
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
