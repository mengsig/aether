//! The turn loop: model steps admitted against the budget, their tool calls
//! run, results delivered in order even after a failed delivery, and the
//! turn's tool results sealed once at its end.

use serde_json::{Value, json};

use super::db;
use super::engine::{
    AskPolicy, Engine, EngineError, Input, ReplyText, Step, TextPolicy, ToolResult,
    asks_with_prose, is_ask,
};
use super::history;
use super::records::{NoesisEvent, SessionStall, StallReason};
use super::seal::{self, Sealer};
use super::segmented::Advance;
use super::service::{
    CallRecord, Caller, MAX_STEPS, StepRecord, Teaching, TurnEnd, TurnReport, driving_agent,
};
use super::{TeachError, question, role_of};
use crate::budget::{INTERACTIVE_BUDGET_SETTING, Lane, Refusal};
use crate::tools::{self, SEALED, ToolName};

const JUDGMENT_ATTEMPTS: usize = 3;
const CHECK_ATTEMPTS: usize = 3;

const EXPLAIN_GUIDANCE: &str = "System guidance, not a learner message: the check card you tried to pose was not posed, and the text beside it was not shown. First teach this step: write only the explanation or worked example, in plain statements. Do not ask the learner anything or give them a task. Make no tool calls in this reply; if using the JSON envelope, set tool_calls to an empty array. The check card is posed for you right after.";
const POSE_GUIDANCE: &str = "System guidance, not a learner message: your explanation is shown. Now pose the check for this step by calling noesis.ask. Write no other text; the card carries the question.";

/// The stages between a mixed teacher reply and a check card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CardStage {
    /// A card with prose beside it is held and the prose dropped; any other is posed.
    Open,
    /// Explain without posing the card.
    Explaining,
    /// Replies show no text; the card is posed.
    Posing,
}

