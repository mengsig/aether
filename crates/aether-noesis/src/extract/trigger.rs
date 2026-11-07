//! When a session is extracted without being asked (parity X2, which
//! differs from upstream): only with `noesis.extraction.sessions = "auto"`
//! in effect, and only once enough teacher prose is uncovered.

use aether_core::noesis::{Job, JobKind};
use rusqlite::Connection;

use super::batch::{tokens, uncovered};
use super::{Corpus, ExtractError, jobs};
use crate::graph::store::coverage_of;
use crate::settings::{NoesisSettings, SessionExtraction};

/// Uncovered teacher tokens that start an automatic extraction.
pub const AUTO_THRESHOLD_TOKENS: u64 = 4_096;

/// Whether the effective settings extract sessions automatically.
pub fn auto_enabled(effective: &NoesisSettings) -> bool {
    effective.enabled && effective.extraction_sessions == SessionExtraction::Auto
}

/// After a teaching turn: queue an extraction of `session` when automatic
/// extraction is in effect, none is already live on it, and at least
/// [`AUTO_THRESHOLD_TOKENS`] of its teacher prose is uncovered. Manual mode
/// reads nothing and queues nothing.
pub fn after_teaching_turn(
    conn: &Connection,
    effective: &NoesisSettings,
    corpus: &dyn Corpus,
    session: &str,
) -> Result<Option<Job>, ExtractError> {
    assert!(session.starts_with("ns_"), "a teaching session id");
    if !auto_enabled(effective) {
        return Ok(None);
    }
    if jobs::live_on(conn, session)?.is_some() {
        return Ok(None);
    }
    let prose = corpus
        .session_prose(session)
        .map_err(ExtractError::Invalid)?;
    let coverage = coverage_of(conn, session)?;
    let pending: u64 = uncovered(prose, &coverage)
        .iter()
        .map(|m| tokens(&m.text))
        .sum();
    if pending < AUTO_THRESHOLD_TOKENS {
        return Ok(None);
    }
    let job = jobs::create(conn, JobKind::ExtractSession, session)?;
    debug_assert_eq!(job.state, aether_core::noesis::JobState::Queued);
    Ok(Some(job))
}
