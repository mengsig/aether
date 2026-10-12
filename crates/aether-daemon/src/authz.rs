//! Who a connection is and what that lets it run, checked before a request is otherwise looked at
//! (before subscribing, shutdown or dispatch). docs/remote.md prints the matrix.

use std::net::SocketAddr;

use aether_core::approval::RuleScope;
use aether_core::event::{Actor, OperatorOrigin};
use aether_protocol::remote::{DeviceIdentity, DeviceRole};
use aether_protocol::{Access, Command, ErrorCode, ErrorReply, Hello};
use aether_remote::DeviceSession;
use aether_remote::auth::AuthError;
use tokio_util::sync::CancellationToken;

use crate::Daemon;
use crate::listen::ConnectionPolicy;

/// The one refusal every failed device hello gets, whatever went wrong.
const DEVICE_REFUSAL: &str = "device authentication failed";
const DEVICES_BUSY: &str = "every device connection is in use; try again shortly";

/// The standing of one connection, fixed when its hello is accepted.
#[derive(Debug, Clone)]
pub(crate) enum AuthContext {
    /// The Unix socket: this user, at this machine.
    Local,
    /// The `[listen]` TCP transport and its shared token. Never a device.
    Token,
    /// The remote carrier: a paired device, with the role it was given.
    Device(DeviceSession),
}

impl AuthContext {
    pub(crate) fn submission(
        &self,
        command: &Command,
    ) -> Result<Option<aether_core::idempotency::Submission>, ErrorReply> {
        let Some(keyed) = command.keyed_request() else {
            return Ok(None);
        };
        let keyed = keyed.map_err(ErrorReply::internal)?;
        let mut submission = aether_core::idempotency::Submission::new(keyed.key, &keyed.fields)
            .map_err(|error| ErrorReply::invalid(error.to_string()))?;
        if let Some(device) = self.device() {
            submission.key.0 = format!("device:{}:{}", device.id, submission.key.0);
        }
        Ok(Some(submission))
    }

    pub(crate) fn redact_event(&self, event: &mut aether_core::event::Event) {
        if !matches!(self, Self::Local)
            && let aether_core::event::EventKind::Custom { name, payload } = &mut event.kind
            && name.starts_with("remote.")
        {
            *name = "remote.restricted".into();
            *payload = serde_json::json!({"redacted": true});
        }
    }

