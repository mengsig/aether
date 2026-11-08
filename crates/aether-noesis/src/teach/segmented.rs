//! Resumable teaching turns, split at the model call so the host can release its store.

use super::db;
use super::engine::{Engine, EngineError, Input, Step};
use super::history;
use super::seal::Sealer;
use super::service::{Caller, MAX_STEPS, Teaching, TurnEnd, TurnReport, driving_agent};
use super::turn::{Turn, budget_stall};
use super::{TeachError, role_of};
use crate::budget::{Admission, Lane};

/// The state retained between store-scoped portions of an advance.
#[derive(Debug)]
pub struct Advance {
    session: String,
    reports: Vec<TurnReport>,
    current: Option<CurrentTurn>,
}

#[derive(Debug)]
struct CurrentTurn {
    caller: Caller,
    turn: Turn,
}

/// A prepared model call. It owns its seal and admission, but no store borrow.
#[derive(Debug)]
pub struct ModelCall {
    agent: String,
    live: Sealer,
    admission: Option<Admission>,
}

impl ModelCall {
    pub fn run<E: Engine>(self, engine: &mut E) -> Result<Step, TeachError> {
        debug_assert!(!self.agent.is_empty());
        let result = engine.step(&self.agent, &self.live);
        match (self.admission, result) {
            (Some(admission), Ok(step)) => {
                admission.settle(step.usage);
                Ok(step)
            }
            (
                Some(admission),
                Err(error @ (EngineError::NotSent(_) | EngineError::BudgetSpent(_))),
            ) => {
                admission.abandon_unsent();
                Err(error.into())
            }
            (_, result) => result.map_err(Into::into),
        }
    }
}

impl Advance {
    pub fn new(session: &str) -> Self {
        Self {
            session: session.to_string(),
            reports: Vec::new(),
            current: None,
        }
    }

    pub fn reports(self) -> Vec<TurnReport> {
        debug_assert!(self.current.is_none());
        self.reports
    }
}

