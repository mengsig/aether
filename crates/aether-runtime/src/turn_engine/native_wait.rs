//! A pending MCP reply keeps native background jobs in their owning session.
use std::sync::Arc;

use aether_core::agent::{WaitCondition, WaitOracle};
use aether_core::id::AgentId;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{Next, Result, Runtime, Steering, ToolEffects, TurnCtx, record_consumed};

/// How long a Codex wait with no job reported yet holds off deciding, so a
/// command started in the same tool batch can report its start first.
pub(super) const JOB_START_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// Why an unheld `wait_for` ended when no job or deadline did.
const UNHELD_WAKE: &str = "a message arrived or the requested condition was satisfied";

/// What met a held wait's condition: its child finished, its check-in time
/// came first, or (for `ask_user`) a message arrived.
fn held_wake(condition: &WaitCondition, oracle: &crate::wait::RuntimeOracle<'_>) -> String {
    let tasks = condition.task_ids();
    if tasks.is_empty() {
        return "a message arrived".into();
    }
    if tasks.iter().all(|task| oracle.task_is_terminal(*task)) {
        return "the delegated task finished".into();
    }
    debug_assert!(condition.earliest_deadline().is_some(), "{condition:?}");
    "the check-in time came; the delegated task is still running".into()
}

/// A bridge call that ends the turn unless native jobs keep it attached.
pub(super) fn parks_turn(name: &str, input: &Value) -> bool {
    match name {
        "wait_for" | "ask_user" => true,
        "delegate" => input.get("wait").and_then(Value::as_bool) != Some(false),
        _ => false,
    }
}

/// A pending native wait calls no model, so its turn's slot goes to the work
/// it waits on, a delegated child included; once the wait is over the turn
/// queues for a slot again, as a resource wait does. `off` says it gave its
/// slot away. A turn that is `ending` needs none back.
pub(super) async fn follow_slot(
    runtime: &Arc<Runtime>,
    agent: AgentId,
    cancel: &CancellationToken,
    (waiting, ending): (bool, bool),
    off: &mut bool,
) {
    if waiting && !*off {
        crate::turn::set_on_provider(runtime, agent, false);
        runtime.notify.notify_one();
        *off = true;
        return;
    }
    if waiting || !*off || ending {
        return;
    }
    *off = false;
    // Stopping the live engine would kill the jobs the wait kept, and going
    // on would break the caps, so the turn queues until a slot or a cancel.
    while let crate::turn::Retake::TimedOut =
        crate::turn::retake_turn_slot(runtime, agent, cancel).await
    {
        tracing::info!(%agent, "still queued for a turn slot after a native wait");
    }
    debug_assert!(!*off, "the slot is taken back or the turn ends");
}

/// An `ask_user` or waiting `delegate` that ran while native jobs did: its
/// reply waits for the answer or the child instead of ending the turn.
struct Held {
    tool: String,
    /// The action's own reply, written for a turn that ends with it.
    content: String,
    asked: bool,
    /// Jobs that finished while the reply waited, oldest first.
    finished: Vec<String>,
}

/// What a released wait answered: the tool, its input and the reply.
pub(super) type Released = (String, Value, (String, bool));

pub(super) struct NativeWait {
    condition: WaitCondition,
    inbox_cursor: u64,
    input: Value,
    reply: oneshot::Sender<(String, bool)>,
    held: Option<Held>,
    /// Why a `wait_for` already ended (its job or deadline); it is answered
    /// at the driver loop's next pass, once the turn is back on a slot.
    ended: Option<String>,
}

impl NativeWait {
    pub(super) fn new(
        condition: WaitCondition,
        ctx: &TurnCtx,
        reply: oneshot::Sender<(String, bool)>,
        input: Value,
    ) -> Self {
        Self {
            condition,
            inbox_cursor: ctx.agent.inbox_cursor,
            input,
            reply,
            held: None,
            ended: None,
        }
    }

