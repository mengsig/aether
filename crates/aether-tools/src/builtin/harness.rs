//! Harness tools: the model proposes a change to authoritative state, the
//! runtime validates and records it. These never touch the world directly.

use std::sync::Arc;

use aether_core::tool::{ResourceKey, SideEffectClass, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::tool::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, parse_input};

mod delegation;
mod memory;
mod providers;
mod tasks;

pub use delegation::{CancelAgent, Delegate, IntegrateWorkspace, SendMessage};
pub use memory::Remember;
pub use providers::RefreshProviderUsage;
pub use tasks::{CompleteTask, FailTask, WaitFor};

pub fn all() -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(super::schedules::ScheduleTool::Create),
        Arc::new(super::schedules::ScheduleTool::List),
        Arc::new(super::schedules::ScheduleTool::Update),
        Arc::new(InspectState),
        Arc::new(ListModels),
        Arc::new(ProviderUsage),
        Arc::new(RefreshProviderUsage),
        Arc::new(Delegate),
        Arc::new(IntegrateWorkspace),
        Arc::new(SendMessage),
        Arc::new(CancelAgent),
        Arc::new(CompleteTask),
        Arc::new(FailTask),
        Arc::new(WaitFor),
        Arc::new(Remember),
        Arc::new(RequestCheckpoint),
        Arc::new(AskUser),
    ];
    tools.extend(super::inspection::all());
    tools
}

fn harness_descriptor(name: &str, description: &str, schema: Value) -> ToolDescriptor {
    super::descriptor(name, description, schema, SideEffectClass::ReadOnly, false)
}

fn parse_id<T: std::str::FromStr<Err = aether_core::id::IdParseError>>(
    raw: &str,
    what: &str,
) -> Result<T, ToolError> {
    raw.parse()
        .map_err(|e| ToolError::InvalidInput(format!("invalid {what} `{raw}`: {e}")))
}

// Read-only queries still go through the runtime, which owns model and task state.
#[derive(Debug)]
pub struct InspectState;

#[async_trait]
impl Tool for InspectState {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "inspect_state",
            "Read your Aether model, effort, task, context usage, budget and compaction policy. Use this for questions about the current harness, not native engine documentation.",
            json!({"type":"object", "properties":{}, "additionalProperties":false}),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {}
        let _: Input = parse_input(input)?;
        Ok(ToolExecution::Action(HarnessAction::InspectState))
    }
}

#[derive(Debug)]
pub struct ProviderUsage;

#[async_trait]
impl Tool for ProviderUsage {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "provider_usage",
            "Read cached quota headroom, reset times and age for every enabled provider. Never probes.",
            json!({"type":"object", "properties":{}, "additionalProperties":false}),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {}
        let _: Input = parse_input(input)?;
        Ok(ToolExecution::Action(HarnessAction::ProviderUsage))
    }
}

#[derive(Debug)]
pub struct ListModels;

#[async_trait]
impl Tool for ListModels {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "list_models",
            "Discover Aether model identifiers before choosing a model. Choose available=true: configured reports credentials, while available also checks the engine and your network policy. For family names such as Sonnet or Luna, omit provider. Filtered results include available alternatives. Credentials are never returned.",
            json!({"type":"object", "properties":{"provider":{"type":"string"}}, "additionalProperties":false}),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            provider: Option<String>,
        }
        let Input { provider } = parse_input(input)?;
        if provider.as_ref().is_some_and(|p| p.trim().is_empty()) {
            return Err(ToolError::InvalidInput("provider must not be empty".into()));
        }
        Ok(ToolExecution::Action(HarnessAction::ListModels {
            provider,
        }))
    }
}

// ----- request_checkpoint / ask_user ---------------------------------------

#[derive(Debug)]
pub struct RequestCheckpoint;

#[async_trait]
impl Tool for RequestCheckpoint {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "request_checkpoint",
            "Ask the harness to compact your older context into a structured checkpoint now, \
             for example before a handoff or when the conversation has grown long.",
            json!({
                "type": "object",
                "properties": {"reason": {"type": "string"}},
                "required": ["reason"],
                "additionalProperties": false
            }),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    /// A decided, in-memory state transition once the model commits to it
    /// - see `Tool::interruptible`.
    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            reason: String,
        }
        let Input { reason } = parse_input(input)?;
        if reason.trim().is_empty() {
            return Err(ToolError::InvalidInput("reason is empty".into()));
        }
        Ok(ToolExecution::Action(HarnessAction::RequestCheckpoint {
            reason,
        }))
    }
}

#[derive(Debug)]
pub struct AskUser;

#[async_trait]
impl Tool for AskUser {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "ask_user",
            "Ask the human a question and sleep until they answer. Use only when a decision \
             is genuinely theirs; otherwise decide and proceed. Asking ends the turn, so do \
             not also complete or fail the task in it - wait for the answer first.",
            json!({
                "type": "object",
                "properties": {"question": {"type": "string"}},
                "required": ["question"],
                "additionalProperties": false
            }),
        )
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    /// A decided, in-memory state transition once the model commits to it
    /// - see `Tool::interruptible`.
    fn interruptible(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            question: String,
        }
        let Input { question } = parse_input(input)?;
        if question.trim().is_empty() {
            return Err(ToolError::InvalidInput("question is empty".into()));
        }
        Ok(ToolExecution::Action(HarnessAction::AskUser { question }))
    }
}

#[cfg(test)]
mod tests;
