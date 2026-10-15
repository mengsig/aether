//! A pending MCP reply keeps native background jobs in their owning session.
use aether_core::agent::{WaitCondition, WaitOracle};
use tokio::sync::oneshot;

use super::{Runtime, TurnCtx};

pub(super) struct NativeWait {
    condition: WaitCondition,
    inbox_cursor: u64,
    input: serde_json::Value,
    reply: oneshot::Sender<(String, bool)>,
}

impl NativeWait {
    pub(super) fn new(
        condition: WaitCondition,
        ctx: &TurnCtx,
        reply: oneshot::Sender<(String, bool)>,
        input: serde_json::Value,
    ) -> Self {
        Self {
            condition,
            inbox_cursor: ctx.agent.inbox_cursor,
            input,
            reply,
        }
    }

    pub(super) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.condition.earliest_deadline().map(|at| {
            tokio::time::Instant::now()
                + (at - aether_core::time::now()).to_std().unwrap_or_default()
        })
    }

    pub(super) fn ready(&self, runtime: &Runtime, ctx: &TurnCtx) -> bool {
        if self.reply.is_closed() {
            return true;
        }
        let oracle = crate::wait::RuntimeOracle {
            runtime,
            agent: ctx.agent.id,
            inbox_cursor: self.inbox_cursor,
        };
        oracle.has_unconsumed_inbox() || self.condition.is_satisfied(&oracle)
    }

    pub(super) fn finish(self, reason: &str) -> Option<(serde_json::Value, (String, bool))> {
        // A closed MCP client needs no reply; the engine still owns its jobs.
        if self.reply.is_closed() {
            return None;
        }
        let result = (
            format!(
                "Wait ended: {reason}. Handle this event now: if a job finished, collect its result using the native tools. The native session and any remaining background jobs are still alive. Do not wait again for the job that just finished."
            ),
            false,
        );
        let _ = self.reply.send(result.clone());
        Some((self.input, result))
    }
}