    /// Hold the reply `content` of the parking `tool` until `condition`.
    pub(super) fn held(
        condition: WaitCondition,
        ctx: &TurnCtx,
        reply: oneshot::Sender<(String, bool)>,
        input: Value,
        tool: String,
        content: String,
    ) -> Self {
        debug_assert!(matches!(tool.as_str(), "ask_user" | "delegate"), "{tool}");
        debug_assert!(
            !condition.task_ids().is_empty() || condition == WaitCondition::Inbox,
            "a held wait waits on a child or an answer: {condition:?}"
        );
        let asked = tool == "ask_user";
        let mut wait = Self::new(condition, ctx, reply, input);
        wait.held = Some(Held {
            tool,
            content,
            asked,
            finished: Vec::new(),
        });
        wait
    }

    pub(super) fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// Why a call cannot run while this wait is pending: one wait at a time,
    /// and a held wait's task may not be settled under it.
    pub(super) fn refusal(&self, name: &str, input: &Value) -> Option<&'static str> {
        if parks_turn(name, input) {
            return Some("A native background wait is already pending in this session.");
        }
        let settles = matches!(name, "complete_task" | "fail_task");
        (settles && self.is_held()).then_some(
            "An ask_user or delegate wait is still pending in this session: settle the task \
             once it returns.",
        )
    }

    pub(super) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.condition.earliest_deadline().map(|at| {
            tokio::time::Instant::now()
                + (at - aether_core::time::now()).to_std().unwrap_or_default()
        })
    }

    /// What ended the wait, once something did. Inbox items up to
    /// `handed_up_to` already went to the engine and are no news.
    pub(super) fn ready(
        &self,
        runtime: &Runtime,
        ctx: &TurnCtx,
        handed_up_to: u64,
    ) -> Result<Option<String>> {
        if self.reply.is_closed() {
            return Ok(Some("the client went away".into()));
        }
        if let Some(why) = &self.ended {
            return Ok(Some(why.clone()));
        }
        let oracle = crate::wait::RuntimeOracle {
            runtime,
            agent: ctx.agent.id,
            inbox_cursor: self.inbox_cursor,
        };
        if !self.is_held() {
            let woken = oracle.has_unconsumed_inbox() || self.condition.is_satisfied(&oracle);
            return Ok(woken.then(|| UNHELD_WAKE.to_owned()));
        }
        // A held wait ends as its parked turn would wake: on its condition,
        // or on what reaches a waiting agent at once (a person, its parent).
        if self.condition.is_satisfied(&oracle) {
            return Ok(Some(held_wake(&self.condition, &oracle)));
        }
        let mut unheard = ctx.agent.clone();
        unheard.inbox_cursor = unheard.inbox_cursor.max(handed_up_to);
        crate::scheduler::urgent_inbox(runtime, &unheard)
    }

    /// A tracked job finished. A `wait_for` ends on it; a held wait only
    /// records it for its reply.
    pub(super) fn job_finished(&mut self, id: &str) {
        debug_assert!(!id.is_empty());
        match self.held.as_mut() {
            Some(held) => held.finished.push(id.to_owned()),
            None => {
                self.ended
                    .get_or_insert_with(|| format!("native background task {id} finished"));
            }
        }
    }

    /// The `wait_for`'s own deadline passed. A held wait's check-in is part
    /// of its condition instead.
    pub(super) fn deadline_reached(&mut self) {
        debug_assert!(!self.is_held(), "a held wait has no own deadline");
        self.ended
            .get_or_insert_with(|| "the requested deadline was reached".into());
    }

    /// Answer a wait whose condition was met. A held wait hands over the
    /// inbox items that met it, which the engine may have no other way to see.
    /// `what` is why it ended, as [`Self::ready`] said. An engine that takes
    /// no input mid-turn (`steerable` false) gets what arrived in the reply.
    pub(super) fn release(
        mut self,
        what: &str,
        steerable: bool,
        runtime: &Runtime,
        ctx: &mut TurnCtx,
        steering: &mut Steering,
        effects: &mut ToolEffects,
    ) -> Result<Option<Released>> {
        debug_assert!(!what.is_empty(), "a released wait says what ended it");
        // A closed MCP client needs no reply; what arrived stays in the inbox.
        if self.reply.is_closed() {
            return Ok(None);
        }
        // Consuming past a steered item the engine may still refuse would skip
        // it, so while one is unacknowledged what arrived is steered after it.
        let follows = !steering.outstanding.is_empty();
        let pending = if follows || (steerable && self.held.is_none()) {
            None
        } else {
            steering.pending(runtime, ctx)?
        };
        let delivered = pending.as_ref().map(|p| p.message.text());
        let Some(held) = self.held.take() else {
            let finished = self.finish(what, delivered.as_deref());
            if finished.is_some()
                && let Some(pending) = pending
            {
                record_consumed(runtime, ctx, pending)?;
            }
            return Ok(finished.map(|(input, reply)| ("wait_for".to_owned(), input, reply)));
        };
        let result = (
            held_reply(what, &held, delivered.as_deref(), follows),
            false,
        );
        if self.reply.send(result.clone()).is_err() {
            return Ok(None);
        }
        if held.asked {
            effects.question_answered();
        }
        steering.retry |= follows;
        if let Some(pending) = pending {
            record_consumed(runtime, ctx, pending)?;
        }
        Ok(Some((held.tool, self.input, result)))
    }

    /// The last job finished before the condition: nothing is left to keep
    /// attached, so the held action parks the turn as it would have at once.
    pub(super) fn into_park(self, effects: &mut ToolEffects) -> Option<Released> {
        debug_assert!(effects.next.is_none(), "a held wait's turn has no end yet");
        let held = self.held?;
        effects.next = Some(Next::Waiting(self.condition));
        let result = (
            format!(
                "Native background jobs finished while you waited ({}), and none is left \
                 running, so this wait now ends the turn.\n\n{}",
                held.finished.join(", "),
                held.content
            ),
            false,
        );
        self.reply.send(result.clone()).ok()?;
        Some((held.tool, self.input, result))
    }

    /// Answer a `wait_for`, handing over `delivered` when the engine cannot
    /// be steered and would otherwise never see what woke it. `None` when
    /// the reply could not be sent.
    fn finish(self, reason: &str, delivered: Option<&str>) -> Option<(Value, (String, bool))> {
        debug_assert!(!self.is_held(), "a held wait is released, never finished");
        // A closed MCP client needs no reply; the engine still owns its jobs.
        if self.reply.is_closed() {
            return None;
        }
        let mut text = format!(
            "Wait ended: {reason}. Handle this event now: if a job finished, collect its result using the native tools. The native session and any remaining background jobs are still alive. Do not wait again for the job that just finished."
        );
        if let Some(delivered) = delivered {
            text.push_str(&format!("\n\nDelivered with this reply:\n{delivered}"));
        }
        let result = (text, false);
        // Gone since the check above: nothing was delivered, so nothing is consumed.
        if self.reply.send(result.clone()).is_err() {
            return None;
        }
        Some((self.input, result))
    }
}

/// A held action's reply once its wait ended inside the turn: what ended it,
/// the jobs that finished meanwhile, what arrived, then the action's own reply.
fn held_reply(what: &str, held: &Held, delivered: Option<&str>, follows: bool) -> String {
    debug_assert!(!(follows && delivered.is_some()), "delivered twice");
    let mut note = format!("Wait ended: {what}.");
    if !held.finished.is_empty() {
        note.push_str(&format!(
            " Native background jobs that finished meanwhile: {}.",
            held.finished.join(", ")
        ));
    }
    note.push_str(
        " The native session and its remaining background jobs are still alive: continue the turn.",
    );
    if let Some(text) = delivered {
        note.push_str(&format!("\n\nDelivered with this reply:\n{text}"));
    }
    if follows {
        note.push_str(" What arrived follows as your next input.");
    }
    // The action's structured payload stays the reply's last line.
    format!("{note}\n\n{}", crate::turn::unparked_reply(&held.content))
}
