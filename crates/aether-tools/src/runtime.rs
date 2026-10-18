//! Batch execution of tool calls with conflict-aware scheduling, timeouts,
//! cancellation and redaction. Emits no events; the agent runtime does that.

use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::ApprovalPolicy;
use aether_core::idempotency::IdempotencyKey;
use aether_core::time::{Timestamp, now};
use aether_core::tool::{SideEffectClass, ToolCall, ToolDescriptor};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::output::Redactor;
use crate::tool::{Tool, ToolContext, ToolError, ToolExecution};

/// Per-batch limits. Shell commands get a longer leash than everything else.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionPolicy {
    pub default_timeout: Duration,
    pub shell_timeout: Duration,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            shell_timeout: Duration::from_secs(120),
        }
    }
}

impl ExecutionPolicy {
    pub fn timeout_for(&self, tool_name: &str) -> Duration {
        if tool_name == "bash" {
            self.shell_timeout
        } else {
            self.default_timeout
        }
    }
}

/// The result of one call in a batch, in the batch's original order.
#[derive(Debug)]
pub struct BatchResult {
    pub call: ToolCall,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub result: Result<ToolExecution, ToolError>,
    /// Retryable failures that were tried again, oldest first. Empty when
    /// the first attempt settled it.
    pub retried: Vec<String>,
    /// Attempts the call's class allows, so a log can say "1 of 3" rather
    /// than restating how many were made.
    pub attempts_allowed: u32,
    /// False when the task executing the tool panicked or was aborted, so
    /// `started_at`/`finished_at` are a synthetic slot-fill, not a
    /// measurement. `true` does not always mean `tool.execute` actually ran,
    /// though: a call cancelled before execution (already interrupted, or
    /// already hard-cancelled) also reports `true` with a ~0 ms synthetic
    /// span - safe only because both of those paths settle as
    /// `ToolError::Cancelled`, which every caller turns into a
    /// `ToolCancelled` event that carries no duration, never a `ToolFailed`
    /// with this synthetic span. A caller must not report a `false` case's
    /// `duration_ms()` as a real (if tiny) elapsed time.
    pub ran: bool,
}

impl BatchResult {
    pub fn duration_ms(&self) -> u64 {
        (self.finished_at - self.started_at)
            .num_milliseconds()
            .max(0) as u64
    }
}

/// Whether a call of this tool needs explicit approval under the policy.
pub fn requires_approval(descriptor: &ToolDescriptor, policy: &ApprovalPolicy) -> bool {
    policy.requires(descriptor.side_effect)
}

#[derive(Debug, Default)]
pub struct ToolRuntime {
    redactor: Redactor,
}

