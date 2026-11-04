//! What a client that cannot read `noesis.*` events is sent instead: the same
//! event as `custom`, named by its dotted type, carrying its fields.

use aether_core::event::{Event, EventKind};
use serde_json::Value;

use crate::{Reply, ServerFrame};

const NOESIS_PREFIX: &str = "noesis.";

/// Rewrite every `noesis.*` event in `frame` as a `custom` event, for a client
/// that did not list `features::NOESIS` or speaks a protocol before 26.
/// Whether anything was rewritten.
pub fn withhold_noesis_events(frame: &mut ServerFrame) -> bool {
    match frame {
        ServerFrame::Event { event, .. } => withhold_in(event),
        ServerFrame::Reply {
            result: Ok(Reply::Events(events)),
            ..
        } => events
            .iter_mut()
            .fold(false, |changed, event| withhold_in(event) | changed),
        _ => false,
    }
}

fn withhold_in(event: &mut Event) -> bool {
    let Some(custom) = as_custom(&event.kind) else {
        return false;
    };
    debug_assert!(matches!(custom, EventKind::Custom { .. }));
    event.kind = custom;
    true
}

/// The `custom` form of a Noesis event; `None` for any other event.
fn as_custom(kind: &EventKind) -> Option<EventKind> {
    let Ok(Value::Object(mut fields)) = serde_json::to_value(kind) else {
        debug_assert!(false, "an event kind serializes to an object");
        return None;
    };
    let Some(Value::String(name)) = fields.remove("type") else {
        debug_assert!(false, "an event kind carries its type");
        return None;
    };
    name.starts_with(NOESIS_PREFIX)
        .then_some(EventKind::Custom {
            name,
            payload: Value::Object(fields),
        })
}

#[cfg(test)]
mod tests;
