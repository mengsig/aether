//! The turn loop: model steps admitted against the budget, their tool calls
//! run, results delivered in order even after a failed delivery, and the
//! whole turn sealed once at its end.

use std::collections::HashMap;

use serde_json::Value;

use super::db;
use super::engine::{Engine, EngineError, Input, Step, ToolResult};
use super::history;
use super::records::{NoesisEvent, SessionStall, StallReason};
use super::seal::{self, Sealer};
use super::service::{
    CallRecord, Caller, MAX_STEPS, StepRecord, Teaching, TurnEnd, TurnReport, driving_agent,
};
use super::{TeachError, question, role_of};
use crate::budget::{INTERACTIVE_BUDGET_SETTING, Lane, Refusal};
use crate::tools::SEALED;

impl<E: Engine> Teaching<'_, E> {
    /// Run the session's agent until it waits, and on a hand-off run the new
    /// Teacher's first turn too. A failed turn leaves its inputs persisted,
    /// so calling this again recovers; one the budget refused stalls the session.
    pub fn advance(&mut self, session: &str) -> Result<Vec<TurnReport>, TeachError> {
        let ran = self.run_turns(session);
        // A store failure here outranks the turn's own error, which recurs.
        match ran.as_ref().err().and_then(budget_stall) {
            Some(message) => self.stall_session(session, message)?,
            None => {
                history::clear_stall(self.conn(), session)?;
            }
        }
        ran
    }

    fn run_turns(&mut self, session: &str) -> Result<Vec<TurnReport>, TeachError> {
        let first = self.run_turn(session)?;
        let handed_off = first.end == TurnEnd::HandedOff;
        let mut reports = vec![first];
        if handed_off {
            reports.push(self.run_turn(session)?);
        }
        debug_assert!(reports.iter().skip(1).all(|r| r.end != TurnEnd::HandedOff));
        Ok(reports)
    }

    /// Record why the session's turn could not run, and announce it.
    fn stall_session(&mut self, session: &str, message: String) -> Result<(), TeachError> {
        debug_assert!(
            message.contains(INTERACTIVE_BUDGET_SETTING),
            "it names the setting"
        );
        let stall = SessionStall {
            reason: StallReason::BudgetExhausted,
            message,
            at: db::now(),
        };
        history::set_stall(self.conn(), session, &stall)?;
        self.events.push(NoesisEvent::SessionStalled {
            session: session.to_string(),
            stall,
        });
        Ok(())
    }

    /// One turn of the session's current agent: model calls, each admitted
    /// against the interactive budget, until the agent waits. A session with
    /// an open question waits for the learner and makes no model call.
    ///
    /// Its steps and events are published once it ends, sealed together
    /// against the secrets left then. Only its text streams before, which
    /// the live seal and the turn-wide pose check account for.
    pub fn run_turn(&mut self, session: &str) -> Result<TurnReport, TeachError> {
        let row = db::session(self.conn(), session)?;
        let caller = Caller {
            agent: row.agent.clone(),
            role: role_of(driving_agent(&row)),
        };
        let mut turn = Turn::new(self.events.len());
        let ran = self.drive_turn(session, &caller, &mut turn);
        let sealer = match self.turn_sealer(session, &turn) {
            Ok(sealer) => sealer,
            Err(error) => {
                // Without the secrets nothing can be sealed, so nothing is published.
                self.events.truncate(turn.events_from);
                return Err(error);
            }
        };
        for event in &mut self.events[turn.events_from..] {
            sealer.event(event);
        }
        let end = ran?;
        let steps = turn.seal(&sealer);
        debug_assert!(end != TurnEnd::StepLimit || steps.len() == MAX_STEPS);
        Ok(TurnReport {
            agent: caller.agent,
            steps,
            end,
        })
    }

    fn drive_turn(
        &mut self,
        session: &str,
        caller: &Caller,
        turn: &mut Turn,
    ) -> Result<TurnEnd, TeachError> {
        self.deliver_queued(session, &caller.agent, None)?;
        if db::open_question(self.conn(), session)?.is_some() {
            return Ok(TurnEnd::AwaitingAnswer);
        }
        for _ in 0..MAX_STEPS {
            let step = self.model_step(session, &caller.agent)?;
            let (end, results) = self.run_calls(session, caller, step, turn)?;
            if !results.is_empty() {
                let input = Input::ToolResults(results);
                self.deliver_queued(session, &caller.agent, Some(input))?;
            }
            if let Some(end) = end {
                return Ok(end);
            }
        }
        debug_assert_eq!(turn.steps.len(), MAX_STEPS);
        Ok(TurnEnd::StepLimit)
    }

    /// Run a step's calls and buffer the step raw; return how the turn ends,
    /// if it does, and the results owed to the engine.
    fn run_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        step: Step,
        turn: &mut Turn,
    ) -> Result<(Option<TurnEnd>, Vec<ToolResult>), TeachError> {
        let (own, unreadable) = seal::step_secrets(&step);
        let mut pending = PendingStep {
            fail_closed: unreadable,
            text: step.text,
            calls: Vec::with_capacity(step.calls.len()),
        };
        turn.own.extend(own.iter().cloned());
        if step.calls.is_empty() {
            turn.steps.push(pending);
            return Ok((Some(TurnEnd::AwaitingLearner), Vec::new()));
        }
        // Text streams as it is written, so a pose is checked against all the
        // turn has shown, not only its own step's text.
        let shown = turn.shown_with(&pending.text);
        let mut end = None;
        let mut owed = Vec::new();
        for call in step.calls {
            let outcome = self.call_tool(session, caller, &call, &shown)?;
            let public = turn.public_id(call.id);
            if outcome.ends.is_none() {
                owed.push(pending.calls.len());
            }
            end = end.or(outcome.ends);
            pending.calls.push(PendingCall {
                id: public,
                name: call.name,
                input: call.input,
                result: outcome.result.content,
                is_error: outcome.result.is_error,
            });
        }
        let results = self.engine_results(session, &own, &pending, &owed, turn)?;
        turn.steps.push(pending);
        Ok((end, results))
    }

    /// The results the engine is owed for a step, keyed by its own call ids
    /// and sealed against the secrets live after the step and its own.
    fn engine_results(
        &self,
        session: &str,
        own: &[String],
        step: &PendingStep,
        owed: &[usize],
        turn: &Turn,
    ) -> Result<Vec<ToolResult>, TeachError> {
        let live = self.live_secrets(session)?;
        let committed = Sealer::new(&live);
        let all = Sealer::new(live.iter().chain(own));
        let mut results = Vec::with_capacity(owed.len());
        for &index in owed {
            let call = &step.calls[index];
            let mut content = call.result.clone();
            // A refused call committed nothing, so only committed secrets
            // seal its reason and the agent still reads what to fix.
            if call.is_error {
                committed.refusal(&mut content);
            } else {
                all.value(&mut content);
            }
            results.push(ToolResult {
                call: turn.engine_id(&call.id).to_string(),
                content,
                is_error: call.is_error,
            });
        }
        debug_assert_eq!(results.len(), owed.len());
        Ok(results)
    }

    /// The final secret set of a turn: what is still sealed at its end and
    /// every sealed text a call of the turn carried, posed or refused.
    fn turn_sealer(&self, session: &str, turn: &Turn) -> Result<Sealer, TeachError> {
        let live = self.live_secrets(session)?;
        let sealer = Sealer::new(live.iter().chain(&turn.own));
        debug_assert!(
            live.iter()
                .all(|secret| secret.trim().is_empty() || sealer.holds(secret))
        );
        Ok(sealer)
    }

    /// The session's sealed texts still unrevealed: the active problem's
    /// solution and the open question's key and explanation.
    pub(super) fn live_secrets(&self, session: &str) -> Result<Vec<String>, TeachError> {
        let active = db::active_problem(self.conn(), session)?;
        let open = db::open_question(self.conn(), session)?;
        debug_assert!(
            active.is_none() || open.is_none(),
            "practice poses problems and teaching poses questions, never both"
        );
        let mut secrets: Vec<String> = active.map(|row| row.solution).into_iter().collect();
        if let Some(open) = open {
            secrets.extend(question::shown_key(&open));
            secrets.extend(open.explanation);
        }
        Ok(secrets)
    }

    /// Deliver what is owed to the session's `agent`: the in-memory queue,
    /// then answers the store records as owed, then `input`. On a failure
    /// everything not yet delivered stays owed, in order.
    pub(super) fn deliver_queued(
        &mut self,
        session: &str,
        agent: &str,
        input: Option<Input>,
    ) -> Result<(), TeachError> {
        self.deliver_memory_queue(agent, None)?;
        self.deliver_owed_answers(session, agent)?;
        self.deliver_memory_queue(agent, input)
    }

    /// Deliver answers committed but not yet delivered, oldest first; they
    /// survive a restart because the store, not memory, records them owed.
    fn deliver_owed_answers(&mut self, session: &str, agent: &str) -> Result<(), TeachError> {
        debug_assert!(
            !self.undelivered.contains_key(agent),
            "the queue goes first"
        );
        for owed in db::owed_answers(self.conn(), session)? {
            let mut fields = owed.record;
            fields.remove("delivered");
            fields.insert("question".into(), Value::String(owed.question.clone()));
            if let Some(correct) = owed.correct {
                fields.insert("correct".into(), Value::Bool(correct));
            }
            debug_assert!(fields.contains_key("dont_know"), "an answer record");
            let payload = Value::Object(fields);
            self.engine.deliver(agent, Input::Answer(payload))?;
            db::mark_delivered(self.conn(), &owed.question)?;
        }
        Ok(())
    }

    fn deliver_memory_queue(
        &mut self,
        agent: &str,
        input: Option<Input>,
    ) -> Result<(), TeachError> {
        let mut queue = self.undelivered.remove(agent).unwrap_or_default();
        queue.extend(input);
        while let Some(next) = queue.pop_front() {
            if let Err(error) = self.engine.deliver(agent, next.clone()) {
                queue.push_front(next);
                self.undelivered.insert(agent.to_string(), queue);
                return Err(error.into());
            }
        }
        debug_assert!(!self.undelivered.contains_key(agent), "all delivered");
        Ok(())
    }

    /// One model call, admitted before it is made and settled after, and
    /// sealed against what `session` holds unrevealed as it starts, so no
    /// text holding a live secret is shown.
    fn model_step(&mut self, session: &str, agent: &str) -> Result<Step, TeachError> {
        let live = Sealer::new(self.live_secrets(session)?);
        if self.engine.charges_own_calls() {
            return Ok(self.engine.step(agent, &live)?);
        }
        let today = chrono::Local::now().date_naive();
        let estimate = self.engine.estimate(agent);
        let admission = self
            .noesis
            .accountant()
            .admit(Lane::Interactive, estimate, today)?;
        match self.engine.step(agent, &live) {
            Ok(step) => {
                admission.settle(step.usage);
                Ok(step)
            }
            Err(error @ (EngineError::NotSent(_) | EngineError::BudgetSpent(_))) => {
                admission.abandon_unsent();
                Err(error.into())
            }
            // Dropping the admission charges the estimate: the call may have run.
            Err(error) => Err(error.into()),
        }
    }
}

