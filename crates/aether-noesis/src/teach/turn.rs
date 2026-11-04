//! The turn loop: model steps admitted against the budget, their tool calls
//! run, and results delivered in order, even after a failed delivery.

use serde_json::Value;

use super::db;
use super::engine::{Engine, EngineError, Input, Step, ToolResult};
use super::service::{
    CallRecord, Caller, MAX_STEPS, StepRecord, Teaching, TurnEnd, TurnReport, driving_agent,
};
use super::{TeachError, question, role_of, seal};
use crate::budget::Lane;
use crate::tools;

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
            text: String::new(),
            calls: Vec::with_capacity(step.calls.len()),
        };
        if step.calls.is_empty() {
            record.text = seal::shown_text(step, Some(&self.live_secrets(session)?));
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
        self.seal_step(session, step, &mut record, &mut results)?;
        Ok((record, end, results))
    }

    /// Seal what a step shows and delivers against the secrets left after its
    /// calls ran (a conclude in the step reveals its solution) and its own.
    fn seal_step(
        &self,
        session: &str,
        step: &Step,
        record: &mut StepRecord,
        results: &mut [ToolResult],
    ) -> Result<(), TeachError> {
        let own = seal::step_secrets(step);
        let readable = own.is_some();
        let live = self.live_secrets(session)?;
        let mut secrets = live.clone();
        secrets.extend(own.unwrap_or_default());
        record.text = seal::shown_text(step, readable.then_some(secrets.as_slice()));
        for call in &mut record.calls {
            call.name = seal::sealed_text(&call.name, &secrets);
            seal::withhold(&mut call.input, &secrets);
            seal_result(&mut call.result, call.is_error, &secrets, &live);
        }
        for result in results.iter_mut() {
            seal_result(&mut result.content, result.is_error, &secrets, &live);
        }
        debug_assert!(readable || record.text == seal::WITHHELD);
        Ok(())
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

/// A result as it may be recorded or delivered. A refusal is withheld whole
/// when it quotes a committed secret; anything else has each secret masked.
fn seal_result(content: &mut Value, is_error: bool, secrets: &[String], live: &[String]) {
    debug_assert!(live.iter().all(|secret| secrets.contains(secret)));
    if is_error {
        seal::seal_refusal(content, live);
    } else {
        seal::withhold(content, secrets);
    }
}
