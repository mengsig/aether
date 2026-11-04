//! The Noesis tool handlers. A refusal is a tool error the model can read and
//! retry; only store failures abort the turn. Refusals never quote a sealed
//! value.

use serde_json::{Value, json};

use super::db::{self, Conclusion, ProblemRow, QuestionRow, SessionRow};
use super::engine::{Engine, Input, ToolCall, ToolResult};
use super::phase::{Trigger, advance};
use super::records::{
    EvidenceDraft, EvidenceKind, EvidenceOutcome, NoesisEvent, Outcome, Phase, Problem, Question,
};
use super::seal::Sealer;
use super::service::{Caller, Teaching, TurnEnd, driving_agent};
use super::{PROBE_DONE_ACK, TeachError, agent_for_role, plan, question};
use crate::prompts::Agent;
use crate::tools::args::{AskArgs, ConcludeArgs, PlanArgs, ProbeDoneArgs, ProblemArgs};
use crate::tools::{self, ToolArgs, ToolName};

/// One tool call's result, for the engine only, and whether it ends the
/// agent's turn. Readers get the sealed [`super::CallRecord`] instead.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CallOutcome {
    pub result: ToolResult,
    /// Set when this call ends the turn: its result arrives later, or never.
    pub ends: Option<TurnEnd>,
}

type Handled = Result<Result<(Value, Option<TurnEnd>), String>, TeachError>;

fn refuse(text: impl Into<String>) -> Handled {
    Ok(Err(text.into()))
}