impl<E: Engine> Teaching<'_, E> {
    /// Run the session's agent until it waits, and on a hand-off run the new
    /// Teacher's first turn too. A failed turn leaves its inputs persisted,
    /// so calling this again recovers; one the budget refused stalls the session.
    pub fn advance(&mut self, session: &str) -> Result<Vec<TurnReport>, TeachError> {
        let mut advance = Advance::new(session);
        let mut result = None;
        while let Some(call) = self.advance_until_call(&mut advance, result)? {
            result = Some(call.run(&mut self.engine));
        }
        Ok(advance.reports())
    }

    /// Record why the session's turn could not run, and announce it.
    pub(super) fn stall_session(
        &mut self,
        session: &str,
        message: String,
    ) -> Result<(), TeachError> {
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
    /// Every reply the engine keeps already passed the one seal check
    /// ([`super::seal::reply_leaks`]). The turn's steps are published once it
    /// ends, their tool results sealed against the secrets left then.
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
        self.mark_answer_turn(session, caller, turn)?;
        self.guide_recall_judgment(session, &caller.agent)?;
        for _ in 0..MAX_STEPS {
            let policy = self.reply_text_policy(session, turn)?;
            let step = self.model_step(session, &caller.agent, policy)?;
            let (end, results) = self.run_calls(session, caller, step, turn)?;
            if !results.is_empty() {
                let input = Input::ToolResults(results);
                self.deliver_queued(session, &caller.agent, Some(input))?;
            }
            self.require_teaching_check(&caller.agent, turn)?;
            self.require_recall_judgment(session, &caller.agent, turn)?;
            if let Some(end) = end {
                return Ok(end);
            }
        }
        debug_assert_eq!(turn.steps.len(), MAX_STEPS);
        if turn.needs_card() {
            return Err(
                EngineError::Stalled("the teaching turn ended before its check".into()).into(),
            );
        }
        Ok(TurnEnd::StepLimit)
    }

    /// Run a step's calls and buffer the step raw; return how the turn ends,
    /// if it does, and the results owed to the engine.
    pub(super) fn run_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        step: Step,
        turn: &mut Turn,
    ) -> Result<(Option<TurnEnd>, Vec<ToolResult>), TeachError> {
        let mut step = step;
        let own: Vec<String> = step.calls.iter().flat_map(seal::call_secrets).collect();
        turn.own.extend(own.iter().cloned());
        let moved_on = self.route_card_stage(caller, &mut step, turn)?;
        let mut pending = PendingStep {
            text: step.text,
            calls: Vec::with_capacity(step.calls.len()),
        };
        if moved_on {
            turn.steps.push(pending);
            return Ok((None, Vec::new()));
        }
        if step.calls.is_empty() {
            return self.finish_without_calls(session, caller, pending, turn);
        }
        // Each step is kept as it ends, so a pose is checked against all the
        // turn has kept, not only its own step's text.
        let shown = turn.shown_with(&pending.text);
        let recovering_check = turn.check_attempts > 0;
        let (end, owed) =
            self.execute_calls(session, caller, step.calls, &shown, &mut pending, turn)?;
        if recovering_check && end != Some(TurnEnd::AwaitingAnswer) {
            pending.text.clear();
        } else if end == Some(TurnEnd::AwaitingAnswer) {
            turn.check_attempts = 0;
        }
        let results = self.engine_results(session, &own, &pending, &owed)?;
        turn.steps.push(pending);
        Ok((end, results))
    }

    fn finish_without_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        mut pending: PendingStep,
        turn: &mut Turn,
    ) -> Result<(Option<TurnEnd>, Vec<ToolResult>), TeachError> {
        debug_assert!(pending.calls.is_empty());
        if turn.check_attempts > 0 {
            pending.text.clear();
            turn.steps.push(pending);
            return Ok((None, Vec::new()));
        }
        if turn.stage == CardStage::Posing {
            return self.pose_again(caller, pending, turn);
        }
        if db::unjudged_recall(self.conn(), session)?.is_some() {
            turn.steps.push(pending);
            return Ok((None, Vec::new()));
        }
        if turn.answer_turn && self.has_next_plan_step(session)? {
            turn.continuation_attempts += 1;
            if turn.continuation_attempts > CHECK_ATTEMPTS {
                return Err(EngineError::Stalled(
                    "the teaching agent could not continue the plan".into(),
                )
                .into());
            }
            self.engine.deliver(&caller.agent, Input::Guidance(
                "System guidance, not a learner message: the last check passed and another plan step remains. Continue teaching and pose its check now; do not wait for the learner to prompt you.".into(),
            ))?;
            turn.steps.push(pending);
            return Ok((None, Vec::new()));
        }
        turn.steps.push(pending);
        Ok((Some(TurnEnd::AwaitingLearner), Vec::new()))
    }

    /// A reply that posed nothing after the explanation: ask again, a bounded
    /// number of times, rather than leave the lesson without its check.
    fn pose_again(
        &mut self,
        caller: &Caller,
        mut pending: PendingStep,
        turn: &mut Turn,
    ) -> Result<(Option<TurnEnd>, Vec<ToolResult>), TeachError> {
        debug_assert!(pending.calls.is_empty() && turn.stage == CardStage::Posing);
        turn.pose_attempts += 1;
        if turn.pose_attempts > CHECK_ATTEMPTS {
            return Err(EngineError::Stalled(
                "the teaching agent did not pose the check after explaining".into(),
            )
            .into());
        }
        self.engine
            .deliver(&caller.agent, Input::Guidance(POSE_GUIDANCE.into()))?;
        pending.text.clear();
        turn.steps.push(pending);
        Ok((None, Vec::new()))
    }

    pub(super) fn mark_answer_turn(
        &self,
        session: &str,
        caller: &Caller,
        turn: &mut Turn,
    ) -> Result<(), TeachError> {
        if !agent_for_teacher(caller)
            || db::session(self.conn(), session)?.phase != super::records::Phase::Teach
        {
            return Ok(());
        }
        let latest = history::questions(self.conn(), session)?.pop();
        if latest
            .as_ref()
            .is_some_and(|asked| asked.question.collapsed && asked.answer.is_some())
        {
            turn.answer_turn = self.engine.last_input_is_answer(&caller.agent)?;
        }
        Ok(())
    }

    fn has_next_plan_step(&self, session: &str) -> Result<bool, TeachError> {
        let marks = db::plan_marks(self.conn(), session)?;
        if !marks.last().is_some_and(|mark| mark.passed) {
            return Ok(false);
        }
        let Some((revision, plan)) = db::latest_plan(self.conn(), session)? else {
            return Ok(false);
        };
        let tree = super::plan::tree(revision, &plan, &marks);
        Ok(tree
            .nodes
            .iter()
            .any(|node| node.status == super::records::NodeStatus::Current))
    }

    pub(super) fn reply_text_policy(
        &self,
        session: &str,
        turn: &Turn,
    ) -> Result<ReplyText, TeachError> {
        let row = db::session(self.conn(), session)?;
        if role_of(driving_agent(&row)) != role_of(crate::prompts::Agent::Teacher) {
            return Ok(ReplyText::new(
                self.shown_text(session, turn)?,
                AskPolicy::Pose,
            ));
        }
        Ok(match turn.stage {
            CardStage::Explaining => ReplyText::new(TextPolicy::Explanation, AskPolicy::Withhold),
            CardStage::Posing => ReplyText::new(TextPolicy::Suppress, AskPolicy::Pose),
            CardStage::Open => ReplyText::new(self.shown_text(session, turn)?, AskPolicy::Hold),
        })
    }

    fn shown_text(&self, session: &str, turn: &Turn) -> Result<TextPolicy, TeachError> {
        if turn.recovering_check() {
            return Ok(TextPolicy::Suppress);
        }
        if turn.answer_turn && self.has_next_plan_step(session)? {
            return Ok(TextPolicy::SuppressWithoutCalls);
        }
        Ok(TextPolicy::Keep)
    }

    /// Route a mixed reply to explanation, then to the card.
    fn route_card_stage(
        &mut self,
        caller: &Caller,
        step: &mut Step,
        turn: &mut Turn,
    ) -> Result<bool, TeachError> {
        if !agent_for_teacher(caller) {
            return Ok(false);
        }
        let (next, guidance) = match turn.stage {
            CardStage::Open if asks_with_prose(step) => {
                turn.card_context = held_card_context(step);
                step.text.clear();
                (CardStage::Explaining, EXPLAIN_GUIDANCE)
            }
            CardStage::Explaining if step.text.trim().is_empty() => {
                turn.explain_attempts += 1;
                if turn.explain_attempts > CHECK_ATTEMPTS {
                    return Err(EngineError::Stalled(
                        "the teaching agent did not explain the step before its check".into(),
                    )
                    .into());
                }
                (CardStage::Explaining, EXPLAIN_GUIDANCE)
            }
            CardStage::Explaining => (CardStage::Posing, POSE_GUIDANCE),
            _ => return Ok(false),
        };
        step.calls.retain(|call| !is_ask(call));
        turn.stage = next;
        self.engine.deliver(
            &caller.agent,
            Input::Guidance(format!("{guidance}{}", turn.card_context)),
        )?;
        debug_assert!(!step.calls.iter().any(is_ask), "no ask runs unrouted");
        Ok(step.calls.is_empty())
    }

    fn execute_calls(
        &mut self,
        session: &str,
        caller: &Caller,
        calls: Vec<super::engine::ToolCall>,
        shown: &str,
        pending: &mut PendingStep,
        turn: &mut Turn,
    ) -> Result<(Option<TurnEnd>, Vec<usize>), TeachError> {
        debug_assert!(!calls.is_empty());
        debug_assert!(pending.calls.is_empty());
        let mut end = None;
        let mut owed = Vec::new();
        for call in calls {
            let outcome = self.call_tool(session, caller, &call, shown)?;
            if agent_for_teacher(caller)
                && check_refusal(
                    &call.name,
                    &call.input,
                    &outcome.result.content,
                    outcome.result.is_error,
                )
                .is_some()
            {
                turn.check_attempts = turn.check_attempts.max(1);
            }
            if outcome.ends.is_none() {
                owed.push(pending.calls.len());
            }
            end = end.or(outcome.ends);
            pending.calls.push(PendingCall {
                id: call.id,
                name: call.name,
                input: call.input,
                result: outcome.result.content,
                is_error: outcome.result.is_error,
            });
        }
        Ok((end, owed))
    }

    pub(super) fn require_teaching_check(
        &mut self,
        agent: &str,
        turn: &mut Turn,
    ) -> Result<(), TeachError> {
        if turn.check_attempts == 0 {
            return Ok(());
        }
        if turn.check_attempts > CHECK_ATTEMPTS {
            return Err(EngineError::Stalled(
                "the teaching agent could not pose a fresh check".into(),
            )
            .into());
        }
        let detail = turn.steps.last().and_then(|step| {
            step.calls.iter().find_map(|call| {
                check_refusal(&call.name, &call.input, &call.result, call.is_error)
            })
        });
        let guidance = format!(
            "System guidance, not a learner message: {} Continue this turn by calling noesis.ask with a fresh check whose answer and explanation are not already shown. Do not discuss tool refusals or ask the learner to prompt you.",
            detail.unwrap_or("The check still needs to be posed.")
        );
        self.engine.deliver(agent, Input::Guidance(guidance))?;
        turn.check_attempts += 1;
        Ok(())
    }

    pub(super) fn require_recall_judgment(
        &mut self,
        session: &str,
        agent: &str,
        turn: &mut Turn,
    ) -> Result<(), TeachError> {
        let Some(question) = db::unjudged_recall(self.conn(), session)? else {
            turn.judgment_attempts = 0;
            return Ok(());
        };
        turn.judgment_attempts += 1;
        if turn.judgment_attempts >= JUDGMENT_ATTEMPTS {
            let message = "I couldn't assess your recall answer. Please send a message to retry.";
            self.engine.deliver(agent, Input::Failure(message.into()))?;
            super::sessions::set_turn_pending(self.conn(), session, false)?;
            return Err(EngineError::Failed(format!(
                "the teaching agent did not judge answered recall question {question} after {JUDGMENT_ATTEMPTS} attempts"
            )).into());
        }
        self.guide_recall_judgment(session, agent)?;
        debug_assert!(turn.judgment_attempts < JUDGMENT_ATTEMPTS);
        Ok(())
    }

    pub(super) fn guide_recall_judgment(
        &mut self,
        session: &str,
        agent: &str,
    ) -> Result<(), TeachError> {
        let Some(question) = db::unjudged_recall(self.conn(), session)? else {
            return Ok(());
        };
        let reminder = format!(
            "System guidance, not a learner message: the answered recall question {question} still needs your judgment. Call noesis.judge_recall before moving on or waiting for the learner."
        );
        self.engine.deliver(agent, Input::Guidance(reminder))?;
        Ok(())
    }

    /// The results the engine is owed for a step, sealed against the
    /// secrets live after the step and its own.
    fn engine_results(
        &self,
        session: &str,
        own: &[String],
        step: &PendingStep,
        owed: &[usize],
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
                if check_refusal(&call.name, &call.input, &call.result, true).is_some() {
                    content = json!({"error": "The check overlaps material already shown; ask a different question."});
                }
            } else {
                all.value(&mut content);
            }
            results.push(ToolResult {
                call: call.id.clone(),
                content,
                is_error: call.is_error,
            });
        }
        debug_assert_eq!(results.len(), owed.len());
        Ok(results)
    }

    /// The secrets a turn's tool results are sealed against: what is still
    /// sealed at its end and every sealed text a call of the turn carried.
    pub(super) fn turn_sealer(&self, session: &str, turn: &Turn) -> Result<Sealer, TeachError> {
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
            if let Some(key) = owed.recall_key {
                fields.insert("answer_key".into(), Value::String(key));
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

    /// One model step, admitted before it is made and settled after, and
    /// checked against what `session` holds unrevealed as it starts, so no
    /// reply holding a live secret is kept.
    fn model_step(
        &mut self,
        session: &str,
        agent: &str,
        text_policy: ReplyText,
    ) -> Result<Step, TeachError> {
        let solution = db::active_problem(self.conn(), session)?.map(|row| row.solution);
        let live = Sealer::live(self.live_secrets(session)?, solution.as_deref());
        if self.engine.charges_own_calls() {
            return Ok(self.engine.step(agent, &live, text_policy)?);
        }
        let today = chrono::Local::now().date_naive();
        let estimate = self.engine.estimate(agent);
        let admission = self
            .noesis
            .accountant()
            .admit(Lane::Interactive, estimate, today)?;
        match self.engine.step(agent, &live, text_policy) {
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
pub(super) fn budget_stall(error: &TeachError) -> Option<String> {
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

/// A tool call as the turn ran it, its input and result raw until the
/// turn's results are sealed.
struct PendingCall {
    id: String,
    name: String,
    input: Value,
    result: Value,
    is_error: bool,
}

pub(super) struct PendingStep {
    text: String,
    calls: Vec<PendingCall>,
}

/// What one turn produced, unpublished until [`Turn::seal`].
pub(super) struct Turn {
    pub(super) steps: Vec<PendingStep>,
    /// Every sealed text the turn's calls carried.
    own: Vec<String>,
    judgment_attempts: usize,
    check_attempts: usize,
    answer_turn: bool,
    continuation_attempts: usize,
    stage: CardStage,
    card_context: String,
    explain_attempts: usize,
    pose_attempts: usize,
    /// Where the turn's events start in the pending event list.
    pub(super) events_from: usize,
}

/// Debug shows only counts: the buffer holds raw tool results.
impl std::fmt::Debug for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Turn")
            .field("steps", &self.steps.len())
            .field("secrets", &self.own.len())
            .field("events_from", &self.events_from)
            .finish()
    }
}

impl Turn {
    pub(super) fn needs_card(&self) -> bool {
        self.stage != CardStage::Open || self.recovering_check()
    }

    pub(super) fn recovering_check(&self) -> bool {
        self.check_attempts > 0
    }
    pub(super) fn new(events_from: usize) -> Self {
        Self {
            steps: Vec::new(),
            own: Vec::new(),
            judgment_attempts: 0,
            check_attempts: 0,
            answer_turn: false,
            continuation_attempts: 0,
            stage: CardStage::Open,
            card_context: String::new(),
            explain_attempts: 0,
            pose_attempts: 0,
            events_from,
        }
    }

    /// Every text the turn has kept: its earlier steps' text, unknown tool
    /// names and inputs as the transcript keeps them, and `current`, one per
    /// line so no phrase spans two.
    fn shown_with(&self, current: &str) -> String {
        let mut shown: Vec<String> = Vec::new();
        for step in &self.steps {
            shown.push(step.text.clone());
            for call in &step.calls {
                // A known tool's name is the engine's own, so it shows nothing the model chose.
                if ToolName::parse(&call.name).is_none() {
                    shown.push(call.name.clone());
                }
                if !call.is_error
                    || tools::sealed_texts(&call.name, &call.input).is_none_or(|s| s.is_empty())
                {
                    shown.extend(tools::visible_texts(&call.name, &call.input));
                }
            }
        }
        shown.push(current.to_string());
        shown.join("\n")
    }

    /// The turn's steps as they may be published, each call as
    /// [`sealed_call`] gives it.
    pub(super) fn seal(self, sealer: &Sealer) -> Vec<StepRecord> {
        let count = self.steps.len();
        let steps: Vec<StepRecord> = self
            .steps
            .into_iter()
            .map(|pending| StepRecord {
                text: pending.text,
                calls: pending
                    .calls
                    .into_iter()
                    .map(|call| sealed_call(call, sealer))
                    .collect(),
            })
            .collect();
        debug_assert_eq!(steps.len(), count, "every step is published");
        steps
    }
}

fn held_card_context(step: &Step) -> String {
    let Some(args) = step.calls.iter().find_map(|call| {
        if !is_ask(call) {
            return None;
        }
        match tools::parse(ToolName::Ask, call.input.clone()) {
            Ok(tools::ToolArgs::Ask(args)) => Some(args),
            _ => None,
        }
    }) else {
        return String::new();
    };
    let prompt: String = args.prompt.chars().take(2_000).collect();
    let node = args.plan_node.as_deref().unwrap_or("unspecified");
    format!(
        " The withheld check targets plan node {node:?} and asks: {prompt:?}. Teach that topic without giving away the answer or repeating the question."
    )
}

fn agent_for_teacher(caller: &Caller) -> bool {
    caller.role == role_of(crate::prompts::Agent::Teacher)
}

fn check_refusal<'a>(
    name: &str,
    input: &Value,
    result: &'a Value,
    is_error: bool,
) -> Option<&'a str> {
    if !is_error || ToolName::parse(name) != Some(ToolName::Ask) {
        return None;
    }
    if !matches!(tools::parse(ToolName::Ask, input.clone()),
        Ok(tools::ToolArgs::Ask(args)) if args.plan_node.is_some())
    {
        return None;
    }
    result
        .get("error")
        .and_then(Value::as_str)
        .filter(|text| text.contains("gives away"))
}

/// A call as it may be published: a pose's input withheld whole, since its
/// event publishes the view, and its result sealed by `sealer`.
fn sealed_call(call: PendingCall, sealer: &Sealer) -> CallRecord {
    let mut result = call.result;
    if call.is_error {
        sealer.refusal(&mut result);
    } else {
        sealer.value(&mut result);
    }
    let input = tools::seal_input(&call.name, &call.input);
    debug_assert!(
        call.is_error || !sealer.reveals(&result),
        "a result is sealed whole"
    );
    debug_assert!(
        tools::sealed_texts(&call.name, &call.input).is_some_and(|own| own.is_empty())
            || input.as_str() == Some(SEALED),
        "a pose's input is never published"
    );
    CallRecord {
        id: call.id,
        name: call.name,
        input,
        result,
        is_error: call.is_error,
    }
}
