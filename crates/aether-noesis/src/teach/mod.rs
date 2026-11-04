//! The teaching loop: sessions and their phases (probe, plan, teach,
//! practice), the Prober, Teacher and Practitioner agents, and the handlers
//! of the Noesis tools. F3 wires [`Teaching`] into the daemon.

mod db;
pub mod engine;
mod handlers;
pub mod phase;
pub mod plan;
pub mod question;
pub mod records;
mod seal;
mod service;
mod sessions;
mod turn;

#[cfg(test)]
mod faux;
#[cfg(test)]
mod parity;
#[cfg(test)]
mod recovery;
#[cfg(test)]
mod regressions;
#[cfg(test)]
mod testkit;

pub use engine::{
    AgentSpec, Engine, EngineError, Input, NOESIS_RUN, SUBSYSTEM, Step, ToolCall, ToolResult,
};
pub use seal::{REFUSAL_WITHHELD, WITHHELD};
pub use service::{CallRecord, Caller, StepRecord, Teaching, TurnEnd, TurnReport};
pub use sessions::{SessionPage, Undelivered};

use crate::budget::{ModelRefusal, Refusal};
use crate::prompts::{Agent, PromptError};

/// The fixed reply to a successful `noesis.probe_done`.
pub const PROBE_DONE_ACK: &str = "Probe recorded. The Teacher takes over from here.";

/// The Aether role of a Noesis agent.
pub fn role_of(agent: Agent) -> String {
    format!("noesis.{}", agent.name())
}

/// The Noesis agent a role names, or `None` for any non-Noesis role.
pub fn agent_for_role(role: &str) -> Option<Agent> {
    let name = role.strip_prefix("noesis.")?;
    Agent::ALL.into_iter().find(|agent| agent.name() == name)
}

#[derive(Debug, thiserror::Error)]
pub enum TeachError {
    /// The caller's input is not acceptable; nothing changed.
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("no {what} `{id}`")]
    NotFound { what: &'static str, id: String },
    /// The request does not fit the session's state; nothing changed.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("refused before any model call: {0}")]
    Budget(#[from] Refusal),
    #[error(transparent)]
    Model(#[from] ModelRefusal),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Prompt(#[from] PromptError),
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("noesis.db holds a malformed record: {0}")]
    Corrupt(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_noesis_roles_and_round_trip() {
        for agent in Agent::ALL {
            let role = role_of(agent);
            assert!(role.starts_with("noesis."));
            assert_eq!(agent_for_role(&role), Some(agent));
        }
        assert_eq!(agent_for_role("prober"), None);
        assert_eq!(agent_for_role("noesis.extractor"), None);
        assert_eq!(agent_for_role("worker"), None);
    }
}
