//! Stable keys identify operations whose acknowledgement may be lost.
use aether_protocol::Command;

pub fn key_command(command: &mut Command) -> bool {
    let Some(key) = key_slot(command) else {
        return false;
    };
    if key.is_none() {
        *key = Some(format!("ssh-{:032x}", rand::random::<u128>()));
    }
    true
}

fn key_slot(command: &mut Command) -> Option<&mut Option<String>> {
    let key = match command {
        Command::CreateAgent(agent) => &mut agent.idempotency_key,
        Command::CreateTask(task) => &mut task.idempotency_key,
        Command::AddApprovalRule(rule) => &mut rule.idempotency_key,
        Command::SendMessage {
            idempotency_key, ..
        }
        | Command::ReplyToQuestion {
            idempotency_key, ..
        }
        | Command::Trigger {
            idempotency_key, ..
        }
        | Command::CreateSchedule {
            idempotency_key, ..
        }
        | Command::ResolveApproval {
            idempotency_key, ..
        }
        | Command::Restart {
            idempotency_key, ..
        }
        | Command::Cancel {
            idempotency_key, ..
        }
        | Command::Interrupt {
            idempotency_key, ..
        }
        | Command::Pause {
            idempotency_key, ..
        }
        | Command::Resume {
            idempotency_key, ..
        }
        | Command::Archive {
            idempotency_key, ..
        }
        | Command::UpdateAgent {
            idempotency_key, ..
        }
        | Command::SetApprovals {
            idempotency_key, ..
        }
        | Command::UpdateSchedule {
            idempotency_key, ..
        }
        | Command::CreateMemory {
            idempotency_key, ..
        } => idempotency_key,
        _ => return None,
    };
    Some(key)
}

pub(crate) fn is_mutating(command: &Command) -> bool {
    matches!(
        command.access(),
        aether_protocol::Access::Operate
            | aether_protocol::Access::HostOnly
            | aether_protocol::Access::RemoteAdmin
    ) && !matches!(
        command,
        Command::CompletePath { .. }
            | Command::ValidateWorkspace { .. }
            | Command::GetGitSummary { .. }
            | Command::ListDevices { .. }
            | Command::GetRemote
            | Command::GetPairing { .. }
            | Command::ResolveSubmission { .. }
    )
}

fn legacy_keyed_command(command: &Command) -> bool {
    matches!(
        command,
        Command::CreateAgent(_)
            | Command::CreateTask(_)
            | Command::SendMessage { .. }
            | Command::ReplyToQuestion { .. }
            | Command::Trigger { .. }
            | Command::CreateSchedule { .. }
    )
}

impl crate::Client {
    pub(crate) fn prepare_remote_command(&self, command: &mut Command) -> crate::Result<bool> {
        if self.carrier.is_none() {
            return Ok(false);
        }
        let modern = self.welcome.as_ref().is_none_or(|welcome| {
            welcome
                .daemon_protocol_version
                .unwrap_or(welcome.protocol_version)
                >= 18
        });
        if !modern
            && matches!(
                command,
                Command::CompletePath { .. }
                    | Command::ListWorkspaces { .. }
                    | Command::ValidateWorkspace { .. }
                    | Command::GetGitSummary { .. }
                    | Command::GetAttention
            )
        {
            return Err(crate::ClientError::Protocol(aether_protocol::ErrorReply {
                code: aether_protocol::ErrorCode::Unimplemented,
                message: "host services require protocol 18; update Aether on the remote host"
                    .into(),
            }));
        }
        if !modern && !legacy_keyed_command(command) {
            if let Some(key) = key_slot(command) {
                *key = None;
            }
            return Ok(false);
        }
        Ok(key_command(command))
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