impl<E: Engine> Teaching<'_, E> {
    /// Resume an advance until its next model call or completion.
    pub fn advance_until_call(
        &mut self,
        advance: &mut Advance,
        result: Option<Result<Step, TeachError>>,
    ) -> Result<Option<ModelCall>, TeachError> {
        let progressed = self.progress_advance(advance, result);
        if matches!(progressed, Ok(Some(_))) {
            return progressed;
        }
        match progressed.as_ref().err().and_then(budget_stall) {
            Some(message) => self.stall_session(&advance.session, message)?,
            None => {
                history::clear_stall(self.conn(), &advance.session)?;
            }
        }
        if matches!(progressed, Ok(None)) {
            super::sessions::set_turn_pending(self.conn(), &advance.session, false)?;
        }
        progressed
    }

    fn progress_advance(
        &mut self,
        advance: &mut Advance,
        result: Option<Result<Step, TeachError>>,
    ) -> Result<Option<ModelCall>, TeachError> {
        if let Some(result) = result {
            let end = self.finish_step(advance, result);
            if let Some(end) = end? {
                self.finish_current(advance, Ok(end))?;
            }
        } else if advance.current.is_some() {
            return Err(TeachError::Corrupt("missing model result".into()));
        }
        loop {
            if advance.current.is_none() {
                if advance.reports.len() == 2
                    || advance
                        .reports
                        .last()
                        .is_some_and(|r| r.end != TurnEnd::HandedOff)
                {
                    return Ok(None);
                }
                if let Some(end) = self.start_current(advance)? {
                    self.finish_current(advance, Ok(end))?;
                    continue;
                }
            }
            return match self.prepare_model_call(advance) {
                Ok(call) => Ok(Some(call)),
                Err(error) => {
                    self.finish_current(advance, Err(error))?;
                    Err(TeachError::Corrupt(
                        "failed turn unexpectedly succeeded".into(),
                    ))
                }
            };
        }
    }

    fn start_current(&mut self, advance: &mut Advance) -> Result<Option<TurnEnd>, TeachError> {
        debug_assert!(advance.current.is_none());
        debug_assert!(advance.reports.len() <= 1);
        let row = db::session(self.conn(), &advance.session)?;
        let caller = Caller {
            agent: row.agent.clone(),
            role: role_of(driving_agent(&row)),
        };
        advance.current = Some(CurrentTurn {
            caller,
            turn: Turn::new(self.events.len()),
        });
        let current = advance
            .current
            .as_ref()
            .ok_or_else(|| TeachError::Corrupt("turn missing".into()))?;
        if let Err(error) = self.deliver_queued(&advance.session, &current.caller.agent, None) {
            self.finish_current(advance, Err(error))?;
        }
        let open = match db::open_question(self.conn(), &advance.session) {
            Ok(open) => open,
            Err(error) => {
                self.finish_current(advance, Err(error))?;
                return Err(TeachError::Corrupt(
                    "failed turn unexpectedly succeeded".into(),
                ));
            }
        };
        if open.is_some() {
            return Ok(Some(TurnEnd::AwaitingAnswer));
        }
        Ok(None)
    }

    fn prepare_model_call(&self, advance: &Advance) -> Result<ModelCall, TeachError> {
        debug_assert!(
            self.conn().is_autocommit(),
            "model calls cannot hold a transaction"
        );
        let current = advance
            .current
            .as_ref()
            .ok_or_else(|| TeachError::Corrupt("turn missing".into()))?;
        debug_assert!(current.turn.steps.len() < MAX_STEPS);
        let solution = db::active_problem(self.conn(), &advance.session)?.map(|row| row.solution);
        let live = Sealer::live(self.live_secrets(&advance.session)?, solution.as_deref());
        debug_assert!(
            self.conn().is_autocommit(),
            "preparing a call leaves no transaction"
        );
        let admission = if self.engine.charges_own_calls() {
            None
        } else {
            let estimate = self.engine.estimate(&current.caller.agent);
            Some(self.noesis.accountant().admit(
                Lane::Interactive,
                estimate,
                chrono::Local::now().date_naive(),
            )?)
        };
        Ok(ModelCall {
            agent: current.caller.agent.clone(),
            live,
            admission,
        })
    }

    fn finish_step(
        &mut self,
        advance: &mut Advance,
        result: Result<Step, TeachError>,
    ) -> Result<Option<TurnEnd>, TeachError> {
        let Some(current) = advance.current.as_mut() else {
            return Err(TeachError::Corrupt("model result without a turn".into()));
        };
        let ran = result.and_then(|step| {
            let (end, results) =
                self.run_calls(&advance.session, &current.caller, step, &mut current.turn)?;
            if !results.is_empty() {
                self.deliver_queued(
                    &advance.session,
                    &current.caller.agent,
                    Some(Input::ToolResults(results)),
                )?;
            }
            Ok(end
                .or_else(|| (current.turn.steps.len() == MAX_STEPS).then_some(TurnEnd::StepLimit)))
        });
        match ran {
            Ok(end) => Ok(end),
            Err(error) => {
                self.finish_current(advance, Err(error))?;
                Err(TeachError::Corrupt(
                    "failed turn unexpectedly succeeded".into(),
                ))
            }
        }
    }

    fn finish_current(
        &mut self,
        advance: &mut Advance,
        ran: Result<TurnEnd, TeachError>,
    ) -> Result<(), TeachError> {
        let current = advance
            .current
            .take()
            .ok_or_else(|| TeachError::Corrupt("turn missing".into()))?;
        let sealer = match self.turn_sealer(&advance.session, &current.turn) {
            Ok(sealer) => sealer,
            Err(error) => {
                self.events.truncate(current.turn.events_from);
                return Err(error);
            }
        };
        let end = ran?;
        let steps = current.turn.seal(&sealer);
        debug_assert!(end != TurnEnd::StepLimit || steps.len() == MAX_STEPS);
        advance.reports.push(TurnReport {
            agent: current.caller.agent,
            steps,
            end,
        });
        Ok(())
    }
}