    pub(crate) fn redact_frame(&self, frame: &mut aether_protocol::ServerFrame) {
        use aether_protocol::{Reply, ServerFrame};
        match frame {
            ServerFrame::Event { event, .. } => self.redact_event(event),
            ServerFrame::Reply {
                result: Ok(Reply::Events(events)),
                ..
            } => {
                for event in events {
                    self.redact_event(event);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn device(&self) -> Option<&DeviceIdentity> {
        match self {
            AuthContext::Device(session) => Some(&session.identity),
            AuthContext::Local | AuthContext::Token => None,
        }
    }

    /// Fires when a device is revoked or re-roled. Never for the host.
    pub(crate) fn closed(&self) -> Option<&CancellationToken> {
        match self {
            AuthContext::Device(session) => Some(&session.closed),
            AuthContext::Local | AuthContext::Token => None,
        }
    }

    /// Who the event log says acted. A device keeps the operator's standing;
    /// the origin names it from the enrolled record, not from the client.
    pub(crate) fn actor(&self, peer: Option<SocketAddr>, connection: String) -> Actor {
        if let Some(device) = self.device()
            && device.role == DeviceRole::Observer
        {
            return Actor::Client {
                id: device.id.to_string(),
            };
        }
        let origin = match (self.device(), peer) {
            (Some(device), _) => OperatorOrigin::Device {
                device: device.id,
                name: device.name.clone(),
                connection,
            },
            (None, Some(peer)) => OperatorOrigin::Tcp { peer, connection },
            (None, None) => OperatorOrigin::LocalSocket { connection },
        };
        Actor::connected_user(origin)
    }

    /// The matrix. No wildcard: a new class or context must be placed.
    fn permits(&self, access: Access) -> bool {
        match (self, access) {
            (_, Access::Observe) => true,
            (AuthContext::Local | AuthContext::Token, Access::Operate | Access::HostOnly) => true,
            (AuthContext::Device(session), Access::Operate) => {
                session.identity.role == DeviceRole::Operator
            }
            (AuthContext::Device(_), Access::HostOnly) => false,
            (AuthContext::Local, Access::RemoteAdmin) => true,
            (AuthContext::Token | AuthContext::Device(_), Access::RemoteAdmin) => false,
            (AuthContext::Device(_), Access::DeviceSelf) => true,
            (AuthContext::Local | AuthContext::Token, Access::DeviceSelf) => false,
        }
    }

    /// Whether this connection may send `command` at all.
    pub(crate) fn authorize(&self, command: &Command) -> Result<(), ErrorReply> {
        if self.closed().is_some_and(CancellationToken::is_cancelled) {
            return Err(ErrorReply::forbidden(
                "this device was revoked or its role changed",
            ));
        }
        let access = command.access();
        if self.permits(access) {
            return Ok(());
        }
        let name = command.name();
        Err(match access {
            Access::DeviceSelf => ErrorReply::invalid(format!(
                "`{name}` acts on the calling device, and this connection is not a paired device"
            )),
            Access::RemoteAdmin => ErrorReply::forbidden(format!(
                "`{name}` administers remote access and is only served on the daemon's own socket"
            )),
            Access::HostOnly => {
                ErrorReply::forbidden(format!("`{name}` is not available to a remote device"))
            }
            Access::Operate | Access::Observe => {
                ErrorReply::forbidden(format!("`{name}` needs the operator role"))
            }
        })
    }
}

impl ConnectionPolicy {
    /// Turn an accepted version's hello into this connection's standing.
    /// A carrier connection must prove a device; nothing else may claim one.
    pub(crate) fn admit(&self, hello: &Hello) -> Result<AuthContext, ErrorReply> {
        let Some(gate) = self.device.as_ref() else {
            self.authorize(hello)?;
            return Ok(match self.token {
                Some(_) => AuthContext::Token,
                None => AuthContext::Local,
            });
        };
        let refusal = || ErrorReply::new(ErrorCode::Unauthorized, DEVICE_REFUSAL);
        let proof = hello.device.as_ref().ok_or_else(refusal)?;
        match gate.authenticator.authenticate(&gate.challenge, proof) {
            Ok(session) => Ok(AuthContext::Device(session)),
            // Not a verdict on the device: it keeps its pairing and retries.
            Err(AuthError::Busy) => Err(ErrorReply::new(ErrorCode::Unavailable, DEVICES_BUSY)),
            Err(error) => {
                // An unauthenticated peer can cause this line: never louder.
                tracing::debug!(device = %proof.id, %error, "refused a device hello");
                Err(refusal())
            }
        }
    }
}

/// The checks the class alone cannot make, because they depend on what is
/// stored. Runs after [`AuthContext::authorize`] and before anything else.
pub(crate) fn guard(
    daemon: &Daemon,
    auth: &AuthContext,
    command: &Command,
) -> Result<(), ErrorReply> {
    auth.authorize(command)?;
    if auth.device().is_none() {
        return Ok(());
    }
    let rt = &daemon.runtime;
    if let Some(submission) = auth.submission(command)?
        && let Some(record) = rt
            .store()
            .submission(&submission.key)
            .map_err(ErrorReply::internal)?
        && record.scheme == aether_core::idempotency::FINGERPRINT_SCHEME
        && record.fingerprint == submission.fingerprint
    {
        return Ok(());
    }
    match command {
        Command::CreateAgent(spec) => ceilings::creation(
            rt,
            spec.scope.as_ref(),
            spec.config.as_ref(),
            spec.approval.as_ref(),
            spec.parent,
            spec.model.as_ref(),
            &spec.fallbacks,
        ),
        Command::CreateSchedule { spec, .. } => match &spec.target {
            aether_core::schedule::ScheduleTarget::Spawn(spec) => ceilings::creation(
                rt,
                spec.scope.as_ref(),
                spec.config.as_ref(),
                spec.approval.as_ref(),
                None,
                spec.model.as_ref(),
                &spec.fallbacks,
            ),
            aether_core::schedule::ScheduleTarget::Agent(_) => Ok(()),
        },
        Command::UpdateAgent { id, patch, .. } => ceilings::update(rt, *id, patch),
        Command::SetApprovals { agent, policy, .. } => ceilings::approvals(rt, *agent, policy),
        Command::Trigger { source, .. }
            if source
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.'))
                || source.is_empty()
                || source.len() > 128 =>
        {
            Err(ErrorReply::forbidden(
                "remote trigger source must be 1-128 ASCII letters, digits, dots, underscores or hyphens",
            ))
        }
        _ => administration_guard(rt, command),
    }
}

fn administration_guard(rt: &aether_runtime::Runtime, command: &Command) -> Result<(), ErrorReply> {
    match command {
        // Who the watcher obeys on GitHub is the host's decision.
        Command::ConfigureGithubWatcher {
            config,
            expected_revision,
        } => {
            let current = rt.github_watcher_status().map_err(ErrorReply::internal)?;
            if *expected_revision != current.revision {
                return Err(ErrorReply::invalid(
                    "the GitHub watcher revision changed; read it again before saving",
                ));
            }
            let mut proposed = config.clone();
            proposed.normalize();
            if proposed.operator_users != current.config.operator_users {
                return Err(ErrorReply::forbidden(
                    "`operator_users` can only be changed at the host",
                ));
            }
            Ok(())
        }
        // A rule for every agent is the host's policy, to make or unmake.
        Command::RevokeApprovalRule { id, .. } => {
            let rules = rt.approval_rules(true).map_err(ErrorReply::internal)?;
            let global = rules
                .iter()
                .any(|rule| rule.id == *id && matches!(rule.scope, RuleScope::Everywhere));
            if global {
                return Err(ErrorReply::forbidden(
                    "a rule that applies everywhere can only be revoked at the host",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;

mod ceilings;
