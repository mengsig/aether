//! The turn loop: model steps admitted against the budget, their tool calls
//! run, and results delivered in order, even after a failed delivery.

use serde_json::Value;

use super::db;
use super::engine::{Engine, EngineError, Input, Step, ToolResult};
use super::service::{
    CallRecord, Caller, MAX_STEPS, StepRecord, Teaching, TurnEnd, TurnReport, driving_agent,
};
use super::{TeachError, question, role_of};
use crate::budget::Lane;
use crate::tools::{self, ToolArgs, ToolName};

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
        self.flush_undelivered(&caller.agent)?;
        if db::open_question(self.conn(), session)?.is_some() {
            report.end = TurnEnd::AwaitingAnswer;
            return Ok(report);
        }
        for _ in 0..MAX_STEPS {
            let step = self.model_step(&caller.agent)?;
            let (record, end, results) = self.run_calls(session, &caller, &step)?;
            report.steps.push(record);
            self.deliver_results(&caller.agent, results)?;
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

    /// Deliver a step's results; on failure keep them for the agent's next
    /// input, so a retry neither loses nor reorders them.
    fn deliver_results(&mut self, agent: &str, results: Vec<ToolResult>) -> Result<(), TeachError> {
        debug_assert!(
            !self.undelivered.contains_key(agent),
            "flushed at turn start"
        );
        if results.is_empty() {
            return Ok(());
        }
        let input = Input::ToolResults(results.clone());
        if let Err(error) = self.engine.deliver(agent, input) {
            self.undelivered.insert(agent.to_string(), results);
            return Err(error.into());
        }
        Ok(())
    }

    /// Deliver tool results an earlier failure kept back, before anything new.
    pub(super) fn flush_undelivered(&mut self, agent: &str) -> Result<(), TeachError> {
        let Some(results) = self.undelivered.remove(agent) else {
            return Ok(());
        };
        debug_assert!(!results.is_empty(), "only a non-empty batch is kept");
        let input = Input::ToolResults(results.clone());
        if let Err(error) = self.engine.deliver(agent, input) {
            self.undelivered.insert(agent.to_string(), results);
            return Err(error.into());
        }
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

/// The step's text as it may be shown: withheld when it gives away the key
/// of a question the step tries to pose (such a pose is always refused).
fn shown_text(step: &Step) -> String {
    let reveals = step
        .calls
        .iter()
        .filter(|call| call.name == ToolName::Ask.as_str())
        .any(
            |call| match tools::parse(ToolName::Ask, call.input.clone()) {
                Ok(ToolArgs::Ask(args)) => question::reveals_key(&args, &step.text),
                _ => raw_key_shown(&call.input, &step.text),
            },
        );
    if reveals {
        WITHHELD.to_string()
    } else {
        step.text.clone()
    }
}

/// For an ask that does not parse: whether its raw key (a correct option's
/// label, or the key text when there are no options) shows in the text.
fn raw_key_shown(input: &Value, text: &str) -> bool {
    let key: Vec<&str> = match &input["answer_key"] {
        Value::String(entry) => vec![entry.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let options = input["options"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let secrets: Vec<&str> = if options.is_empty() {
        key
    } else {
        options
            .iter()
            .filter(|option| option["value"].as_str().is_some_and(|v| key.contains(&v)))
            .filter_map(|option| option["label"].as_str())
            .collect()
    };
    let haystack = text.to_lowercase();
    secrets
        .iter()
        .any(|secret| question::contains_phrase(&haystack, &secret.trim().to_lowercase()))
}