impl<E: Engine> Teaching<'_, E> {
    /// Run one Noesis tool call from `caller` in `session`.
    pub(crate) fn call_tool(
        &mut self,
        session: &str,
        caller: &Caller,
        call: &ToolCall,
        visible_text: &str,
    ) -> Result<CallOutcome, TeachError> {
        let handled = self.handle(session, caller, call, visible_text)?;
        let (mut content, is_error, ends) = match handled {
            Ok((content, ends)) => (content, false, ends),
            Err(problem) => (json!({ "error": problem }), true, None),
        };
        // A result never repeats a committed sealed text, even where a refusal
        // quotes the model; a refused call committed nothing of its own.
        let mut secrets = self.live_secrets(session)?;
        if is_error {
            Sealer::new(&secrets).refusal(&mut content);
        } else {
            secrets.extend(tools::sealed_texts(&call.name, &call.input).unwrap_or_default());
            Sealer::new(&secrets).value(&mut content);
        }
        debug_assert!(
            !is_error || ends.is_none(),
            "a refused call never ends the turn"
        );
        Ok(CallOutcome {
            result: ToolResult {
                call: call.id.clone(),
                content,
                is_error,
            },
            ends,
        })
    }

    fn handle(
        &mut self,
        session: &str,
        caller: &Caller,
        call: &ToolCall,
        visible_text: &str,
    ) -> Handled {
        let Some(tool) = ToolName::parse(&call.name) else {
            return refuse("this is not a Noesis tool");
        };
        let Some(agent) = agent_for_role(&caller.role) else {
            return refuse("Noesis tools are only for noesis.* roles");
        };
        if !tools::tools_for(agent).contains(&tool) {
            return refuse(format!(
                "{} is not available to {}",
                tool.as_str(),
                caller.role
            ));
        }
        let row = db::session(self.conn(), session)?;
        if tool == ToolName::ProbeDone && row.phase != Phase::Probe {
            return refuse("the probe is already recorded; noesis.probe_done is called once");
        }
        if row.agent != caller.agent || driving_agent(&row) != agent {
            return refuse("this agent no longer drives the session");
        }
        let args = match tools::parse(tool, call.input.clone()) {
            Ok(args) => args,
            Err(error) => return refuse(error.to_string()),
        };
        match args {
            ToolArgs::Ask(args) => self.ask(&row, &args, visible_text),
            ToolArgs::ProbeDone(args) => self.probe_done(&row, &args),
            ToolArgs::Plan(args) => self.plan(&row, &args),
            ToolArgs::Problem(args) => self.problem(&row, args),
            ToolArgs::Conclude(args) => self.conclude(&row, &args),
            ToolArgs::GraphSearch(args) => {
                let found = db::search_propositions(self.conn(), &args.query, args.limit)?;
                Ok(Ok((json!({ "propositions": found }), None)))
            }
            // The learner model arrives with F4; until then it holds nothing.
            ToolArgs::LearnerRead(_) => {
                Ok(Ok((json!({ "concepts": [], "available": false }), None)))
            }
        }
    }

    fn ask(&mut self, row: &SessionRow, args: &AskArgs, visible_text: &str) -> Handled {
        if db::open_question(self.conn(), &row.id)?.is_some() {
            return refuse("a question is already open; wait for its answer");
        }
        if row.phase == Phase::Probe && args.plan_node.is_some() {
            return refuse("probe questions are not tagged with a plan node");
        }
        let plan = db::latest_plan(self.conn(), &row.id)?.map(|(_, dag)| dag);
        let authored = match question::author(args, plan.as_ref(), visible_text) {
            Ok(authored) => authored,
            Err(problems) => {
                return refuse(format!(
                    "{}: {}",
                    ToolName::Ask.as_str(),
                    problems.join("; ")
                ));
            }
        };
        let id = db::fresh_id("q");
        let (options, answer_key) = if authored.options.is_empty() {
            (None, authored.answer_key)
        } else {
            let shuffled = question::shuffle(&id, &authored.options);
            let (shown, key) = question::rekey(&shuffled, &authored.answer_key);
            (Some(shown), key)
        };
        let posed = Question {
            id: id.clone(),
            kind: args.kind,
            prompt: args.prompt.clone(),
            options,
            multi: authored.multi,
            collapsed: question::collapsed(row.phase, args.kind, args.intent),
            plan_node: args.plan_node.clone(),
            concepts: dedup(&args.concepts),
            posed_at: db::now(),
        };
        let stored = QuestionRow {
            question: posed,
            answer_key,
            explanation: args.explanation.clone(),
        };
        // The plan is shown in full while the question is open, and plan
        // events not yet published go out with this pose.
        let mut sealed = question::shown_key(&stored);
        sealed.extend(stored.explanation.iter().cloned());
        sealed.extend(question::model_key_values(args));
        let sealer = Sealer::new(&sealed);
        let shown_plan = plan.as_ref().is_some_and(|plan| plan_holds(plan, &sealer));
        if shown_plan || self.pending_plans_hold(&sealer) {
            return refuse(
                "the plan gives away this question's answer or explanation; rewrite the plan or the question",
            );
        }
        self.commit_pose(row, stored)?;
        let reply = json!({ "question": id, "status": "posed", "next": "the learner's answer arrives as this call's result" });
        Ok(Ok((reply, Some(TurnEnd::AwaitingAnswer))))
    }

    /// Store a pose, advance plan to teach on a tagged question, and emit.
    fn commit_pose(&mut self, row: &SessionRow, stored: QuestionRow) -> Result<(), TeachError> {
        let posed = stored.question.clone();
        let next = if posed.plan_node.is_some() {
            advance(row.phase, Trigger::TaggedQuestion)
        } else {
            row.phase
        };
        let tx = self.conn().unchecked_transaction()?;
        db::insert_question(&tx, &row.id, &stored)?;
        if next != row.phase && !db::transition(&tx, &row.id, row.phase, next, &row.agent)? {
            return Err(TeachError::Conflict(
                "the session changed phase during the pose".into(),
            ));
        }
        db::touch(&tx, &row.id)?;
        tx.commit()?;
        let tagged = posed.plan_node.is_some();
        self.events.push(NoesisEvent::QuestionPosed {
            session: row.id.clone(),
            question: posed,
        });
        if next != row.phase {
            self.events.push(NoesisEvent::PhaseChanged {
                session: row.id.clone(),
                from: row.phase,
                to: next,
            });
        }
        if tagged {
            self.push_plan_event(&row.id)?;
        }
        Ok(())
    }

    fn probe_done(&mut self, row: &SessionRow, args: &ProbeDoneArgs) -> Handled {
        debug_assert_eq!(row.phase, Phase::Probe, "checked before dispatch");
        if db::open_question(self.conn(), &row.id)?.is_some() {
            return refuse("a question is open; wait for its answer before noesis.probe_done");
        }
        let problems = strand_problems(args);
        if !problems.is_empty() {
            return refuse(format!(
                "{}: {}",
                ToolName::ProbeDone.as_str(),
                problems.join("; ")
            ));
        }
        // The Teacher and its hand-off input exist before the phase moves, so a
        // failed wake later finds the input persisted and recovers.
        let teacher = match self.spawn_teacher(row, args) {
            Ok(teacher) => teacher,
            Err(TeachError::Engine(error)) => {
                return refuse(format!("the hand-off failed, call again: {error}"));
            }
            Err(error) => return Err(error),
        };
        if !db::transition(self.conn(), &row.id, Phase::Probe, Phase::Plan, &teacher)? {
            return refuse("the probe is already recorded; noesis.probe_done is called once");
        }
        self.events.push(NoesisEvent::PhaseChanged {
            session: row.id.clone(),
            from: Phase::Probe,
            to: Phase::Plan,
        });
        for strand in &args.strands {
            for misconception in &strand.misconceptions {
                self.evidence
                    .push(observed(&row.id, &strand.strand, misconception));
            }
        }
        Ok(Ok((json!(PROBE_DONE_ACK), Some(TurnEnd::HandedOff))))
    }

    fn spawn_teacher(
        &mut self,
        row: &SessionRow,
        args: &ProbeDoneArgs,
    ) -> Result<String, TeachError> {
        let teacher = self.spawn(Agent::Teacher)?;
        let assessment = json!({ "strands": args.strands, "summary": args.summary });
        let handoff = format!(
            "Learning goal: {}\nThe Prober's assessment:\n{assessment:#}",
            row.goal
        );
        self.engine.deliver(&teacher, Input::Handoff(handoff))?;
        Ok(teacher)
    }

    fn plan(&mut self, row: &SessionRow, args: &PlanArgs) -> Handled {
        // Checked before validation, whose problems quote node ids.
        let live = Sealer::new(self.live_secrets(&row.id)?);
        let named = args.nodes.iter().flat_map(|node| [&node.id, &node.title]);
        let linked = args.edges.iter().flat_map(|edge| [&edge.from, &edge.to]);
        if named.chain(linked).any(|text| live.holds(text)) {
            return refuse("the plan gives away a sealed answer; rewrite it");
        }
        let dag = match plan::validate(args) {
            Ok(dag) => dag,
            Err(problems) => {
                return refuse(format!(
                    "{}: {}",
                    ToolName::Plan.as_str(),
                    problems.join("; ")
                ));
            }
        };
        let revision =
            db::latest_plan(self.conn(), &row.id)?.map_or(1, |(revision, _)| revision + 1);
        let tx = self.conn().unchecked_transaction()?;
        db::insert_plan(&tx, &row.id, revision, &dag)?;
        db::touch(&tx, &row.id)?;
        tx.commit()?;
        self.push_plan_event(&row.id)?;
        Ok(Ok((
            json!({ "revision": revision, "nodes": dag.nodes.len() }),
            None,
        )))
    }

    /// Whether a plan event not yet taken for publishing shows a secret of
    /// `sealer`; a superseded revision posted earlier in the turn counts.
    fn pending_plans_hold(&self, sealer: &Sealer) -> bool {
        self.events.iter().any(|event| match event {
            NoesisEvent::PlanUpdated { plan, .. } => plan
                .nodes
                .iter()
                .any(|node| sealer.holds(&node.id) || sealer.holds(&node.title)),
            _ => false,
        })
    }

    fn push_plan_event(&mut self, session: &str) -> Result<(), TeachError> {
        let Some((revision, dag)) = db::latest_plan(self.conn(), session)? else {
            return Ok(());
        };
        let tree = plan::tree(revision, &dag, &db::tagged_nodes(self.conn(), session)?);
        self.events.push(NoesisEvent::PlanUpdated {
            session: session.to_string(),
            plan: tree,
        });
        Ok(())
    }

    /// The proposition ids a problem's targets name. The ids are stored and
    /// shown with the problem, so one that repeats the solution is refused.
    fn problem_targets(
        &self,
        names: &[String],
        solution: &Sealer,
    ) -> Result<Result<Vec<String>, String>, TeachError> {
        let mut targets = Vec::with_capacity(names.len());
        let mut unknown = Vec::new();
        for name in names {
            match db::resolve_target(self.conn(), name)? {
                Some(id) if !targets.contains(&id) => targets.push(id),
                Some(_) => {}
                None => unknown.push(printable(name)),
            }
        }
        if !unknown.is_empty() {
            let named = unknown.join(", ");
            return Ok(Err(format!(
                "no proposition is named {named}; use noesis.graph_search"
            )));
        }
        if targets.iter().any(|id| solution.holds(id)) {
            return Ok(Err(
                "a target's proposition id gives away the solution; target another proposition"
                    .to_string(),
            ));
        }
        debug_assert!(targets.len() <= names.len(), "targets are deduplicated");
        Ok(Ok(targets))
    }

    fn problem(&mut self, row: &SessionRow, args: ProblemArgs) -> Handled {
        if db::active_problem(self.conn(), &row.id)?.is_some() {
            return refuse("a problem is already active; noesis.conclude it first");
        }
        let solution = Sealer::new([&args.solution]);
        if solution.holds(&args.statement) || args.targets.iter().any(|t| solution.holds(t)) {
            return refuse("the statement or a target gives away the solution; rewrite it");
        }
        let targets = match self.problem_targets(&args.targets, &solution)? {
            Ok(targets) => targets,
            Err(problem) => return refuse(problem),
        };
        let problem = Problem {
            id: db::fresh_id("pb"),
            statement: args.statement,
            mode: args.mode,
            targets,
            posed_at: db::now(),
        };
        let stored = ProblemRow {
            problem: problem.clone(),
            solution: args.solution,
        };
        let tx = self.conn().unchecked_transaction()?;
        db::insert_problem(&tx, &row.id, &stored)?;
        db::touch(&tx, &row.id)?;
        tx.commit()?;
        self.events.push(NoesisEvent::ProblemPosed {
            session: row.id.clone(),
            problem: problem.clone(),
        });
        Ok(Ok((
            json!({ "problem": problem.id, "status": "active" }),
            None,
        )))
    }

    fn conclude(&mut self, row: &SessionRow, args: &ConcludeArgs) -> Handled {
        let Some(active) = db::active_problem(self.conn(), &row.id)? else {
            return refuse("there is no active problem to conclude");
        };
        if args.outcome == Outcome::SolvedWithHints && args.rung.is_none_or(|rung| rung == 0) {
            return refuse("solved_with_hints needs the highest hint rung used, 1 to 5");
        }
        let conclusion = Conclusion {
            outcome: args.outcome,
            rung: args.rung,
            miss_class: args.miss_class,
            retest: args.retest,
        };
        let tx = self.conn().unchecked_transaction()?;
        if !db::conclude_problem(&tx, &active.problem.id, &conclusion)? {
            return refuse("this problem is already concluded");
        }
        db::touch(&tx, &row.id)?;
        tx.commit()?;
        debug_assert!(db::active_problem(self.conn(), &row.id)?.is_none());
        self.events.push(NoesisEvent::PracticeConcluded {
            session: row.id.clone(),
            problem: active.problem.id.clone(),
            outcome: args.outcome,
            rung: args.rung,
            miss_class: args.miss_class,
            retest: args.retest,
            solution: active.solution,
        });
        if let Some(evidence) = practice_evidence(&row.id, &active.problem, args) {
            self.evidence.push(evidence);
        }
        Ok(Ok((
            json!({ "concluded": active.problem.id, "outcome": args.outcome.as_str() }),
            None,
        )))
    }
}

