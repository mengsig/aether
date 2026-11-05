//! Engineering extraction: a session's evidence bundle into the graph. It is
//! opt-in behind both switches (`noesis.enabled` and
//! `noesis.engineering_context`); with either off, nothing is read or queued.
//!
//! Only `verified` and `reviewed` items are read: the graph holds true
//! statements only, and a `preliminary` item has not been checked.

use aether_core::noesis::{EvidenceStatus, Job, JobKind};
use rusqlite::Connection;

use super::batch::Message;
use super::{ExtractError, jobs, session_origin};
use crate::evidence::{self, kind_name, status_name};
use crate::settings::{EngineeringExtraction, NoesisSettings};

/// The `message_ref` prefix of a bundle item, beside a session's own refs.
pub const REF_PREFIX: &str = "evidence:";

/// Whether engineering extraction may run at all under `effective`.
pub fn allowed(effective: &NoesisSettings) -> bool {
    evidence::in_effect(effective)
}

/// Whether a stored bundle is extracted without being asked.
pub fn auto_enabled(effective: &NoesisSettings) -> bool {
    allowed(effective) && effective.extraction_engineering == EngineeringExtraction::Auto
}

/// After a session with a bundle is created: queue its engineering
/// extraction when `noesis.extraction.engineering = "auto"` is in effect.
/// Practice sessions, sessions without a bundle and a live job queue nothing.
pub fn after_bundle_stored(
    conn: &Connection,
    effective: &NoesisSettings,
    session: &str,
) -> Result<Option<Job>, ExtractError> {
    assert!(session.starts_with("ns_"), "a teaching session id");
    if !auto_enabled(effective) {
        return Ok(None);
    }
    let Some(origin) = session_origin(conn, session)? else {
        return Ok(None);
    };
    if origin.practice || jobs::live_on(conn, session)?.is_some() {
        return Ok(None);
    }
    if load(conn, session)?.is_none() {
        return Ok(None);
    }
    let job = jobs::create(conn, JobKind::ExtractEngineering, session)?;
    debug_assert_eq!(job.kind, JobKind::ExtractEngineering);
    Ok(Some(job))
}

/// What an engineering job reads: the bundle's checked items, one message
/// each, the status label kept in its text. Refused unless both switches hold.
pub fn material(
    conn: &Connection,
    engineering_in_effect: bool,
    session: &str,
) -> Result<Vec<Message>, String> {
    if !engineering_in_effect {
        return Err("engineering context is off; nothing was read".into());
    }
    let bundle = load(conn, session)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("session `{session}` has no evidence bundle"))?;
    let messages: Vec<Message> = bundle
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.status != EvidenceStatus::Preliminary)
        .map(|(ord, item)| Message {
            message_ref: format!("{REF_PREFIX}{ord}"),
            text: format!(
                "[{}] {} {}: {}",
                status_name(item.status),
                kind_name(item.reference.kind),
                item.reference.id,
                item.excerpt
            ),
        })
        .collect();
    debug_assert!(messages.len() <= bundle.items.len());
    debug_assert!(
        messages
            .iter()
            .all(|m| m.message_ref.starts_with(REF_PREFIX))
    );
    Ok(messages)
}

fn load(
    conn: &Connection,
    session: &str,
) -> Result<Option<aether_core::noesis::EvidenceBundle>, ExtractError> {
    evidence::store::load(conn, session).map_err(|error| match error {
        evidence::EvidenceError::Sqlite(error) => ExtractError::Sqlite(error),
        other => ExtractError::Invalid(other.to_string()),
    })
}
