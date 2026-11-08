//! Knowledge-graph extraction (NOESIS.md §4, parity X1–X8): the uncovered
//! teacher prose of a session, or an imported source, is batched without
//! splitting a message; an extractor submits findings, an auditor links them
//! to existing propositions, and the pure [`crate::graph::merge`] folds them
//! in. Each unit commits its merge and coverage in one transaction.
//!
//! Extraction reads only what [`Corpus`] hands it (persisted, sealed
//! transcripts and imported sources) and never the learner tables.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_core::model::Usage;

use crate::budget::{Accountant, Admission, Lane, Spend};
use crate::graph::store::GraphError;

pub mod batch;
mod candidates;
mod endpoints;
pub mod engineering;
pub mod findings;
pub mod jobs;
pub mod prompt;
mod run;
pub mod trigger;

#[cfg(test)]
mod faux;
#[cfg(test)]
mod findings_tests;
#[cfg(test)]
mod tests;

pub use batch::Message;
pub use run::{JobEvent, JobPorts, Slot, Slots, claim_next, run_job};
pub use trigger::after_teaching_turn;

/// Who teaches a session, for reading its prose from that agent's transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOrigin {
    pub agent: String,
    /// Practice sessions never write to the graph.
    pub practice: bool,
}

/// `session`'s teaching agent and mode, or `None` when there is no such session.
pub fn session_origin(
    conn: &rusqlite::Connection,
    session: &str,
) -> Result<Option<SessionOrigin>, ExtractError> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT agent, mode FROM sessions WHERE id = ?1",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((agent, mode)) = row else {
        return Ok(None);
    };
    assert!(!agent.is_empty(), "the schema requires a session agent");
    assert!(
        matches!(mode.as_str(), "teach" | "practice"),
        "the schema bounds the mode"
    );
    Ok(Some(SessionOrigin {
        agent,
        practice: mode == "practice",
    }))
}

/// The background agent a model call is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Extractor,
    Auditor,
}

impl Role {
    /// The Aether role the call runs under; both are background roles.
    pub fn aether_role(self) -> &'static str {
        match self {
            Role::Extractor => "noesis.extractor",
            Role::Auditor => "noesis.auditor",
        }
    }
}

/// One model call's answer: its visible text and what it cost.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// Nothing reached a model, so nothing is charged.
    #[error("the extraction model sent nothing: {0}")]
    NotSent(String),
    /// The call may have reached a model.
    #[error("the extraction model failed: {0}")]
    Failed(String),
}

/// Whatever runs the extractor and auditor: Aether agents in run `noesis` in
/// the daemon, a scripted faux model in tests.
pub trait ExtractModel {
    fn invoke(&mut self, role: Role, prompt: &str) -> Result<Reply, ModelError>;
}

/// The background budget every extraction model call is charged to.
pub trait BackgroundBudget {
    /// Asked before every model call with its estimated spend: `Some(reason)`
    /// refuses the call, and nothing is sent.
    fn before_invocation(&mut self, estimate: Spend) -> Option<String>;
    /// What the call admitted last actually cost.
    fn settle(&mut self, spent: Spend);
}

/// The background budget as an [`Accountant`] keeps it: each call is
/// admitted on the background lane, whose cap and daily dollar and token
/// budgets refuse it before it is made.
#[derive(Debug)]
pub struct AccountedBudget {
    accountant: Accountant,
    admission: Option<Admission>,
}

impl AccountedBudget {
    pub fn new(accountant: Accountant) -> Self {
        Self {
            accountant,
            admission: None,
        }
    }
}

impl BackgroundBudget for AccountedBudget {
    fn before_invocation(&mut self, estimate: Spend) -> Option<String> {
        assert!(self.admission.is_none(), "one call at a time per job");
        let today = chrono::Utc::now().date_naive();
        match self.accountant.admit(Lane::Background, estimate, today) {
            Ok(admission) => {
                debug_assert_eq!(admission.lane(), Lane::Background);
                self.admission = Some(admission);
                None
            }
            Err(refusal) => Some(refusal.to_string()),
        }
    }

    fn settle(&mut self, spent: Spend) {
        let admission = self.admission.take();
        debug_assert!(admission.is_some(), "settled without an admitted call");
        if let Some(admission) = admission {
            admission.settle(spent);
        }
    }
}

/// The persisted material extraction reads.
pub trait Corpus {
    /// The session's teacher prose, in transcript order, as sealed and stored.
    fn session_prose(&self, session: &str) -> Result<Vec<Message>, String>;
    /// The text of an imported source's artifact.
    fn source_text(&self, artifact: &str) -> Result<String, String>;
}

/// Asks a running job to stop at its next model-call boundary; the merge
/// it committed last stays.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a call charges: billed dollars plus the subscription estimate.
pub fn spend_of(usage: &Usage) -> Spend {
    let usd = usage.cost_usd + usage.estimated_cost_usd.unwrap_or(0.0);
    Spend {
        usd: if usd.is_finite() { usd.max(0.0) } else { 0.0 },
        tokens: usage.total_tokens(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("no job `{0}`")]
    UnknownJob(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Invalid(String),
}
