use super::{harness_descriptor, parse_id};
use crate::tool::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, parse_input};
use aether_core::tool::{ResourceKey, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use aether_core::{
    agent::WaitCondition,
    id::{ArtifactId, TaskId},
};
use std::collections::BTreeSet;

// ----- complete_task / fail_task -------------------------------------------

/// Largest `summary` or `error`, in bytes: far above a review or ship report
/// and still a single bounded read for the parent.
pub const MAX_RESULT_TEXT_BYTES: usize = 256 * 1024;
/// Largest `structured` object, serialized, in bytes.
pub const MAX_STRUCTURED_BYTES: usize = 1024 * 1024;

fn check_text_len(field: &str, text: &str) -> Result<(), ToolError> {
    if text.len() > MAX_RESULT_TEXT_BYTES {
        return Err(ToolError::InvalidInput(format!(
            "{field} is {} bytes; the limit is {MAX_RESULT_TEXT_BYTES}. Shorten it and keep \
             the detail in an artifact or in `structured`",
            text.len()
        )));
    }
    Ok(())
}

/// `structured` must be an object; an explicit `null` counts as omitted,
/// since models routinely send it for an optional field.
fn check_structured(input: &Value) -> Result<(), ToolError> {
    let Some(structured) = input.get("structured").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    if !structured.is_object() {
        return Err(ToolError::InvalidInput(
            "structured must be a JSON object, or null/omitted to send none".into(),
        ));
    }
    let size = structured.to_string().len();
    if size > MAX_STRUCTURED_BYTES {
        return Err(ToolError::InvalidInput(format!(
            "structured is {size} bytes; the limit is {MAX_STRUCTURED_BYTES}. Store the bulk \
             in an artifact and reference it"
        )));
    }
    Ok(())
}

#[derive(Debug)]
pub struct CompleteTask;

#[async_trait]
impl Tool for CompleteTask {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "complete_task",
            "Mark your current task complete. The summary is what your parent and future \
             turns will see instead of your transcript, so state outcomes, decisions, and \
             where results live. Provide the required `summary` string and optional `structured` data. Call this exactly once when the objective is met.",
            json!({
                "type": "object",
                "properties": {
                    "summary": {"type": "string", "description": "Durable summary of what was done and where."},
                    "artifacts": {"type": "array", "items": {"type": "string"}, "description": "Artifact ids (art_...) to attach."},
                    "structured": {"type": "object", "description": "Optional machine-readable result; null or omitted sends none."}
                },
                "required": ["summary"],
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
        check_structured(&input)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            summary: String,
            #[serde(default)]
            artifacts: Vec<String>,
            #[serde(default)]
            structured: Option<Value>,
        }
        let Input {
            summary,
            artifacts,
            structured,
        } = parse_input(input)?;
        if summary.trim().is_empty() {
            return Err(ToolError::InvalidInput("summary is empty".into()));
        }
        check_text_len("summary", &summary)?;
        let artifacts = artifacts
            .iter()
            .map(|a| parse_id::<ArtifactId>(a, "artifact id"))
            .collect::<Result<_, _>>()?;
        Ok(ToolExecution::Action(HarnessAction::CompleteTask {
            summary,
            artifacts,
            structured,
        }))
    }
}

#[derive(Debug)]
pub struct FailTask;

#[async_trait]
impl Tool for FailTask {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "fail_task",
            "Mark your current task failed when the objective cannot be met. Explain what was \
             tried and what blocks completion so your parent can decide what to do. Optional \
             `structured` data reaches the parent alongside the error.",
            json!({
                "type": "object",
                "properties": {
                    "error": {"type": "string"},
                    "structured": {"type": "object", "description": "Optional machine-readable failure detail; null or omitted sends none."}
                },
                "required": ["error"],
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
        check_structured(&input)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            error: String,
            #[serde(default)]
            structured: Option<Value>,
        }
        let Input { error, structured } = parse_input(input)?;
        if error.trim().is_empty() {
            return Err(ToolError::InvalidInput("error is empty".into()));
        }
        check_text_len("error", &error)?;
        Ok(ToolExecution::Action(HarnessAction::FailTask {
            error,
            structured,
        }))
    }
}

// ----- wait_for ------------------------------------------------------------

#[derive(Debug)]
pub struct WaitFor;

#[async_trait]
impl Tool for WaitFor {
    fn descriptor(&self) -> ToolDescriptor {
        harness_descriptor(
            "wait_for",
            "Sleep until tasks finish, a message arrives, or a timeout passes. You consume no \
             tokens while waiting; the runtime wakes you with the results in your inbox. With \
             no arguments you wait for any inbox item. Omit timeouts for ordinary task or \
             message waits; use a timed wake only for a required check-in or deadline. \
             With live native background jobs, the CLI session stays attached and a native \
             completion notification may also end the wait; no polling is needed. \
             A bare wait with nothing pending on an assigned task is refused for non-long-lived agents.",
            json!({
                "type": "object",
                "properties": {
                    "tasks": {"type": "array", "items": {"type": "string"}, "description": "Copy task IDs (`tsk_...`) exactly from delegate.task_id or combine delegate.wait_for_args.tasks. Agent and task IDs are independent: never change an agt_ prefix to tsk_."},
                    "any": {"type": "boolean", "default": false, "description": "Wake when any listed task finishes instead of all."},
                    "at_least": {"type": "integer", "minimum": 1, "description": "Wake when this many distinct listed tasks have finished. Requires tasks; must be between 1 and the number of distinct task IDs."},
                    "message_from": {"type": "string", "description": "Agent ID (`agt_...`) from delegate.agent_id or an Aether inbox sender whose message wakes you."},
                    "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": 31536000, "description": "Wake after this long regardless (up to one year)."},
                    "wake_at": {"type": "string", "format": "date-time", "description": "Wake at this future RFC3339 timestamp, including timezone. Mutually exclusive with timeout_seconds. Persisted across daemon restarts; messages may wake you earlier."}
                },
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
            #[serde(default)]
            tasks: Vec<String>,
            #[serde(default)]
            any: bool,
            #[serde(default)]
            at_least: Option<usize>,
            #[serde(default)]
            message_from: Option<String>,
            #[serde(default)]
            timeout_seconds: Option<u64>,
            #[serde(default)]
            wake_at: Option<String>,
        }
        let Input {
            tasks,
            any,
            at_least,
            message_from,
            timeout_seconds,
            wake_at,
        } = parse_input(input)?;
        let ids: BTreeSet<TaskId> = tasks
            .iter()
            .map(|t| parse_id(t, "task id"))
            .collect::<Result<_, _>>()?;
        if let Some(n) = at_least {
            if ids.is_empty() {
                return Err(ToolError::InvalidInput(format!(
                    "at_least={n} requires distinct task IDs in tasks; supply tasks from delegate.task_id or omit at_least for a message or timer wait"
                )));
            }
            if n == 0 || n > ids.len() {
                return Err(ToolError::InvalidInput(format!(
                    "at_least={n} must be between 1 and {} (the number of distinct task IDs in tasks)",
                    ids.len()
                )));
            }
        }
        let task_condition = if ids.is_empty() {
            None
        } else if let Some(n) = at_least {
            Some(WaitCondition::AtLeast {
                n,
                of: ids
                    .into_iter()
                    .map(|id| WaitCondition::Task { id })
                    .collect(),
            })
        } else if any {
            Some(WaitCondition::any_task(ids))
        } else {
            Some(WaitCondition::all_tasks(ids))
        };
        let sender = message_from
            .map(|a| parse_id(&a, "agent id").map(|agent| WaitCondition::MessageFrom { agent }))
            .transpose()?;
        // Tasks and a sender are both reasons to wake: either one is enough.
        let work = match (task_condition, sender) {
            (Some(t), Some(m)) => Some(WaitCondition::Any { of: vec![t, m] }),
            (Some(t), None) => Some(t),
            (None, Some(m)) => Some(m),
            (None, None) => None,
        };
        if timeout_seconds.is_some() && wake_at.is_some() {
            return Err(ToolError::InvalidInput(
                "use either timeout_seconds or wake_at, not both".into(),
            ));
        }
        let now = aether_core::time::now();
        let at = match (timeout_seconds, wake_at) {
            (Some(seconds), _) => {
                if !(1..=31_536_000).contains(&seconds) {
                    return Err(ToolError::InvalidInput(
                        "timeout_seconds must be between 1 and 31536000".into(),
                    ));
                }
                Some(now + chrono::Duration::seconds(seconds as i64))
            }
            (_, Some(raw)) => {
                let at = chrono::DateTime::parse_from_rfc3339(&raw)
                    .map_err(|e| ToolError::InvalidInput(format!("invalid wake_at: {e}")))?
                    .with_timezone(&chrono::Utc);
                if at <= now || at > now + chrono::Duration::days(365) {
                    return Err(ToolError::InvalidInput(
                        "wake_at must be in the future and within one year".into(),
                    ));
                }
                Some(at)
            }
            _ => None,
        };
        let deadline = at.map(|at| WaitCondition::Deadline { at });
        let condition = match (work, deadline) {
            (Some(t), Some(d)) => WaitCondition::Any { of: vec![t, d] },
            (Some(t), None) => t,
            (None, Some(d)) => WaitCondition::Any {
                of: vec![WaitCondition::Inbox, d],
            },
            (None, None) => WaitCondition::Inbox,
        };
        Ok(ToolExecution::Action(HarnessAction::WaitFor { condition }))
    }
}
