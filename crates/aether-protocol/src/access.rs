//! What each command needs from its connection: one exhaustive match with no
//! wildcard arm, so a new command must be classified (docs/remote.md has the table).

use aether_core::approval::RuleScope;

use super::{AgentPatch, Command};

/// The standing a command needs. Who holds which is the daemon's decision
/// (`aether-daemon`'s `authz`); this only names the class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Reads. Any authenticated connection, an observer device included.
    Observe,
    /// Changes the fleet. The host, or a device with the operator role.
    Operate,
    /// Reaches past the fleet into the host itself. Never a paired device.
    HostOnly,
    /// Administers remote access. The host's Unix socket and nothing else.
    RemoteAdmin,
    /// A paired device acting on its own record, whatever its role.
    DeviceSelf,
}

/// Whether a remembered rule would apply to every agent this daemon runs.
fn is_global(scope: Option<&RuleScope>) -> bool {
    matches!(scope, Some(RuleScope::Everywhere))
}

/// A rule for every agent is a change to the host's own policy.
fn rule_access(scope: Option<&RuleScope>) -> Access {
    if is_global(scope) {
        Access::HostOnly
    } else {
        Access::Operate
    }
}

/// Refreshing reconnects to other processes; reading does not.
fn mcp_servers_access(refresh: bool) -> Access {
    if refresh {
        Access::Operate
    } else {
        Access::Observe
    }
}

/// An operator device may see who else is paired; history is the host's.
fn list_devices_access(include_revoked: bool) -> Access {
    if include_revoked {
        Access::RemoteAdmin
    } else {
        Access::Operate
    }
}

/// Switching the backend or the tool set reaches into the host.
fn update_agent_access(patch: &AgentPatch) -> Access {
    if patch.backend.is_some() || patch.tools.is_some() {
        Access::HostOnly
    } else {
        Access::Operate
    }
}

/// Asking what became of a command needs the standing to send it. Only
/// mutations take keys, so nothing an observer may send can be resolved.
fn resolution_access(command: &Command) -> Access {
    match command.access() {
        Access::Observe => Access::Operate,
        needed => needed,
    }
}

