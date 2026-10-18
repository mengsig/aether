//! `send_message`: to one agent or its live subtree, with or without images.

use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_protocol::{Command, ErrorReply, MessageDelivery, Reply};
use aether_runtime::Runtime;

use super::dispatch::blocking;
use super::map_err;
use crate::Daemon;

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let has_images =
        matches!(&command, Command::SendMessage { attachments, .. } if !attachments.is_empty());
    if let Command::SendMessage {
        content,
        attachments,
        ..
    } = &command
    {
        // An empty message would wake the agent for a paid turn with nothing to read.
        if content.trim().is_empty() && attachments.is_empty() {
            return Err(ErrorReply::invalid("a message needs text or an image"));
        }
    }
    if !has_images {
        return send(&daemon.runtime, command, by, submission);
    }
    // Megabytes are decoded and written: not on an async worker.
    let rt = daemon.runtime.clone();
    blocking(move || send(&rt, command, by, submission)).await?
}

/// The runtime answers a recorded submission before it looks at an image,
/// and stores the images of a new one as it commits the delivery.
fn send(
    rt: &Runtime,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let Command::SendMessage {
        to,
        content,
        subtree,
        attachments,
        ..
    } = command
    else {
        return Err(ErrorReply::internal("not a send_message command"));
    };
    if !subtree {
        let message = rt
            .send_message_with_images(to, content, &attachments, by, submission)
            .map_err(map_err)?;
        return Ok(Reply::MessageSent { message });
    }
    let deliveries = rt
        .fan_out_message_with_images(to, content, &attachments, by, submission)
        .map_err(map_err)?
        .into_iter()
        .map(|d| MessageDelivery {
            agent: d.agent,
            message: d.message,
        })
        .collect();
    Ok(Reply::MessagesSent { deliveries })
}
