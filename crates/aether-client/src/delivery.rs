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
        | Command::RecordMemoryFeedback {
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
            | Command::WorkspaceDiff { .. }
            | Command::ListDevices { .. }
            | Command::GetRemote
            | Command::GetPairing { .. }
            | Command::ResolveSubmission { .. }
            | Command::MemoryFeedback { .. }
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

/// The first protocol version whose daemon stores a message's images.
const IMAGES_SINCE: u32 = 19;

impl crate::Client {
    /// Refuse a message with images to a host that would deliver its text
    /// alone: an older daemon ignores `attachments` without saying so.
    pub(crate) fn check_images_supported(&self, command: &Command) -> crate::Result<()> {
        let Command::SendMessage { attachments, .. } = command else {
            return Ok(());
        };
        let Some(welcome) = self.welcome.as_ref().filter(|_| !attachments.is_empty()) else {
            return Ok(());
        };
        let host = welcome
            .daemon_protocol_version
            .unwrap_or(welcome.protocol_version);
        if host >= IMAGES_SINCE {
            return Ok(());
        }
        Err(crate::ClientError::Protocol(aether_protocol::ErrorReply {
            code: aether_protocol::ErrorCode::Unimplemented,
            message: format!(
                "sending images requires protocol {IMAGES_SINCE} and the host speaks {host}; \
                 update Aether on the host. Nothing was sent"
            ),
        }))
    }

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
