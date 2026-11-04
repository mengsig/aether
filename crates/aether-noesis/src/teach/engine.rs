//! The boundary between the teaching loop and whatever runs its agents:
//! Aether agents in run `noesis` in the daemon (F3), a scripted faux engine
//! in tests. Every model call goes through [`Engine::step`], and the loop
//! admits each one against the Noesis budget first.

use serde_json::Value;

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

/// Debug shows the sealed projection, as every other read path does.
impl std::fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCall")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("input", &crate::tools::seal_input(&self.name, &self.input))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub call: String,
    pub content: Value,
    pub is_error: bool,
}

/// What a Noesis agent is given. Inputs are persisted before inference, so
/// an input whose turn failed is still there for the retry.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The learner's own message, or the session goal.
    Learner(String),
    /// The probe assessment handed to a new Teacher.
    Handoff(String),
    /// Results of tool calls the loop ran in the agent's last step.
    ToolResults(Vec<ToolResult>),
    /// A learner's answer: the result of the parked `noesis.ask` call.
    Answer(Value),
}

/// One model call's output: visible text and a batch of tool calls.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Step {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Spend,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// Nothing reached a model, so nothing is charged.
    #[error("the engine sent nothing: {0}")]
    NotSent(String),
    /// The call may have reached a model; its estimate is charged.
    #[error("the engine failed: {0}")]
    Failed(String),
}

pub trait Engine {
    fn models(&self) -> Vec<ModelInfo>;
    /// Create an agent; returns its Aether agent id.
    fn spawn(&mut self, spec: AgentSpec) -> Result<String, EngineError>;
    /// Persist an input for the agent's next step. A failed delivery may be
    /// repeated, so a tool result or answer seen twice (same id) is ignored.
    fn deliver(&mut self, agent: &str, input: Input) -> Result<(), EngineError>;
    /// The estimated spend of the agent's next step, admitted before it runs.
    fn estimate(&self, agent: &str) -> Spend;
    /// Run one model call over the agent's pending inputs.
    fn step(&mut self, agent: &str) -> Result<Step, EngineError>;
}
