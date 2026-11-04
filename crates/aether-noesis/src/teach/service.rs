//! [`Teaching`]: sessions, learner input and the turn loop over an engine.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::db::{self, SessionRow};
use super::engine::{AgentSpec, Engine, EngineError, Input, NOESIS_RUN, SUBSYSTEM, Step};
use super::records::{
    Answer, EvidenceDraft, EvidenceOutcome, Mode, NoesisEvent, Phase, QuestionKind, QuestionResult,
    RevealedKey, SessionView,
};
use super::{TeachError, phase, plan, question, role_of};
use crate::Noesis;
use crate::budget::{Lane, ModelOffer, choose_model};
use crate::prompts::{self, Agent};
use crate::tools;

pub const MAX_GOAL: usize = 2_000;
pub const MAX_MESSAGE: usize = 20_000;
/// Model calls one turn may make before it stops and waits for the learner.
pub const MAX_STEPS: usize = 12;

/// The agent driving a session and the Noesis role it runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub agent: String,
    pub role: String,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// The agent replied without a tool call and waits for the learner.
    AwaitingLearner,
    /// A question was posed; the turn resumes with the answer.
    AwaitingAnswer,
    /// The Prober finished and a Teacher now drives the session.
    HandedOff,
    /// The turn made [`MAX_STEPS`] model calls and stopped.
    StepLimit,
}

/// A tool call as persisted and shown: sealed input, visible result.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub result: Value,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepRecord {
    pub text: String,
    pub calls: Vec<CallRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnReport {
    pub agent: String,
    pub steps: Vec<StepRecord>,
    pub end: TurnEnd,
}

/// The teaching loop over a running [`Noesis`]; it exists only while Noesis
/// is enabled. Events and learner evidence collect until taken.
pub struct Teaching<'a, E: Engine> {
    pub(super) noesis: &'a Noesis,
    pub(super) engine: E,
    pub(super) prompt_overrides: Option<PathBuf>,
    pub(super) events: Vec<NoesisEvent>,
    pub(super) evidence: Vec<EvidenceDraft>,
}

impl<E: Engine> std::fmt::Debug for Teaching<'_, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Teaching")
            .field("prompt_overrides", &self.prompt_overrides)
            .field("pending_events", &self.events.len())
            .field("pending_evidence", &self.evidence.len())
            .finish_non_exhaustive()
    }
}

