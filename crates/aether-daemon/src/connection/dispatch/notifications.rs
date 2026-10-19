//! The notification center: services post, people read, resolve and answer.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::id::NotificationId;
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::notifications::Poster;
use aether_store::NotificationFilter;

use crate::connection::dispatch::blocking;
use crate::connection::map_err;

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::PostNotification { .. }
            | Command::ListNotifications { .. }
            | Command::GetNotification { .. }
            | Command::MarkNotificationRead { .. }
            | Command::ResolveNotification { .. }
            | Command::AnswerNotification { .. }
    )
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    match command {
        Command::PostNotification {
            source,
            notification,
        } => {
            let posted = rt
                .post_notification(Poster::Service(source), notification)
                .map_err(map_err)?;
            Ok(Reply::Notification(posted.notification))
        }
        Command::ListNotifications {
            states,
            source,
            limit,
        } => list(daemon, states, source, limit).await,
        Command::GetNotification { id } => {
            Ok(Reply::Notification(rt.notification(id).map_err(map_err)?))
        }
        Command::MarkNotificationRead { id } => Ok(Reply::Notification(
            rt.mark_notification_read(id, by).map_err(map_err)?,
        )),
        Command::ResolveNotification { id, source, key } => resolve(daemon, id, source, key, by),
        Command::AnswerNotification { id, action, text } => {
            let answered = rt
                .answer_notification(id, action, text, by)
                .map_err(map_err)?;
            Ok(Reply::NotificationAnswered {
                notification: answered.notification,
                delivered_to: answered.delivered_to,
            })
        }
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn list(
    daemon: &Daemon,
    states: Vec<aether_core::notification::NotificationState>,
    source: Option<String>,
    limit: Option<usize>,
) -> Result<Reply, ErrorReply> {
    let filter = NotificationFilter {
        states,
        source,
        limit: limit.unwrap_or(NotificationFilter::default().limit),
    };
    let rt = daemon.runtime.clone();
    let listed = blocking(move || rt.notifications(&filter)).await?;
    Ok(Reply::Notifications(listed.map_err(map_err)?))
}

/// Exactly one of an id, or the source and key a notification was posted under.
fn resolve(
    daemon: &Daemon,
    id: Option<NotificationId>,
    source: Option<String>,
    key: Option<String>,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let resolved = match (id, source, key) {
        (Some(id), None, None) => rt.resolve_notification(id, by),
        (None, Some(source), Some(key)) => rt.resolve_notification_by_key(&source, &key, by),
        _ => {
            return Err(ErrorReply::new(
                aether_protocol::ErrorCode::InvalidArgument,
                "resolve_notification takes an `id`, or a `source` and a `key`",
            ));
        }
    };
    Ok(Reply::Notification(resolved.map_err(map_err)?))
}