/// Whether a node id or title of `plan` holds any of `secrets`; edges name
/// only node ids, so the nodes cover everything a plan shows.
fn plan_holds(plan: &plan::PlanDag, sealer: &Sealer) -> bool {
    plan.nodes
        .iter()
        .any(|node| sealer.holds(&node.id) || sealer.holds(&node.title))
}

fn strand_problems(args: &ProbeDoneArgs) -> Vec<String> {
    let mut problems = Vec::new();
    if args.strands.is_empty() {
        problems.push("at least one strand is assessed".to_string());
    }
    let mut names = std::collections::HashSet::new();
    for strand in &args.strands {
        if !names.insert(strand.strand.to_lowercase()) {
            problems.push(format!("strand `{}` is listed twice", strand.strand));
        }
    }
    problems
}

fn observed(session: &str, strand: &str, misconception: &str) -> EvidenceDraft {
    EvidenceDraft {
        session: session.to_string(),
        concepts: vec![strand.to_string()],
        kind: EvidenceKind::MisconceptionObserved,
        outcome: EvidenceOutcome::Observed,
        confidence: None,
        dont_know: false,
        mode: None,
        hint_rung: None,
        miss_class: None,
        note: Some(misconception.to_string()),
    }
}

/// A dismissed problem says nothing about the learner.
fn practice_evidence(
    session: &str,
    problem: &Problem,
    args: &ConcludeArgs,
) -> Option<EvidenceDraft> {
    let outcome = match args.outcome {
        Outcome::Solved | Outcome::SolvedWithHints => EvidenceOutcome::Correct,
        Outcome::MovedOn | Outcome::Surrendered => EvidenceOutcome::Incorrect,
        Outcome::Dismissed => return None,
    };
    Some(EvidenceDraft {
        session: session.to_string(),
        concepts: problem.targets.clone(),
        kind: EvidenceKind::PracticeConclude,
        outcome,
        confidence: None,
        dont_know: false,
        mode: Some(problem.mode),
        hint_rung: args.rung,
        miss_class: args.miss_class,
        note: None,
    })
}

fn dedup(items: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::with_capacity(items.len());
    for item in items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        if !unique.iter().any(|seen| seen == item) {
            unique.push(item.to_string());
        }
    }
    unique
}

/// Model text echoed into a refusal, shortened.
fn printable(text: &str) -> String {
    let short: String = text.chars().filter(|c| !c.is_control()).take(80).collect();
    format!("`{short}`")
}
