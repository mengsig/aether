//! Remote administration: runtime settings, pairing and device records. Host-only, except listing
//! devices, which an operator device may do and is told less. docs/remote.md specifies each reply.

use aether_core::event::Actor;
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};

use super::map_err;
use crate::Daemon;
use crate::authz::AuthContext;
use crate::remote::{AdminError, RemoteControl};

fn refusal(error: AdminError) -> ErrorReply {
    match error {
        AdminError::Invalid(message) => ErrorReply::new(ErrorCode::InvalidArgument, message),
        AdminError::NotFound(message) => ErrorReply::new(ErrorCode::NotFound, message),
        AdminError::Transition(message) => ErrorReply::new(ErrorCode::InvalidTransition, message),
        AdminError::Runtime(error) => map_err(error),
        AdminError::Internal(message) => ErrorReply::internal(message),
    }
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    let Some(remote) = daemon.remote.get() else {
        return Err(ErrorReply::new(
            ErrorCode::Unavailable,
            "this daemon was started without remote control",
        ));
    };
    serve(remote, command, by, auth).await.map_err(refusal)
}

async fn serve(
    remote: &RemoteControl,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, AdminError> {
    let caller = auth.device();
    Ok(match command {
        Command::GetRemote => Reply::Remote(remote.status().await?),
        Command::ConfigureRemote {
            settings,
            expected_revision,
        } => Reply::Remote(remote.configure(settings, expected_revision, by).await?),
        Command::StartPairing { role } => Reply::Pairing(remote.start_pairing(role, by).await?),
        Command::GetPairing { id } => Reply::Pairing(remote.pairing(&id)?),
        Command::ConfirmPairing { id, accept } => {
            Reply::Pairing(remote.confirm_pairing(&id, accept, by).await?)
        }
        Command::CancelPairing { id } => Reply::Pairing(remote.cancel_pairing(&id, by).await?),
        Command::ListDevices { include_revoked } => {
            // A device is never told of the revoked, whatever it asks for.
            let devices = remote.devices(include_revoked && caller.is_none())?;
            let listed: Result<Vec<_>, _> = devices
                .iter()
                .map(|device| remote.view(device, caller))
                .collect();
            Reply::Devices(listed?)
        }
        Command::RevokeDevice { id, reason } => {
            let device = remote.revoke(id, reason, by).await?;
            Reply::Device(remote.view(&device, caller)?)
        }
        Command::UpdateDevice { id, name, role } => {
            let device = remote.update(id, name, role, by).await?;
            Reply::Device(remote.view(&device, caller)?)
        }
        other => {
            return Err(AdminError::Internal(format!(
                "`{}` is not a remote administration command",
                other.name()
            )));
        }
    })
}