impl ToolRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_redactor(redactor: Redactor) -> Self {
        Self { redactor }
    }

    /// The redactor every tool and engine result passes through before it is recorded.
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// Fill in what the call is about before it is recorded: the resources
    /// it touches, and - for classes whose outcome is kept - its idempotency
    /// key. The agent runtime calls this before `tool.requested` so the event
    /// carries both.
    pub fn prepare(call: &mut ToolCall, tool: &dyn Tool, ctx: &ToolContext) {
        call.resources = tool.resources(&call.input, ctx);
        if tool.descriptor().side_effect.retry_policy().record_key {
            call.idempotency_key = Some(match tool.idempotency_key(&call.input) {
                Some(declared) => IdempotencyKey::declared(&call.name, &declared),
                None => IdempotencyKey::derive(&call.name, &call.input),
            });
        }
    }

    /// Run a batch. Read-only calls run together; a mutating call waits for
    /// every earlier call whose resources conflict with its own. Results come
    /// back in input order regardless of completion order.
    ///
    /// Two stop signals, not one: `interrupt` (a person's Esc, or steering)
    /// cuts an interruptible call short but a non-interruptible one
    /// (`Tool::interruptible` false) ignores it - a decided harness action
    /// always runs. `cancel` is the harder stop, the whole agent ending; even
    /// a non-interruptible call must not outlive that.
    /// `interrupt` is expected to be a descendant of `cancel` the way the
    /// runtime's own turn tokens are, so cancelling always implies interrupt.
    pub async fn execute_batch(
        &self,
        calls: Vec<(ToolCall, Arc<dyn Tool>)>,
        ctx: Arc<ToolContext>,
        interrupt: CancellationToken,
        cancel: CancellationToken,
        policy: &ExecutionPolicy,
    ) -> Vec<BatchResult> {
        let n = calls.len();
        let mut prepared: Vec<(ToolCall, Arc<dyn Tool>, SideEffectClass)> = Vec::with_capacity(n);
        for (mut call, tool) in calls {
            if call.resources.is_empty() {
                Self::prepare(&mut call, tool.as_ref(), &ctx);
            }
            let class = tool.descriptor().side_effect;
            prepared.push((call, tool, class));
        }

        let (txs, rxs): (Vec<_>, Vec<_>) = (0..n).map(|_| watch::channel(false)).unzip();
        let mut set = JoinSet::new();
        for (i, (call, tool, class)) in prepared.iter().enumerate() {
            let deps: Vec<watch::Receiver<bool>> = prepared[..i]
                .iter()
                .enumerate()
                .filter(|(_, (other, _, other_class))| {
                    !call.can_run_concurrently_with(*class, other, *other_class)
                })
                .map(|(j, _)| rxs[j].clone())
                .collect();
            set.spawn(Self::run_one(
                i,
                call.clone(),
                tool.clone(),
                deps,
                ctx.clone(),
                interrupt.clone(),
                cancel.clone(),
                policy.timeout_for(&call.name),
                self.redactor.clone(),
            ));
        }
        // Senders live here so receivers see "done" as each task flips its flag.
        let txs: Vec<watch::Sender<bool>> = txs;
        let mut slots: Vec<Option<BatchResult>> = (0..n).map(|_| None).collect();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((i, result)) => {
                    let _ = txs[i].send(true);
                    slots[i] = Some(result);
                }
                Err(e) => tracing::error!(error = %e, "tool task panicked or was aborted"),
            }
        }
        // A task that panicked leaves a slot empty; report it as an internal error.
        slots
            .into_iter()
            .enumerate()
            .map(|(i, slot)| {
                slot.unwrap_or_else(|| BatchResult {
                    call: prepared[i].0.clone(),
                    started_at: now(),
                    finished_at: now(),
                    result: Err(ToolError::Internal("tool task did not complete".into())),
                    retried: Vec::new(),
                    attempts_allowed: prepared[i].2.retry_policy().attempts.max(1),
                    ran: false,
                })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_one(
        index: usize,
        call: ToolCall,
        tool: Arc<dyn Tool>,
        deps: Vec<watch::Receiver<bool>>,
        ctx: Arc<ToolContext>,
        interrupt: CancellationToken,
        cancel: CancellationToken,
        timeout: Duration,
        redactor: Redactor,
    ) -> (usize, BatchResult) {
        for mut dep in deps {
            // A dropped sender means that call is over; either way we may proceed.
            let _ = dep.wait_for(|done| *done).await;
        }
        let started_at = now();
        // One deadline for the whole call, not one per attempt: the caller
        // was promised `timeout`, and retrying must not multiply it.
        let deadline = tokio::time::Instant::now() + timeout;
        // A read is repeated freely; a call that can compound its effect is
        // not, so the class - not the error - decides how often we try.
        let attempts = tool.descriptor().side_effect.retry_policy().attempts.max(1);
        let mut retried = Vec::new();
        let mut result = Err(ToolError::Cancelled);
        let interruptible = tool.interruptible();
        for attempt in 1..=attempts {
            result = if interruptible && interrupt.is_cancelled() {
                Err(ToolError::Cancelled)
            } else if interruptible {
                let child = interrupt.child_token();
                tokio::select! {
                    res = tool.execute(call.input.clone(), &ctx, child.clone()) => res,
                    _ = interrupt.cancelled() => { child.cancel(); Err(ToolError::Cancelled) }
                    _ = tokio::time::sleep_until(deadline) => { child.cancel(); Err(ToolError::Timeout) }
                }
            } else if cancel.is_cancelled() {
                // A decided harness action ignores a mere interrupt (see
                // `Tool::interruptible`) but not the whole agent ending,
                // which would orphan what it started. Skipping it here narrows
                // the race; `Runtime::cancel_live_children` closes what remains.
                Err(ToolError::Cancelled)
            } else {
                // Bound by the timeout and by a hard cancel that lands mid-call,
                // but never by a mere interrupt: a decided harness action
                // always runs once the model committed to it.
                let child = cancel.child_token();
                tokio::select! {
                    res = tool.execute(call.input.clone(), &ctx, child.clone()) => res,
                    _ = cancel.cancelled() => { child.cancel(); Err(ToolError::Cancelled) }
                    _ = tokio::time::sleep_until(deadline) => { child.cancel(); Err(ToolError::Timeout) }
                }
            };
            let out_of_time = tokio::time::Instant::now() >= deadline;
            match &result {
                // Nothing is left of the budget, so another attempt could
                // only time out again; report what actually happened.
                Err(e) if e.is_retryable() && attempt < attempts && !out_of_time => {
                    retried.push(e.to_string())
                }
                _ => break,
            }
        }
        let result = result.map(|exec| match exec {
            ToolExecution::Outcome(mut outcome) => {
                if let std::borrow::Cow::Owned(clean) = redactor.redact(&outcome.content) {
                    outcome.content = clean;
                }
                // `structured` is committed to the event log whole, and an
                // MCP server chooses its content, so the redactor must see
                // it too - not just the text half.
                if let Some(value) = outcome.structured.as_mut() {
                    redact_json(&redactor, value);
                }
                ToolExecution::Outcome(outcome)
            }
            action => action,
        });
        (
            index,
            BatchResult {
                call,
                started_at,
                finished_at: now(),
                result,
                retried,
                attempts_allowed: attempts,
                ran: true,
            },
        )
    }
}

/// Redact every string *value* in a JSON value, in place, at any depth;
/// serde_json's own parse depth limit bounds the recursion. Keys are names a
/// server chose, not content, so they are left as they are.
pub fn redact_json(redactor: &Redactor, value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => {
            if let std::borrow::Cow::Owned(clean) = redactor.redact(text) {
                *text = clean;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json(redactor, item);
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values_mut() {
                redact_json(redactor, field);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
