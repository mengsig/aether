//! A direct user message owes a visible reply. The debt is a pair of custom
//! events, so a restarted turn rebuilds it from the log instead of forgetting.
use aether_core::event::{EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::inbox::InboxItem;
use aether_store::EventFilter;

use super::{Result, Runtime, TurnCtx, has_direct_user_message};

pub(crate) const REPLY_OWED: &str = "reply.owed";
pub(crate) const REPLY_GIVEN: &str = "reply.given";

/// Custom events read per page while looking for the agent's latest verdict.
const OBLIGATION_SCAN_PAGE: usize = 50;

/// Whether `items` up to `up_to` hold a direct user message: consuming them
/// creates the obligation.
pub(crate) fn consumed_user_message(items: &[InboxItem], up_to: u64) -> bool {
    let end = items.partition_point(|item| item.seq <= up_to);
    has_direct_user_message(&items[..end])
}

/// The event recording that the inbox through `up_to` left a reply owed.
pub(crate) fn owed_draft(ctx: &TurnCtx, up_to: u64) -> EventDraft {
    ctx.draft(EventKind::Custom {
        name: REPLY_OWED.into(),
        payload: serde_json::json!({ "inbox_seq": up_to }),
    })
}

/// Whether the agent's newest obligation event says a reply is still owed.
pub(crate) fn load(runtime: &Runtime, agent: AgentId) -> Result<bool> {
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::own_events(agent)
    };
    let mut before = u64::MAX;
    loop {
        let page = runtime
            .store
            .events_before(before, OBLIGATION_SCAN_PAGE, &filter)?;
        let Some(oldest) = page.last() else {
            return Ok(false);
        };
        assert!(
            oldest.seq < before,
            "the scan must move back through the log"
        );
        before = oldest.seq;
        let verdict = page.iter().find_map(|event| match &event.kind {
            EventKind::Custom { name, .. } if name == REPLY_OWED => Some(true),
            EventKind::Custom { name, .. } if name == REPLY_GIVEN => Some(false),
            _ => None,
        });
        if let Some(owed) = verdict {
            return Ok(owed);
        }
    }
}

/// A new user input arrived: an earlier streamed answer cannot discharge it.
pub(crate) fn owe(ctx: &mut TurnCtx, from_user: bool) {
    if from_user {
        ctx.user_reply_pending = true;
        ctx.reply_given_unrecorded = false;
    }
}

/// Visible assistant text has answered in memory; the log learns of it only
/// with the committed reply (`take_given_draft`), never before it is durable.
pub(crate) fn answer(ctx: &mut TurnCtx) {
    if ctx.user_reply_pending {
        ctx.user_reply_pending = false;
        ctx.reply_given_unrecorded = true;
    }
}

/// The event closing the debt, to commit with the reply that paid it.
pub(crate) fn given_draft(ctx: &mut TurnCtx) -> EventDraft {
    ctx.reply_given_unrecorded = false;
    ctx.draft(EventKind::Custom {
        name: REPLY_GIVEN.into(),
        payload: serde_json::Value::Null,
    })
}

/// `given_draft` when an answered reply is still unrecorded.
pub(crate) fn take_given_draft(ctx: &mut TurnCtx) -> Option<EventDraft> {
    ctx.reply_given_unrecorded.then(|| given_draft(ctx))
}