/// What the learner is told when the teaching budget refused a turn's call.
fn budget_stall(error: &TeachError) -> Option<String> {
    match error {
        TeachError::Budget(
            refusal @ (Refusal::BudgetZero { .. }
            | Refusal::BudgetExhausted { .. }
            | Refusal::WouldExceed { .. }),
        ) => Some(format!(
            "{refusal}; raise {INTERACTIVE_BUDGET_SETTING} to continue today"
        )),
        TeachError::Engine(EngineError::BudgetSpent(message)) => Some(message.clone()),
        _ => None,
    }
}

/// A tool call as the turn ran it, held raw until the turn is sealed.
struct PendingCall {
    /// The opaque public id; the engine's own id stays in [`Turn`].
    id: String,
    name: String,
    input: Value,
    result: Value,
    is_error: bool,
}

struct PendingStep {
    text: String,
    calls: Vec<PendingCall>,
    /// A sealing call's input was unreadable, so its secrets are unknown.
    fail_closed: bool,
}

/// What one turn produced, unpublished until [`Turn::seal`].
struct Turn {
    steps: Vec<PendingStep>,
    /// Every sealed text the turn's calls carried.
    own: Vec<String>,
    /// Public call id to the engine's own id, for delivering results.
    engine_ids: HashMap<String, String>,
    /// Where the turn's events start in the pending event list.
    events_from: usize,
}

