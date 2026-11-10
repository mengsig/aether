//! Resumable teaching turns, split at the model call so the host can release its store.

use super::db;
use super::engine::{Engine, EngineError, Input, ReplyText, Step};
use super::history;
use super::records::{NoesisEvent, SessionStall, StallReason};
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
    text_policy: ReplyText,
    admission: Option<Admission>,
}

impl ModelCall {
    /// Execute without a store borrow; return the result to the same advance.
    pub fn run<E: Engine>(self, engine: &mut E) -> Result<Step, TeachError> {
        debug_assert!(!self.agent.is_empty());
        debug_assert_eq!(self.admission.is_none(), engine.charges_own_calls());
        let result = engine.step(&self.agent, &self.live, self.text_policy);
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
    /// Start a session advance that can be resumed between model calls.
    pub fn new(session: &str) -> Self {
        Self {
            session: session.to_string(),
            reports: Vec::new(),
            current: None,
        }
    }

    /// Take the sealed reports after the advance completes.
    pub fn reports(self) -> Vec<TurnReport> {
        debug_assert!(self.current.is_none());
        debug_assert!(self.reports.len() <= 2);
        self.reports
    }
}

impl<E: Engine> Teaching<'_, E> {
    /// Record a recoverable failure after a turn cannot publish its output.
    pub fn stall_engine(&mut self, session: &str) -> Result<(), TeachError> {
        debug_assert!(!session.is_empty());
        let stall = SessionStall {
            reason: StallReason::EngineStalled,
            message: "The teacher stopped responding. Try again to continue your lesson.".into(),
            at: db::now(),
        };
        history::set_stall(self.conn(), session, &stall)?;
        self.events.push(NoesisEvent::SessionStalled {
            session: session.to_string(),
            stall,
        });
        Ok(())
    }

    /// Resume until the next call or completion; pass `None` only at the start.
    /// Keep the same engine and teaching buffer between calls.
    pub fn advance_until_call(
        &mut self,
        advance: &mut Advance,
        result: Option<Result<Step, TeachError>>,
    ) -> Result<Option<ModelCall>, TeachError> {
        let progressed = self.progress_advance(advance, result);
        if matches!(progressed, Ok(Some(_))) {
            return progressed;
        }
        let error = progressed.as_ref().err();
        if let Some(message) = error.and_then(budget_stall) {
            self.stall_session(&advance.session, message)?;
        } else if matches!(error, Some(TeachError::Engine(_))) {
            self.stall_engine(&advance.session)?;
        } else {
            history::clear_stall(self.conn(), &advance.session)?;
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
        let agent = advance
            .current
            .as_ref()
            .ok_or_else(|| TeachError::Corrupt("turn missing".into()))?
            .caller
            .agent
            .clone();
        if let Err(error) = self.deliver_queued(&advance.session, &agent, None) {
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
        let current = advance
            .current
            .as_mut()
            .ok_or_else(|| TeachError::Corrupt("turn missing".into()))?;
        self.mark_answer_turn(&advance.session, &current.caller, &mut current.turn)?;
        self.guide_recall_judgment(&advance.session, &agent)?;
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
            text_policy: self.reply_text_policy(&advance.session, &current.turn)?,
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
        debug_assert!(current.turn.steps.len() < MAX_STEPS);
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
            self.require_teaching_check(&current.caller.agent, &mut current.turn)?;
            self.require_recall_judgment(
                &advance.session,
                &current.caller.agent,
                &mut current.turn,
            )?;
            if end.is_none() && current.turn.steps.len() == MAX_STEPS && current.turn.needs_card() {
                return Err(EngineError::Stalled(
                    "the teaching turn ended before its check".into(),
                )
                .into());
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
        let mut turn = current.turn;
        if super::turn::agent_for_deferred_prose(&current.caller) {
            self.publish_standalone_text(&current.caller.agent, end, &mut turn, &sealer)?;
        }
        let steps = turn.seal(&sealer);
        debug_assert!(end != TurnEnd::StepLimit || steps.len() == MAX_STEPS);
        advance.reports.push(TurnReport {
            agent: current.caller.agent,
            steps,
            end,
        });
        Ok(())
    }
}
