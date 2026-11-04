//! The turn loop: model steps admitted against the budget, their tool calls
//! run, and results delivered in order, even after a failed delivery.

use super::db;
use super::engine::{Engine, EngineError, Input, Step, ToolResult};
use super::service::{
    CallRecord, Caller, MAX_STEPS, StepRecord, Teaching, TurnEnd, TurnReport, driving_agent,
};
use super::{TeachError, question, role_of};
use crate::budget::Lane;
use crate::tools;

/// What a step's visible text reads as once it gave away a sealed answer.
pub const WITHHELD: &str = "<withheld: this message gave away a sealed answer>";

impl<E: Engine> Teaching<'_, E> {
    /// Run the session's agent until it waits, and on a hand-off run the new
    /// Teacher's first turn too. A failed turn leaves its inputs persisted,
    /// so calling this again recovers.
    pub fn advance(&mut self, session: &str) -> Result<Vec<TurnReport>, TeachError> {
        let first = self.run_turn(session)?;
        let handed_off = first.end == TurnEnd::HandedOff;
        let mut reports = vec![first];
        if handed_off {
            reports.push(self.run_turn(session)?);
        }
        debug_assert!(reports.iter().skip(1).all(|r| r.end != TurnEnd::HandedOff));
        Ok(reports)
    }

    /// One turn of the session's current agent: model calls, each admitted
    /// against the interactive budget, until the agent waits. A session with
    /// an open question waits for the learner and makes no model call.
    pub fn run_turn(&mut self, session: &str) -> Result<TurnReport, TeachError> {
        let row = db::session(self.conn(), session)?;
        let caller = Caller {
            agent: row.agent.clone(),
            role: role_of(driving_agent(&row)),
        };
        let mut report = TurnReport {
            agent: caller.agent.clone(),
            steps: Vec::new(),
            end: TurnEnd::StepLimit,
        };
        self.deliver_queued(&caller.agent, None)?;
        if db::open_question(self.conn(), session)?.is_some() {
            report.end = TurnEnd::AwaitingAnswer;
            return Ok(report);
        }
        for _ in 0..MAX_STEPS {
            let step = self.model_step(&caller.agent)?;
            let (record, end, results) = self.run_calls(session, &caller, &step)?;
            report.steps.push(record);
            if !results.is_empty() {
                let input = Input::ToolResults(results);
                self.deliver_queued(&caller.agent, Some(input))?;
            }
            if let Some(end) = end {
                report.end = end;
                return Ok(report);
            }
        }
        debug_assert_eq!(report.steps.len(), MAX_STEPS);
        Ok(report)
    }

    fn run_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        step: &Step,
    ) -> Result<(StepRecord, Option<TurnEnd>, Vec<ToolResult>), TeachError> {
        let mut record = StepRecord {
            text: shown_text(step),
            calls: Vec::with_capacity(step.calls.len()),
        };
        if step.calls.is_empty() {
            return Ok((record, Some(TurnEnd::AwaitingLearner), Vec::new()));
        }
        let mut end = None;
        let mut results = Vec::new();
        for call in &step.calls {
            let outcome = self.call_tool(session, caller, call, &step.text)?;
            record.calls.push(CallRecord {
                id: call.id.clone(),
                name: call.name.clone(),
                input: tools::seal_input(&call.name, &call.input),
                result: outcome.result.content.clone(),
                is_error: outcome.result.is_error,
            });
            match outcome.ends {
                Some(ends) => end = end.or(Some(ends)),
                None => results.push(outcome.result),
            }
        }
        debug_assert_eq!(record.calls.len(), step.calls.len());
        Ok((record, end, results))
    }

    /// Deliver what is queued for `agent`, then `input`. On a failure the
    /// failed input and everything after it stay queued, in order.
    pub(super) fn deliver_queued(
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

    /// One model call, admitted before it is made and settled after.
    fn model_step(&mut self, agent: &str) -> Result<Step, TeachError> {
        let today = chrono::Local::now().date_naive();
        let estimate = self.engine.estimate(agent);
        let admission = self
            .noesis
            .accountant()
            .admit(Lane::Interactive, estimate, today)?;
        match self.engine.step(agent) {
            Ok(step) => {
                admission.settle(step.usage);
                Ok(step)
            }
            Err(EngineError::NotSent(reason)) => {
                admission.abandon_unsent();
                Err(EngineError::NotSent(reason).into())
            }
            // Dropping the admission charges the estimate: the call may have run.
            Err(error) => Err(error.into()),
        }
    }
}

/// The step's text as it may be shown: withheld when it holds any sealed
/// text a call of the step carries, or when a sealed input is unreadable.
fn shown_text(step: &Step) -> String {
    let haystack = step.text.to_lowercase();
    let reveals =
        step.calls
            .iter()
            .any(|call| match tools::sealed_texts(&call.name, &call.input) {
                None => true,
                Some(secrets) => secrets.iter().any(|secret| {
                    question::contains_phrase(&haystack, &secret.trim().to_lowercase())
                }),
            });
    if reveals {
        WITHHELD.to_string()
    } else {
        step.text.clone()
    }
}
