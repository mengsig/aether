//! A device's own push subscription. docs/remote.md specifies each reply.

use aether_protocol::remote::DeviceId;
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};
use aether_remote::push::PushError;

use crate::Daemon;
use crate::authz::AuthContext;

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    let Some(push) = daemon.push.as_ref() else {
        return Err(ErrorReply::new(
            ErrorCode::Unavailable,
            "this daemon does not send push notifications",
        ));
    };
    let notifier = push.notifier();
    let device = auth.device().map(|identity| identity.id);
    let config = match command {
        Command::GetPushConfig => notifier.config(device),
        Command::SubscribePush {
            subscription,
            kinds,
        } => notifier.subscribe(own(device)?, subscription, kinds),
        Command::UnsubscribePush => notifier.unsubscribe(own(device)?),
        Command::TestPush => {
            notifier.test(own(device)?).await.map_err(refusal)?;
            return Ok(Reply::Ok);
        }
        other => {
            return Err(ErrorReply::internal(format!(
                "`{}` is not a push command",
                other.name()
            )));
        }
    };
    config.map(Reply::PushConfig).map_err(refusal)
}

/// The calling device. Authorization already refused every other caller.
fn own(device: Option<DeviceId>) -> Result<DeviceId, ErrorReply> {
    device.ok_or_else(|| {
        ErrorReply::invalid("a push subscription belongs to a paired device, and this is not one")
    })
}

/// The contract's error for each way a push command fails.
fn refusal(error: PushError) -> ErrorReply {
    match error {
        PushError::BadEndpoint
        | PushError::BadKeys
        | PushError::HostNotAllowed { .. }
        | PushError::NotSubscribed => ErrorReply::invalid(error.to_string()),
        PushError::Disabled | PushError::Service(_) => {
            ErrorReply::new(ErrorCode::Unavailable, error.to_string())
        }
        PushError::Key(_) | PushError::Message(_) | PushError::Store(_) => {
            ErrorReply::internal(error)
        }
    }
}

#[cfg(test)]
mod tests;
