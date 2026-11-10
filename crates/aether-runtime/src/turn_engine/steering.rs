//! Native engine steering.
use super::{
    Arc, CancellationToken, Commit, EventKind, MessageRole, Mutation, PermissionDecision, Result,
    Runtime, TurnCtx, VecDeque, mpsc, now,
};
use aether_engine::EngineInput;

/// Messages handed to the running engine ahead of the turn boundary. An item
/// is recorded consumed only once the engine acknowledges it, so nothing a
/// person said can vanish between a turn's end and the process's exit.
pub(super) struct Steering {
    /// Handed over, not yet acknowledged; oldest first.
    pub(super) outstanding: VecDeque<PendingInbox>,
    /// Inbox seq up to which items were handed over (consumed or outstanding).
    pub(super) sent_up_to: u64,
    /// Input was full, or more is queued than one line carries: steer again
    /// on capacity even if no new inbox item arrives.
    pub(super) retry: bool,
}

impl Steering {
    pub(super) fn new(cursor: u64) -> Self {
        Self {
            outstanding: VecDeque::new(),
            sent_up_to: cursor,
            retry: false,
        }
    }

    /// Unconsumed items not handed over yet, as one user message.
    pub(super) fn pending(&self, runtime: &Runtime, ctx: &TurnCtx) -> Result<Option<PendingInbox>> {
        pending_inbox_after(runtime, ctx, self.sent_up_to.max(ctx.agent.inbox_cursor))
    }

    pub(super) fn handed(&mut self, pending: PendingInbox) {
        self.retry = false;
        self.sent_up_to = self.sent_up_to.max(pending.up_to);
        self.outstanding.push_back(pending);
    }

    /// Forget what was handed over: it stays in the inbox for the next turn.
    pub(super) fn rewind(&mut self, cursor: u64) {
        self.outstanding.clear();
        self.sent_up_to = cursor;
        self.retry = false;
    }
}

/// Hand the unconsumed inbox items one input line may carry to the running
/// engine; they are recorded consumed when it acknowledges them.
pub(super) async fn steer(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    steering: &mut Steering,
    input: &mut Option<mpsc::Sender<EngineInput>>,
) -> Result<()> {
    steering.retry = false;
    let Some(tx) = input.as_ref() else {
        return Ok(());
    };
    let Some(pending) = steering.pending(runtime, ctx)? else {
        return Ok(());
    };
    let from_user = pending.from_user;
    let more_queued = pending.more_queued;
    let message = EngineInput {
        text: pending.message.text(),
        images: runtime.engine_images(&pending.message),
    };
    match tx.try_send(message) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            // The adapter can be waiting for this loop to drain its events.
            steering.retry = true;
            return Ok(());
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            *input = None;
            return Ok(());
        }
    }
    steering.handed(pending);
    // What did not fit this line follows as soon as the engine takes input.
    steering.retry = more_queued;
    crate::turn::reply_obligation::owe(ctx, from_user);
    Ok(())
}

/// What an engine question is answered with while a steered message is still
/// unacknowledged: the person's reply is steered right after that message.
const REPLY_FOLLOWS: &str =
    "The person replied. Their reply follows as your next input; read it there.";

/// The person's reply to an engine question, consumed like any message.
/// Consuming it past a steered item the engine may still refuse would skip
/// that item, so while one is outstanding the reply follows it as input.
pub(super) async fn answer_question(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    decisions: Option<&mpsc::Sender<PermissionDecision>>,
    request_id: String,
    pending: PendingInbox,
    steering: &mut Steering,
    cancel: &CancellationToken,
) -> Result<()> {
    let Some(tx) = decisions else {
        cancel.cancel();
        return Err(crate::RuntimeError::Internal(
            "engine asked a question without a reply channel; answer remains queued".into(),
        ));
    };
    debug_assert!(
        pending.up_to > ctx.agent.inbox_cursor.max(steering.sent_up_to),
        "a reply is an item neither consumed nor handed over"
    );
    let follows = !steering.outstanding.is_empty();
    let answer = if follows {
        REPLY_FOLLOWS.to_owned()
    } else {
        pending.message.text()
    };
    if tx
        .send(PermissionDecision {
            request_id,
            allow: true,
            message: None,
            answer: Some(answer),
        })
        .await
        .is_err()
    {
        cancel.cancel();
        return Err(crate::RuntimeError::Internal(
            "engine question reply channel closed; answer remains queued".into(),
        ));
    }
    if follows {
        // Left unconsumed and unhanded: steering takes it at the next pass,
        // and a refused earlier item rewinds both into the next turn.
        steering.retry = true;
        return Ok(());
    }
    steering.sent_up_to = steering.sent_up_to.max(pending.up_to);
    steering.retry = pending.more_queued;
    record_consumed(runtime, ctx, pending)
}

/// The unconsumed inbox items one input line may carry, as one user message.
pub(super) struct PendingInbox {
    pub(super) message: aether_core::model::Message,
    pub(super) ids: Vec<aether_core::id::InboxItemId>,
    pub(super) up_to: u64,
    pub(super) from_user: bool,
    /// Later items were left queued: their images did not fit this message.
    pub(super) more_queued: bool,
}

fn pending_inbox_after(
    runtime: &Runtime,
    ctx: &TurnCtx,
    after: u64,
) -> Result<Option<PendingInbox>> {
    let queued = runtime.store.inbox(ctx.agent.id, after)?;
    // One input line carries what one message may; the rest follow once this is handed over.
    let items = aether_context::render::deliverable_prefix(&queued);
    let from_user = crate::turn::has_direct_user_message(items);
    let (message, ids, up_to) =
        aether_context::render::inbox_message(items, runtime.attachment_dir());
    Ok(match (message, up_to) {
        (Some(message), Some(up_to)) => Some(PendingInbox {
            message,
            ids,
            up_to,
            from_user,
            more_queued: items.len() < queued.len(),
        }),
        _ => None,
    })
}

/// Record inbox items as consumed mid-turn, exactly as a turn boundary would.
pub(super) fn record_consumed(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    pending: PendingInbox,
) -> Result<()> {
    // Acceptance orders the message after any text still arriving for the
    // previous input. A reply emitted before this point cannot answer it.
    crate::turn::reply_obligation::owe(ctx, pending.from_user);
    ctx.agent.inbox_cursor = ctx.agent.inbox_cursor.max(pending.up_to);
    ctx.agent.updated_at = now();
    let hold = runtime.hold();
    let row = crate::turn::agent_row(runtime, ctx, &hold)?;
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::AgentMessage {
            role: MessageRole::User,
            content: pending.message.content,
            invocation: None,
        }))
        .event(ctx.draft(EventKind::InboxConsumed {
            up_to: pending.up_to,
            items: pending.ids,
        }))
        .mutate(Mutation::UpsertAgent(row));
    if pending.from_user {
        commit.push_event(crate::turn::reply_obligation::owed_draft(
            ctx,
            pending.up_to,
        ));
    }
    runtime.apply_held(hold, commit)?;
    Ok(())
}