/// Debug shows only counts: the buffer holds unsealed model text.
impl std::fmt::Debug for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Turn")
            .field("steps", &self.steps.len())
            .field("secrets", &self.own.len())
            .field("events_from", &self.events_from)
            .finish_non_exhaustive()
    }
}

impl Turn {
    fn new(events_from: usize) -> Self {
        Self {
            steps: Vec::new(),
            own: Vec::new(),
            engine_ids: HashMap::new(),
            events_from,
        }
    }

    /// Every text the turn has shown, its earlier steps' and `current`, one
    /// per line so no phrase is matched across two steps.
    fn shown_with(&self, current: &str) -> String {
        let earlier = self.steps.iter().map(|step| step.text.as_str());
        earlier.chain([current]).collect::<Vec<_>>().join("\n")
    }

    /// A fresh opaque id for an engine call id; only [`Turn::engine_id`]
    /// maps it back.
    fn public_id(&mut self, engine_id: String) -> String {
        let public = db::fresh_id("call");
        let previous = self.engine_ids.insert(public.clone(), engine_id);
        debug_assert!(previous.is_none(), "public ids are fresh");
        public
    }

    fn engine_id(&self, public: &str) -> &str {
        let engine_id = self.engine_ids.get(public);
        assert!(engine_id.is_some(), "every buffered call has an engine id");
        engine_id.map_or("", String::as_str)
    }

    /// The turn's steps as they may be published, sealed by `sealer`.
    fn seal(self, sealer: &Sealer) -> Vec<StepRecord> {
        let mut steps = Vec::with_capacity(self.steps.len());
        for pending in self.steps {
            let text = if pending.fail_closed {
                seal::WITHHELD.to_string()
            } else {
                pending.text
            };
            let calls = pending.calls.into_iter().map(|call| CallRecord {
                id: call.id,
                name: call.name,
                input: call.input,
                result: call.result,
                is_error: call.is_error,
            });
            let mut record = StepRecord {
                text,
                calls: calls.collect(),
            };
            if pending.fail_closed {
                // An unreadable sealing input hides which secrets its step carried.
                for call in &mut record.calls {
                    call.input = Value::String(SEALED.into());
                    if !call.is_error {
                        call.result = Value::String(SEALED.into());
                    }
                }
            }
            sealer.step(&mut record);
            steps.push(record);
        }
        debug_assert!(
            steps
                .iter()
                .all(|step| !sealer.holds(&step.text) || step.text == seal::WITHHELD)
        );
        steps
    }
}
