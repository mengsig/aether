use super::*;
use crate::notifications::Poster;
use aether_core::notification::{NotificationChange, NotificationPost};

pub(super) fn post(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    post: NotificationPost,
) -> Result<ToolOutcome> {
    let key = post.key.clone();
    let posted = match runtime.post_notification(Poster::Agent(ctx.agent.id), post) {
        Ok(posted) => posted,
        Err(error) => return refused(error),
    };
    debug_assert_eq!(posted.notification.key, key, "a post keeps its key");
    let id = posted.notification.id;
    Ok(ToolOutcome::ok(match posted.change {
        Some(NotificationChange::Posted) => {
            format!("Posted {id}. An answer reaches your inbox; do not wait for it here.")
        }
        Some(_) => format!("Updated {id}."),
        None => format!("{id} already says this; nothing changed."),
    }))
}

pub(super) fn resolve(runtime: &Arc<Runtime>, ctx: &TurnCtx, key: &str) -> Result<ToolOutcome> {
    let source = ctx.agent.id.to_string();
    match runtime.resolve_notification_by_key(&source, key, ctx.base_actor.clone()) {
        Ok(resolved) => Ok(ToolOutcome::ok(format!("Resolved {}.", resolved.id))),
        Err(error) => refused(error),
    }
}

/// A refusal the model can act on is its tool result; anything else fails the turn.
fn refused(error: RuntimeError) -> Result<ToolOutcome> {
    match error {
        RuntimeError::Invalid(why) => Ok(ToolOutcome::error(format!("Refused: {why}"))),
        RuntimeError::NotFound(what) => Ok(ToolOutcome::error(format!("{what} not found."))),
        other => Err(other),
    }
}
