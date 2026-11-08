//! The boundary between the teaching loop and whatever runs its agents:
//! Aether agents in run `noesis` in the daemon (F3), a scripted faux engine
//! in tests. Every model call goes through [`Engine::step`], and the loop
//! admits each one against the Noesis budget first.

use serde_json::Value;

use super::seal::Sealer;

use crate::budget::{Billing, Spend};
use crate::prompts::Agent;
use crate::tools::ToolDef;

/// The Aether run every Noesis agent belongs to.
pub const NOESIS_RUN: &str = "noesis";
/// The persisted `agents.subsystem` of every Noesis agent.
pub const SUBSYSTEM: &str = "noesis";

/// Everything needed to create one Noesis agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSpec {
    pub run: &'static str,
    pub subsystem: &'static str,
    pub agent: Agent,
    /// `noesis.<agent>`.
    pub role: String,
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,
}

/// A model the engine can run, as Aether's model list reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub billing: Billing,
    pub ready: bool,
}

#[derive(Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Debug omits the raw id, input and an unknown name: each is model text
/// that may repeat a sealed value before the loop has checked it.
impl std::fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known = crate::tools::ToolName::parse(&self.name);
        f.debug_struct("ToolCall")
            .field("id_chars", &self.id.chars().count())
            .field("tool", &known.map(crate::tools::ToolName::as_str))
            .finish_non_exhaustive()
    }
}

/// A tool call's result for the engine. `call` is the engine's own raw id,
/// kept only to correlate the result; Debug omits it.
#[derive(Clone, PartialEq)]
pub struct ToolResult {
    pub call: String,
    pub content: Value,
    pub is_error: bool,
}

impl std::fmt::Debug for ToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolResult")
            .field("call_chars", &self.call.chars().count())
            .field("content", &self.content)
            .field("is_error", &self.is_error)
            .finish()
    }
}

/// What a Noesis agent is given. Inputs are persisted before inference, so
/// an input whose turn failed is still there for the retry.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The learner's own message.
    Learner(String),
    /// A session's first input: its goal, practice targets and any bundle.
    Opening(String),
    /// The probe assessment handed to a new Teacher.
    Handoff(String),
    /// Results of tool calls the loop ran in the agent's last step.
    ToolResults(Vec<ToolResult>),
    /// A learner's answer: the result of the parked `noesis.ask` call.
    Answer(Value),
}

/// One model call's output: visible text and a batch of tool calls.
#[derive(Clone, PartialEq, Default)]
pub struct Step {
    /// The reply's text blocks run together, as the step shows them.
    pub text: String,
    /// The same text block by block, as a transcript keeps them; empty when
    /// the engine keeps no blocks of its own.
    pub text_blocks: Vec<String>,
    pub calls: Vec<ToolCall>,
    pub usage: Spend,
}

/// Debug omits the text, which may hold a sealed answer until it is checked.
impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("text_chars", &self.text.chars().count())
            .field("text_blocks", &self.text_blocks.len())
            .field("calls", &self.calls)
            .field("usage", &self.usage)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// Nothing reached a model, so nothing is charged.
    #[error("the engine sent nothing: {0}")]
    NotSent(String),
    /// The call may have reached a model; its estimate is charged.
    #[error("the engine failed: {0}")]
    Failed(String),
    /// The Noesis budget refused the call, so nothing reached a model.
    #[error("{0}")]
    BudgetSpent(String),
}

pub trait Engine {
    fn models(&self) -> Vec<ModelInfo>;
    /// Create an agent; returns its Aether agent id.
    fn spawn(&mut self, spec: AgentSpec) -> Result<String, EngineError>;
    /// Persist an input for the agent's next step; the transcript seq it was
    /// recorded at, or `None` when it added nothing. A failed delivery may be
    /// repeated, so a tool result or answer seen twice (same id) is ignored.
    /// Inputs are the agent's private context, sealed only against what is
    /// committed when they go: a host publishes turn reports, never them.
    fn deliver(&mut self, agent: &str, input: Input) -> Result<Option<u64>, EngineError>;
    /// The estimated spend of the agent's next step, admitted before it runs.
    fn estimate(&self, agent: &str) -> Spend;
    /// Whether [`Engine::step`] admits and settles each call against the
    /// Noesis budget itself (the daemon's runtime charge), so the loop must not.
    fn charges_own_calls(&self) -> bool {
        false
    }
    /// Run one model step over the agent's pending inputs. Its reply is
    /// buffered whole, never streamed, and kept only as
    /// [`super::seal::kept_reply`] decides against `live`, the secrets the
    /// session holds unrevealed as the step starts.
    fn step(&mut self, agent: &str, live: &Sealer) -> Result<Step, EngineError>;
}