impl<'a, E: Engine> Teaching<'a, E> {
    pub fn new(noesis: &'a Noesis, engine: E) -> Self {
        // `<aether data dir>/noesis/prompts`, read only with the switch on.
        let data_dir = noesis.store().path().parent().map(PathBuf::from);
        let overrides = noesis
            .settings()
            .prompt_overrides
            .then_some(data_dir)
            .flatten();
        Self {
            noesis,
            engine,
            prompt_overrides: overrides.map(|dir| dir.join("noesis").join("prompts")),
            events: Vec::new(),
            evidence: Vec::new(),
        }
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// The events emitted since the last call, in order, for F3 to publish.
    pub fn take_events(&mut self) -> Vec<NoesisEvent> {
        std::mem::take(&mut self.events)
    }

    /// The learner evidence produced since the last call, for F4 to append.
    pub fn take_evidence(&mut self) -> Vec<EvidenceDraft> {
        std::mem::take(&mut self.evidence)
    }

    pub(super) fn conn(&self) -> &rusqlite::Connection {
        self.noesis.store().connection()
    }

    /// Create a session: a Prober for `teach`, a Practitioner for `practice`.
    pub fn create_session(
        &mut self,
        goal: &str,
        mode: Mode,
        practice_targets: &[String],
    ) -> Result<SessionView, TeachError> {
        let goal = goal.trim();
        if goal.is_empty() || goal.chars().count() > MAX_GOAL {
            return Err(TeachError::Invalid(format!(
                "a goal has 1 to {MAX_GOAL} characters"
            )));
        }
        if mode == Mode::Teach && !practice_targets.is_empty() {
            return Err(TeachError::Invalid(
                "practice targets need a practice session".into(),
            ));
        }
        let targets = self.resolve_targets(practice_targets)?;
        let agent = match mode {
            Mode::Teach => Agent::Prober,
            Mode::Practice => Agent::Practitioner,
        };
        let agent_id = self.spawn(agent)?;
        let mut opening = format!("Learning goal: {goal}");
        if !targets.is_empty() {
            opening.push_str(&format!("\nPractice targets: {}", targets.join(", ")));
        }
        self.engine.deliver(&agent_id, Input::Learner(opening))?;
        let at = db::now();
        let row = SessionRow {
            id: db::fresh_id("ns"),
            goal: goal.to_string(),
            mode,
            phase: phase::initial(mode),
            agent: agent_id,
            created_at: at.clone(),
            updated_at: at,
        };
        db::insert_session(self.conn(), &row)?;
        let view = self.session_view(&row)?;
        self.events.push(NoesisEvent::SessionCreated {
            session: Box::new(view.clone()),
        });
        Ok(view)
    }

    fn resolve_targets(&self, targets: &[String]) -> Result<Vec<String>, TeachError> {
        let mut ids = Vec::with_capacity(targets.len());
        for target in targets {
            let Some(id) = db::resolve_target(self.conn(), target)? else {
                return Err(TeachError::NotFound {
                    what: "proposition",
                    id: target.clone(),
                });
            };
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Create a Noesis agent in run `noesis` under its `noesis.*` role, on
    /// the strong slot's model: never a silent fallback to a billed one.
    pub(super) fn spawn(&mut self, agent: Agent) -> Result<String, TeachError> {
        let settings = self.noesis.settings();
        let models = self.engine.models();
        let offers: Vec<ModelOffer<'_>> = models
            .iter()
            .map(|model| ModelOffer {
                id: &model.id,
                billing: model.billing,
                ready: model.ready,
            })
            .collect();
        let model = choose_model(
            settings.model_strong.as_deref(),
            &offers,
            settings.billing_allow_api,
        )?;
        let prompt = prompts::compose(agent, self.prompt_overrides.as_deref())?;
        let spec = AgentSpec {
            run: NOESIS_RUN,
            subsystem: SUBSYSTEM,
            agent,
            role: role_of(agent),
            model,
            system_prompt: prompt.text,
            tools: tools::definitions(agent),
        };
        debug_assert!(spec.role.starts_with("noesis.") && spec.run == NOESIS_RUN);
        let id = self.engine.spawn(spec)?;
        if id.is_empty() {
            return Err(TeachError::Engine(EngineError::Failed(
                "the engine returned an empty agent id".into(),
            )));
        }
        Ok(id)
    }

    /// The session as every reader sees it, sealed values absent.
    pub fn get_session(&self, session: &str) -> Result<SessionView, TeachError> {
        let row = db::session(self.conn(), session)?;
        self.session_view(&row)
    }

    pub(super) fn session_view(&self, row: &SessionRow) -> Result<SessionView, TeachError> {
        let conn = self.conn();
        let plan = match db::latest_plan(conn, &row.id)? {
            Some((revision, dag)) => Some(plan::tree(
                revision,
                &dag,
                &db::tagged_nodes(conn, &row.id)?,
            )),
            None => None,
        };
        let open_question = db::open_question(conn, &row.id)?.map(|open| open.question);
        let active_problem = db::active_problem(conn, &row.id)?.map(|active| active.problem);
        Ok(SessionView {
            id: row.id.clone(),
            goal: row.goal.clone(),
            mode: row.mode,
            phase: row.phase,
            agent: row.agent.clone(),
            plan,
            open_question,
            active_problem,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        })
    }

    /// Persist a learner message for the session's agent; run the turn with
    /// [`Teaching::advance`]. Refused while a question waits for its answer.
    pub fn send(&mut self, session: &str, text: &str) -> Result<(), TeachError> {
        if text.trim().is_empty() || text.chars().count() > MAX_MESSAGE {
            return Err(TeachError::Invalid(format!(
                "a message has 1 to {MAX_MESSAGE} characters"
            )));
        }
        let row = db::session(self.conn(), session)?;
        if db::open_question(self.conn(), session)?.is_some() {
            return Err(TeachError::Conflict(
                "a question is open; answer it first".into(),
            ));
        }
        self.engine
            .deliver(&row.agent, Input::Learner(text.to_string()))?;
        db::touch(self.conn(), session)?;
        Ok(())
    }

    /// Record the learner's answer to the open question, grade a choice
    /// answer locally, reveal the key, and hand the answer to the agent.
    pub fn answer(
        &mut self,
        session: &str,
        question_id: &str,
        answer: &Answer,
    ) -> Result<QuestionResult, TeachError> {
        let row = db::session(self.conn(), session)?;
        let open = db::open_question(self.conn(), session)?;
        let Some(open) = open.filter(|open| open.question.id == question_id) else {
            return Err(TeachError::Conflict(format!(
                "question `{question_id}` is not open in this session"
            )));
        };
        let posed = question::Posed {
            kind: open.question.kind,
            multi: open.question.multi,
            options: open.question.options.as_deref().unwrap_or_default(),
            answer_key: &open.answer_key,
        };
        let graded = question::grade(&posed, answer).map_err(TeachError::Invalid)?;
        let recorded = json!({
            "choices": answer.choices,
            "text": answer.text,
            "dont_know": answer.dont_know,
            "confidence": graded.confidence,
            "thinking": answer.thinking,
        });
        if !db::record_answer(self.conn(), question_id, &recorded, graded.correct)? {
            return Err(TeachError::Conflict(format!(
                "question `{question_id}` was already answered"
            )));
        }
        let mut for_agent = recorded;
        for_agent["question"] = json!(question_id);
        if let Some(correct) = graded.correct {
            for_agent["correct"] = json!(correct);
        }
        self.engine.deliver(&row.agent, Input::Answer(for_agent))?;
        db::touch(self.conn(), session)?;
        let result = reveal(&open, graded.correct);
        self.events.push(NoesisEvent::AnswerRecorded {
            session: session.to_string(),
            result: result.clone(),
        });
        self.record_answer_evidence(&row, &open.question, answer.dont_know, &graded);
        Ok(result)
    }

    fn record_answer_evidence(
        &mut self,
        row: &SessionRow,
        posed: &super::records::Question,
        dont_know: bool,
        graded: &question::Graded,
    ) {
        let Some(kind) = question::evidence_kind(row.phase, posed.kind, posed.collapsed) else {
            return;
        };
        let outcome = match (dont_know, graded.correct) {
            (true, _) => EvidenceOutcome::DontKnow,
            (false, Some(true)) => EvidenceOutcome::Correct,
            (false, Some(false)) => EvidenceOutcome::Incorrect,
            // A recall answer is judged by the agent, not here.
            (false, None) => return,
        };
        debug_assert!(
            !dont_know || graded.confidence.is_none(),
            "don't know carries no confidence"
        );
        let concepts = if posed.concepts.is_empty() {
            posed.plan_node.iter().cloned().collect()
        } else {
            posed.concepts.clone()
        };
        self.evidence.push(EvidenceDraft {
            session: row.id.clone(),
            concepts,
            kind,
            outcome,
            confidence: graded.confidence,
            dont_know,
            mode: None,
            hint_rung: None,
            miss_class: None,
            note: None,
        });
    }

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
    /// against the interactive budget, until the agent waits.
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
        for _ in 0..MAX_STEPS {
            let step = self.model_step(&caller.agent)?;
            let (record, end, results) = self.run_calls(session, &caller, &step)?;
            report.steps.push(record);
            if !results.is_empty() {
                self.engine
                    .deliver(&caller.agent, Input::ToolResults(results))?;
            }
            if let Some(end) = end {
                report.end = end;
                return Ok(report);
            }
        }
        Ok(report)
    }

    fn run_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        step: &Step,
    ) -> Result<(StepRecord, Option<TurnEnd>, Vec<super::ToolResult>), TeachError> {
        let mut record = StepRecord {
            text: step.text.clone(),
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
        Ok((record, end, results))
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

/// The agent a session's phase belongs to.
pub(super) fn driving_agent(row: &SessionRow) -> Agent {
    match row.phase {
        Phase::Probe => Agent::Prober,
        Phase::Plan | Phase::Teach => Agent::Teacher,
        Phase::Practice => Agent::Practitioner,
    }
}

fn reveal(open: &super::db::QuestionRow, correct: Option<bool>) -> QuestionResult {
    let answer_key = match open.question.kind {
        QuestionKind::Choice => Some(RevealedKey::Choices(open.answer_key.clone())),
        QuestionKind::Recall => open.answer_key.first().cloned().map(RevealedKey::Text),
        QuestionKind::Goal => None,
    };
    QuestionResult {
        question: open.question.id.clone(),
        correct,
        answer_key,
        explanation: open.explanation.clone(),
    }
}
