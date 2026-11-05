//! Engineering context (CONTRACTS.md §6.3): an evidence bundle the frontend
//! assembled from reads the user can already make, kept with the session it
//! starts (N3) and shown to the teaching agents with every status label.
//!
//! Noesis never fetches anything a bundle names: a reference and its URL are
//! data for the learner and the model, never something to open.

use aether_core::noesis::{EvidenceBundle, EvidenceRefKind, EvidenceStatus};

use crate::settings::NoesisSettings;

pub mod prompt;
pub mod store;

#[cfg(test)]
mod tests;

/// The most bytes (UTF-8) all of a bundle's excerpts may hold together.
pub const MAX_EXCERPT_BYTES: usize = 32 * 1024;
/// Bounds on what surrounds the excerpts, so the whole bundle stays bounded.
pub const MAX_ITEMS: usize = 50;
pub const MAX_TITLE_CHARS: usize = 300;
pub const MAX_REF_ID_BYTES: usize = 200;
pub const MAX_URL_BYTES: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    /// Engineering context is not in effect; nothing was stored or sent.
    #[error("engineering context is off; turn on noesis.engineering_context to learn from it")]
    Disabled,
    #[error("context excerpts total {total} bytes; the limit is {MAX_EXCERPT_BYTES}")]
    TooLarge { total: usize },
    #[error("invalid evidence bundle: {0}")]
    Invalid(String),
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("noesis.db holds a malformed evidence bundle: {0}")]
    Corrupt(String),
}

/// Whether engineering context is in effect: Noesis on, and the switch on.
pub fn in_effect(effective: &NoesisSettings) -> bool {
    effective.enabled && effective.engineering_context
}

/// The total bytes of `bundle`'s excerpts.
pub fn excerpt_bytes(bundle: &EvidenceBundle) -> usize {
    bundle.items.iter().map(|item| item.excerpt.len()).sum()
}

/// Check `bundle` against §6.3 and the bounds above; returns its excerpt bytes.
pub fn validate(bundle: &EvidenceBundle) -> Result<usize, EvidenceError> {
    let title = bundle.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(EvidenceError::Invalid(format!(
            "a title has 1 to {MAX_TITLE_CHARS} characters"
        )));
    }
    if bundle.items.is_empty() || bundle.items.len() > MAX_ITEMS {
        return Err(EvidenceError::Invalid(format!(
            "a bundle has 1 to {MAX_ITEMS} items"
        )));
    }
    for (index, item) in bundle.items.iter().enumerate() {
        let id = &item.reference.id;
        if id.trim().is_empty() || id.len() > MAX_REF_ID_BYTES {
            return Err(EvidenceError::Invalid(format!(
                "item {index}: a reference id has 1 to {MAX_REF_ID_BYTES} bytes"
            )));
        }
        if item
            .reference
            .url
            .as_ref()
            .is_some_and(|url| url.len() > MAX_URL_BYTES)
        {
            return Err(EvidenceError::Invalid(format!(
                "item {index}: a url has at most {MAX_URL_BYTES} bytes"
            )));
        }
    }
    let total = excerpt_bytes(bundle);
    if total > MAX_EXCERPT_BYTES {
        return Err(EvidenceError::TooLarge { total });
    }
    Ok(total)
}

/// Admit a bundle for a new session: refused unless engineering context is
/// in effect, then checked. Nothing is stored or sent before this passes.
pub fn admit(effective: &NoesisSettings, bundle: &EvidenceBundle) -> Result<usize, EvidenceError> {
    if !in_effect(effective) {
        return Err(EvidenceError::Disabled);
    }
    validate(bundle)
}

pub fn kind_name(kind: EvidenceRefKind) -> &'static str {
    match kind {
        EvidenceRefKind::Pr => "pr",
        EvidenceRefKind::Issue => "issue",
        EvidenceRefKind::Item => "item",
        EvidenceRefKind::Run => "run",
        EvidenceRefKind::Stage => "stage",
        EvidenceRefKind::Commit => "commit",
        EvidenceRefKind::File => "file",
        EvidenceRefKind::Agent => "agent",
    }
}

pub fn kind_of(name: &str) -> Option<EvidenceRefKind> {
    let all = [
        EvidenceRefKind::Pr,
        EvidenceRefKind::Issue,
        EvidenceRefKind::Item,
        EvidenceRefKind::Run,
        EvidenceRefKind::Stage,
        EvidenceRefKind::Commit,
        EvidenceRefKind::File,
        EvidenceRefKind::Agent,
    ];
    all.into_iter().find(|kind| kind_name(*kind) == name)
}

pub fn status_name(status: EvidenceStatus) -> &'static str {
    match status {
        EvidenceStatus::Verified => "verified",
        EvidenceStatus::Reviewed => "reviewed",
        EvidenceStatus::Preliminary => "preliminary",
    }
}

pub fn status_of(name: &str) -> Option<EvidenceStatus> {
    let all = [
        EvidenceStatus::Verified,
        EvidenceStatus::Reviewed,
        EvidenceStatus::Preliminary,
    ];
    all.into_iter().find(|status| status_name(*status) == name)
}
