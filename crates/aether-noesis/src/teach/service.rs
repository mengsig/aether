//! [`Teaching`]: sessions, learner input and the turn loop over an engine.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use aether_core::noesis::EvidenceBundle;
use serde_json::{Value, json};

use super::db::{self, SessionRow};
use super::engine::{AgentSpec, Engine, EngineError, Input, NOESIS_RUN, SUBSYSTEM};
use super::records::{
    Answer, EvidenceDraft, EvidenceOutcome, Mode, NoesisEvent, Phase, Question, QuestionKind,
    QuestionResult, RevealedKey, SessionView,
};
use super::{TeachError, phase, plan, question, role_of};
use crate::Noesis;
use crate::budget::{ModelOffer, choose_model};
use crate::evidence;
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
    /// Inputs whose delivery failed, in order, by agent; delivered before
    /// anything new reaches that agent.
    pub(super) undelivered: HashMap<String, VecDeque<Input>>,
}

impl<E: Engine> std::fmt::Debug for Teaching<'_, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Teaching")
            .field("prompt_overrides", &self.prompt_overrides)
            .field("pending_events", &self.events.len())
            .field("pending_evidence", &self.evidence.len())
            .field("undelivered_agents", &self.undelivered.len())
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
            undelivered: HashMap::new(),
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

    pub(super) fn conn(&self) -> &'a rusqlite::Connection {
        self.noesis.store().connection()
    }

    /// Create a session: a Prober for `teach`, a Practitioner for `practice`.
    pub fn create_session(
        &mut self,
        goal: &str,
        mode: Mode,
        practice_targets: &[String],
    ) -> Result<SessionView, TeachError> {
        self.create_session_from(goal, mode, practice_targets, None)
    }

    /// [`Teaching::create_session`] starting from an evidence bundle, which is
    /// admitted before any agent exists and stored with the session row.
    pub fn create_session_from(
        &mut self,
        goal: &str,
        mode: Mode,
        practice_targets: &[String],
        context: Option<&EvidenceBundle>,
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
        if let Some(bundle) = context {
            evidence::admit(&self.noesis.effective(), bundle)
                .map_err(|error| TeachError::Invalid(error.to_string()))?;
        }
        let targets = self.resolve_targets(practice_targets)?;
        let agent = match mode {
            Mode::Teach => Agent::Prober,
            Mode::Practice => Agent::Practitioner,
        };
        let agent_id = self.spawn(agent)?;
        let opening = opening(goal, &targets, context)?;
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
        self.insert_with_bundle(&row, context)?;
        let view = self.session_view(&row)?;
        self.events.push(NoesisEvent::SessionCreated {
            session: Box::new(view.clone()),
        });
        Ok(view)
    }

    /// The session row and its bundle in one transaction: both or neither.
    fn insert_with_bundle(
        &self,
        row: &SessionRow,
        context: Option<&EvidenceBundle>,
    ) -> Result<(), TeachError> {
        let tx = self.conn().unchecked_transaction()?;
        db::insert_session(&tx, row)?;
        if let Some(bundle) = context {
            evidence::store::save(&tx, &row.id, bundle, &row.created_at).map_err(|error| {
                match error {
                    evidence::EvidenceError::Sqlite(error) => TeachError::Sqlite(error),
                    other => TeachError::Invalid(other.to_string()),
                }
            })?;
        }
        tx.commit()?;
        Ok(())
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
        // An open question was posed in the session's current phase.
        let open_question = db::open_question(conn, &row.id)?.map(|open| Question {
            step: Some(question::step(
                row.phase,
                open.question.kind,
                open.question.collapsed,
            )),
            ..open.question
        });
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
        self.deliver_queued(session, &row.agent, None)?;
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
        let chosen = question::chosen_labels(posed.options, &answer.choices);
        let recorded = json!({
            "choices": answer.choices,
            "chosen": chosen,
            "text": answer.text,
            "dont_know": answer.dont_know,
            "confidence": graded.confidence,
            "thinking": answer.thinking,
        });
        self.record_owed_answer(&row, question_id, recorded, graded.correct)?;
        let result = reveal(&open, graded.correct);
        self.events.push(NoesisEvent::AnswerRecorded {
            session: session.to_string(),
            result: result.clone(),
        });
        self.record_answer_evidence(&row, &open.question, answer.dont_know, &graded);
        // Recorded and announced first. A failed delivery stays owed in the
        // store, so the next turn or a restart delivers it, and is returned.
        self.deliver_queued(session, &row.agent, None)?;
        debug_assert!(db::owed_answers(self.conn(), session)?.is_empty());
        Ok(result)
    }

    /// Commit the answer as owed to the agent; delivering it is the caller's.
    /// The record and the agent agree even across a restart.
    fn record_owed_answer(
        &mut self,
        row: &SessionRow,
        question_id: &str,
        mut recorded: Value,
        correct: Option<bool>,
    ) -> Result<(), TeachError> {
        debug_assert!(recorded.is_object(), "an answer record is an object");
        recorded["delivered"] = json!(false);
        let tx = self.conn().unchecked_transaction()?;
        if !db::record_answer(&tx, question_id, &recorded, correct)? {
            return Err(TeachError::Conflict(format!(
                "question `{question_id}` was already answered"
            )));
        }
        db::touch(&tx, &row.id)?;
        tx.commit()?;
        debug_assert!(db::open_question(self.conn(), &row.id)?.is_none());
        Ok(())
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

/// The first input of a session's agent: its goal, targets and any bundle.
fn opening(
    goal: &str,
    targets: &[String],
    context: Option<&EvidenceBundle>,
) -> Result<String, TeachError> {
    debug_assert!(!goal.is_empty(), "a checked goal");
    let mut opening = format!("Learning goal: {goal}");
    if !targets.is_empty() {
        opening.push_str(&format!("\nPractice targets: {}", targets.join(", ")));
    }
    if let Some(bundle) = context {
        opening.push_str("\n\n");
        let rendered = evidence::prompt::render(bundle)
            .map_err(|error| TeachError::Invalid(error.to_string()))?;
        opening.push_str(&rendered);
    }
    Ok(opening)
}