impl Command {
    /// The standing this command needs, from its tag and, where a field
    /// widens what it reaches, from that field.
    #[rustfmt::skip]
    pub fn access(&self) -> Access {
        match self {
            Self::CreateMemory { spec, .. } if spec.proposed => Access::HostOnly,
            Self::Memories { query } if query.proposed => Access::HostOnly,
            Self::McpServers { refresh, .. } => mcp_servers_access(*refresh),
            Self::ListDevices { include_revoked } => list_devices_access(*include_revoked),
            Self::UpdateAgent { patch, .. } => update_agent_access(patch),
            Self::ResolveApproval { remember, .. } => {
                rule_access(remember.as_ref().and_then(|r| r.scope.as_ref()))
            }
            Self::AddApprovalRule(rule) => rule_access(Some(&rule.scope)),
            Self::ResolveSubmission { command, .. } => resolution_access(command),

            Self::Hello(_) | Self::Status | Self::PreviewRetention | Self::Ping
            | Self::GetAgent { .. } | Self::ListAgents { .. } | Self::GetTree { .. }
            | Self::GetGithubWatcher | Self::GetSchedule { .. } | Self::ListSchedules { .. }
            | Self::ScheduleHistory { .. } | Self::GetTask { .. } | Self::ListTasks { .. }
            | Self::ListApprovals | Self::ListApprovalRules { .. } | Self::Subscribe { .. }
            | Self::Unsubscribe { .. } | Self::GetEvents { .. } | Self::GetTranscript { .. }
            | Self::GetInbox { .. } | Self::GetSnapshot { .. } | Self::GetInvocation { .. }
            | Self::ListInvocations { .. } | Self::GetCheckpoint { .. }
            | Self::LatestCheckpoint { .. } | Self::GetArtifact { .. }
            | Self::ListArtifacts { .. } | Self::ReadArtifact { .. } | Self::Memories { .. }
            | Self::ListRuns { .. } | Self::GetRunReport { .. } | Self::Replay { .. }
            | Self::ListModels | Self::AuthStatus | Self::Providers | Self::GetAttention
            | Self::ListWorkspaces { .. } | Self::GetPushConfig
            | Self::ListRepositoryAliases => Access::Observe,

            Self::CreateAgent(_) | Self::ReplyToQuestion { .. } | Self::SendMessage { .. }
            | Self::Cancel { .. } | Self::Interrupt { .. } | Self::ConfigureGithubWatcher { .. }
            | Self::CreateSchedule { .. } | Self::UpdateSchedule { .. } | Self::Trigger { .. }
            | Self::Restart { .. } | Self::Wake { .. } | Self::Pause { .. }
            | Self::Resume { .. } | Self::Archive { .. } | Self::SetApprovals { .. }
            | Self::Compact { .. } | Self::CreateTask(_) | Self::RevokeApprovalRule { .. }
            | Self::PublishArtifact(_) | Self::CreateMemory { .. } | Self::ForgetMemory { .. }
            | Self::RestoreMemory { .. } | Self::PinMemory { .. }
            | Self::SyncRepositoryKnowledge { .. } | Self::CompletePath { .. }
            | Self::ValidateWorkspace { .. } | Self::GetGitSummary { .. } => Access::Operate,

            Self::Shutdown | Self::EngineTool { .. } | Self::EngineTools { .. }
            | Self::ExportRun { .. } | Self::ImportRun { .. }
            | Self::AliasRepository { .. } | Self::AcceptMemory { .. }
            | Self::RejectMemory { .. } => Access::HostOnly,

            Self::GetRemote | Self::ConfigureRemote { .. } | Self::StartPairing { .. }
            | Self::GetPairing { .. } | Self::ConfirmPairing { .. }
            | Self::CancelPairing { .. } | Self::RevokeDevice { .. }
            | Self::UpdateDevice { .. } => Access::RemoteAdmin,

            Self::SubscribePush { .. } | Self::UnsubscribePush | Self::TestPush => {
                Access::DeviceSelf
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewApprovalRule, RememberSpec};
    use aether_core::id::{AgentId, ApprovalId};
    use aether_core::tool::ApprovalDecision;

    fn resolve(scope: Option<RuleScope>) -> Command {
        Command::ResolveApproval {
            id: ApprovalId::new(),
            decision: ApprovalDecision::Approved,
            note: None,
            remember: scope.map(|scope| RememberSpec {
                scope: Some(scope),
                ..Default::default()
            }),
            idempotency_key: None,
        }
    }

    #[test]
    fn the_host_only_set_is_never_open_to_an_operator_device() {
        for command in [
            Command::Shutdown,
            Command::ExportRun {
                run: aether_core::id::RunId::new(),
                path: "/tmp/x".into(),
            },
            Command::ImportRun {
                path: "/tmp/x".into(),
            },
        ] {
            assert_eq!(command.access(), Access::HostOnly, "{}", command.name());
        }
        for command in [
            Command::GetRemote,
            Command::StartPairing {
                role: Default::default(),
            },
            Command::ListDevices {
                include_revoked: true,
            },
            Command::RevokeDevice {
                id: aether_core::id::DeviceId::new(),
                reason: None,
            },
        ] {
            assert_eq!(command.access(), Access::RemoteAdmin, "{}", command.name());
        }
        let listing = Command::ListDevices {
            include_revoked: false,
        };
        assert_eq!(listing.access(), Access::Operate);
    }

    #[test]
    fn a_global_rule_is_host_only_and_a_scoped_one_is_not() {
        let agent = AgentId::new();
        assert_eq!(resolve(None).access(), Access::Operate);
        assert_eq!(
            resolve(Some(RuleScope::Agent { id: agent })).access(),
            Access::Operate
        );
        assert_eq!(
            resolve(Some(RuleScope::Everywhere)).access(),
            Access::HostOnly
        );
        let rule = |scope| {
            Command::AddApprovalRule(NewApprovalRule {
                decision: ApprovalDecision::Approved,
                matcher: Default::default(),
                scope,
                note: None,
                idempotency_key: None,
            })
        };
        assert_eq!(rule(RuleScope::Everywhere).access(), Access::HostOnly);
        assert_eq!(
            rule(RuleScope::Subtree { root: agent }).access(),
            Access::Operate
        );
    }

    /// Resolving never opens a command its caller could not send, and never
    /// drops below the standing every keyed command needs.
    #[test]
    fn resolving_a_submission_needs_the_standing_of_the_command_it_names() {
        let resolving = |command| Command::ResolveSubmission {
            command: Box::new(command),
            horizon: None,
        };
        assert_eq!(resolving(resolve(None)).access(), Access::Operate);
        assert_eq!(
            resolving(resolve(Some(RuleScope::Everywhere))).access(),
            Access::HostOnly
        );
        assert_eq!(resolving(Command::Status).access(), Access::Operate);
        assert_eq!(resolving(Command::GetRemote).access(), Access::RemoteAdmin);
        assert_eq!(
            resolving(resolving(Command::Shutdown)).access(),
            Access::HostOnly
        );
    }

    #[test]
    fn reading_is_observable_and_refreshing_is_not() {
        let servers = |refresh| Command::McpServers {
            refresh,
            server: None,
        };
        assert_eq!(servers(false).access(), Access::Observe);
        assert_eq!(servers(true).access(), Access::Operate);
        assert_eq!(Command::GetAttention.access(), Access::Observe);
        assert_eq!(Command::TestPush.access(), Access::DeviceSelf);
    }

    #[test]
    fn switching_tools_is_host_only_and_renaming_is_not() {
        let update = |patch| Command::UpdateAgent {
            id: AgentId::new(),
            patch,
            idempotency_key: None,
        };
        let rename = AgentPatch {
            name: Some("worker".into()),
            ..Default::default()
        };
        assert_eq!(update(rename).access(), Access::Operate);
        let tools = AgentPatch {
            tools: Some(Default::default()),
            ..Default::default()
        };
        assert_eq!(update(tools).access(), Access::HostOnly);
    }
}
