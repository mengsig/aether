//! The teaching loop's reads and writes of `noesis.db` (N1 tables). Sealed
//! values are written here and read back only by the grading and reveal
//! paths; the row types hide them from Debug.

use std::fmt;

use rusqlite::{Connection, OptionalExtension, params};

use super::TeachError;
use super::plan::PlanDag;
use super::records::{
    ChoiceOption, MissClass, Mode, Outcome, Phase, PracticeMode, Problem, Question, QuestionKind,
};
use crate::tools::SEALED;

pub(super) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(super) fn fresh_id(prefix: &str) -> String {
    debug_assert!(prefix.bytes().all(|b| b.is_ascii_lowercase()));
    format!("{prefix}_{}", ulid::Ulid::generate())
}

fn corrupt(what: &str) -> TeachError {
    TeachError::Corrupt(what.to_string())
}

fn json_text<T: serde::Serialize>(value: &T) -> Result<String, TeachError> {
    serde_json::to_string(value)
        .map_err(|error| TeachError::Corrupt(format!("cannot encode a record: {error}")))
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, TeachError> {
    serde_json::from_str(text).map_err(|_| corrupt(what))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionRow {
    pub id: String,
    pub goal: String,
    pub mode: Mode,
    pub phase: Phase,
    pub agent: String,
    pub created_at: String,
    pub updated_at: String,
}

pub(super) fn insert_session(conn: &Connection, row: &SessionRow) -> Result<(), TeachError> {
    conn.execute(
        "INSERT INTO sessions(id, goal, mode, phase, agent, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.id,
            row.goal,
            row.mode.as_str(),
            row.phase.as_str(),
            row.agent,
            row.created_at,
            row.updated_at
        ],
    )?;
    Ok(())
}

pub(super) fn session(conn: &Connection, id: &str) -> Result<SessionRow, TeachError> {
    let raw = conn
        .query_row(
            "SELECT id, goal, mode, phase, agent, created_at, updated_at FROM sessions WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((id, goal, mode, phase, agent, created_at, updated_at)) = raw else {
        return Err(TeachError::NotFound {
            what: "session",
            id: id.to_string(),
        });
    };
    Ok(SessionRow {
        mode: Mode::parse(&mode).ok_or_else(|| corrupt("session mode"))?,
        phase: Phase::parse(&phase).ok_or_else(|| corrupt("session phase"))?,
        id,
        goal,
        agent,
        created_at,
        updated_at,
    })
}

/// Move a session from `from` to `to` under `agent`; false when the session
/// was no longer in `from`, so a racing transition cannot apply twice.
pub(super) fn transition(
    conn: &Connection,
    id: &str,
    from: Phase,
    to: Phase,
    agent: &str,
) -> Result<bool, TeachError> {
    let changed = conn.execute(
        "UPDATE sessions SET phase = ?3, agent = ?4, updated_at = ?5 WHERE id = ?1 AND phase = ?2",
        params![id, from.as_str(), to.as_str(), agent, now()],
    )?;
    debug_assert!(changed <= 1, "session ids are unique");
    Ok(changed == 1)
}

pub(super) fn touch(conn: &Connection, id: &str) -> Result<(), TeachError> {
    conn.execute(
        "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
        params![id, now()],
    )?;
    Ok(())
}

/// A question as stored, sealed values included.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct QuestionRow {
    pub question: Question,
    pub answer_key: Vec<String>,
    pub explanation: Option<String>,
}

impl fmt::Debug for QuestionRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuestionRow")
            .field("question", &self.question)
            .field("answer_key", &SEALED)
            .field("explanation", &SEALED)
            .finish()
    }
}

pub(super) fn insert_question(
    conn: &Connection,
    session: &str,
    row: &QuestionRow,
) -> Result<(), TeachError> {
    let question = &row.question;
    let options = question.options.as_ref().map(json_text).transpose()?;
    let concepts = json_text(&question.concepts)?;
    let answer_key = json_text(&row.answer_key)?;
    conn.execute(
        "INSERT INTO questions(id, session, kind, prompt, options, multi, collapsed, plan_node,
                               concepts, answer_key, explanation, posed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            question.id,
            session,
            question.kind.as_str(),
            question.prompt,
            options,
            question.multi,
            question.collapsed,
            question.plan_node,
            concepts,
            answer_key,
            row.explanation,
            question.posed_at,
        ],
    )?;
    Ok(())
}

/// The session's unanswered question; at most one exists (a unique index).
pub(super) fn open_question(
    conn: &Connection,
    session: &str,
) -> Result<Option<QuestionRow>, TeachError> {
    let raw = conn
        .query_row(
            "SELECT id, kind, prompt, options, multi, collapsed, plan_node, concepts, answer_key,
                    explanation, posed_at
             FROM questions WHERE session = ?1 AND answered_at IS NULL",
            [session],
            RawQuestion::read,
        )
        .optional()?;
    raw.map(RawQuestion::decode).transpose()
}

/// A `questions` row with its JSON columns still encoded.
struct RawQuestion {
    id: String,
    kind: String,
    prompt: String,
    options: Option<String>,
    multi: bool,
    collapsed: bool,
    plan_node: Option<String>,
    concepts: String,
    answer_key: String,
    explanation: Option<String>,
    posed_at: String,
}

impl RawQuestion {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            kind: row.get(1)?,
            prompt: row.get(2)?,
            options: row.get(3)?,
            multi: row.get(4)?,
            collapsed: row.get(5)?,
            plan_node: row.get(6)?,
            concepts: row.get(7)?,
            answer_key: row.get(8)?,
            explanation: row.get(9)?,
            posed_at: row.get(10)?,
        })
    }

    fn decode(self) -> Result<QuestionRow, TeachError> {
        let options: Option<Vec<ChoiceOption>> = match &self.options {
            Some(text) => Some(from_json(text, "question options")?),
            None => None,
        };
        let question = Question {
            kind: QuestionKind::parse(&self.kind).ok_or_else(|| corrupt("question kind"))?,
            concepts: from_json(&self.concepts, "question concepts")?,
            id: self.id,
            prompt: self.prompt,
            options,
            multi: self.multi,
            collapsed: self.collapsed,
            plan_node: self.plan_node,
            posed_at: self.posed_at,
        };
        Ok(QuestionRow {
            question,
            answer_key: from_json(&self.answer_key, "question answer key")?,
            explanation: self.explanation,
        })
    }
}

/// Record the answer to an open question; false if it was already answered.
pub(super) fn record_answer(
    conn: &Connection,
    question: &str,
    answer: &serde_json::Value,
    correct: Option<bool>,
) -> Result<bool, TeachError> {
    let changed = conn.execute(
        "UPDATE questions SET answer = ?2, correct = ?3, answered_at = ?4
         WHERE id = ?1 AND answered_at IS NULL",
        params![question, answer.to_string(), correct, now()],
    )?;
    Ok(changed == 1)
}

/// A committed answer whose delivery to the agent is still owed.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OwedAnswer {
    pub question: String,
    pub record: serde_json::Map<String, serde_json::Value>,
    pub correct: Option<bool>,
}

/// The session's answers recorded with `"delivered": false`, oldest first.
/// Rows without the flag predate it and count as delivered.
pub(super) fn owed_answers(
    conn: &Connection,
    session: &str,
) -> Result<Vec<OwedAnswer>, TeachError> {
    let mut statement = conn.prepare_cached(
        "SELECT id, answer, correct FROM questions
         WHERE session = ?1 AND answered_at IS NOT NULL
           AND json_extract(answer, '$.delivered') = 0
         ORDER BY answered_at, rowid",
    )?;
    let rows = statement.query_map([session], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<bool>>(2)?,
        ))
    })?;
    let mut owed = Vec::new();
    for row in rows {
        let (question, answer, correct) = row?;
        let serde_json::Value::Object(record) = from_json(&answer, "question answer")? else {
            return Err(corrupt("question answer"));
        };
        owed.push(OwedAnswer {
            question,
            record,
            correct,
        });
    }
    Ok(owed)
}

/// Mark an owed answer delivered; an answer is delivered once.
pub(super) fn mark_delivered(conn: &Connection, question: &str) -> Result<(), TeachError> {
    let changed = conn.execute(
        "UPDATE questions SET answer = json_set(answer, '$.delivered', json('true'))
         WHERE id = ?1 AND json_extract(answer, '$.delivered') = 0",
        [question],
    )?;
    if changed != 1 {
        return Err(TeachError::Conflict(format!(
            "the answer to `{question}` is not owed to the agent"
        )));
    }
    Ok(())
}

/// The plan nodes of the session's tagged questions, oldest first.
pub(super) fn tagged_nodes(conn: &Connection, session: &str) -> Result<Vec<String>, TeachError> {
    let mut statement = conn.prepare_cached(
        "SELECT plan_node FROM questions WHERE session = ?1 AND plan_node IS NOT NULL ORDER BY rowid",
    )?;
    let nodes = statement.query_map([session], |row| row.get::<_, String>(0))?;
    Ok(nodes.collect::<Result<_, _>>()?)
}

pub(super) fn latest_plan(
    conn: &Connection,
    session: &str,
) -> Result<Option<(u32, PlanDag)>, TeachError> {
    let raw = conn
        .query_row(
            "SELECT revision, tree FROM plans WHERE session = ?1 ORDER BY revision DESC LIMIT 1",
            [session],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    match raw {
        Some((revision, tree)) => Ok(Some((revision, from_json(&tree, "plan tree")?))),
        None => Ok(None),
    }
}

pub(super) fn insert_plan(
    conn: &Connection,
    session: &str,
    revision: u32,
    dag: &PlanDag,
) -> Result<(), TeachError> {
    conn.execute(
        "INSERT INTO plans(session, revision, tree, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![session, revision, json_text(dag)?, now()],
    )?;
    Ok(())
}

/// A problem as stored, its sealed solution included.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProblemRow {
    pub problem: Problem,
    pub solution: String,
}

impl fmt::Debug for ProblemRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProblemRow")
            .field("problem", &self.problem)
            .field("solution", &SEALED)
            .finish()
    }
}

pub(super) fn insert_problem(
    conn: &Connection,
    session: &str,
    row: &ProblemRow,
) -> Result<(), TeachError> {
    let problem = &row.problem;
    conn.execute(
        "INSERT INTO problems(id, session, statement, mode, targets, solution, posed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            problem.id,
            session,
            problem.statement,
            problem.mode.as_str(),
            json_text(&problem.targets)?,
            row.solution,
            problem.posed_at,
        ],
    )?;
    Ok(())
}

pub(super) fn active_problem(
    conn: &Connection,
    session: &str,
) -> Result<Option<ProblemRow>, TeachError> {
    let raw = conn
        .query_row(
            "SELECT id, statement, mode, targets, solution, posed_at
             FROM problems WHERE session = ?1 AND concluded_at IS NULL",
            [session],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, statement, mode, targets, solution, posed_at)) = raw else {
        return Ok(None);
    };
    let problem = Problem {
        mode: PracticeMode::parse(&mode).ok_or_else(|| corrupt("problem mode"))?,
        targets: from_json(&targets, "problem targets")?,
        id,
        statement,
        posed_at,
    };
    Ok(Some(ProblemRow { problem, solution }))
}

pub(super) struct Conclusion {
    pub outcome: Outcome,
    pub rung: Option<u8>,
    pub miss_class: Option<MissClass>,
    pub retest: bool,
}

/// Close an active problem; false if it was already concluded.
pub(super) fn conclude_problem(
    conn: &Connection,
    problem: &str,
    conclusion: &Conclusion,
) -> Result<bool, TeachError> {
    let changed = conn.execute(
        "UPDATE problems SET outcome = ?2, rung = ?3, miss_class = ?4, retest = ?5, concluded_at = ?6
         WHERE id = ?1 AND concluded_at IS NULL",
        params![
            problem,
            conclusion.outcome.as_str(),
            conclusion.rung,
            conclusion.miss_class.map(MissClass::as_str),
            conclusion.retest,
            now(),
        ],
    )?;
    Ok(changed == 1)
}

/// A practice target named by id, name or alias, as its proposition id.
pub(super) fn resolve_target(
    conn: &Connection,
    target: &str,
) -> Result<Option<String>, TeachError> {
    let id = conn
        .query_row(
            "SELECT id FROM propositions WHERE id = ?1 OR lower(name) = lower(?1)
             UNION
             SELECT proposition FROM proposition_aliases WHERE lower(alias) = lower(?1)
             ORDER BY 1 LIMIT 1",
            [target.trim()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(id)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct Found {
    pub id: String,
    pub name: String,
    pub statement: String,
}

/// Propositions whose name, statement or an alias holds `query`.
pub(super) fn search_propositions(
    conn: &Connection,
    query: &str,
    limit: u32,
) -> Result<Vec<Found>, TeachError> {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let mut statement = conn.prepare_cached(
        "SELECT p.id, p.name, p.statement FROM propositions p
         WHERE p.name LIKE ?1 ESCAPE '\\' OR p.statement LIKE ?1 ESCAPE '\\'
            OR EXISTS (SELECT 1 FROM proposition_aliases a
                       WHERE a.proposition = p.id AND a.alias LIKE ?1 ESCAPE '\\')
         ORDER BY p.name, p.id LIMIT ?2",
    )?;
    let rows = statement.query_map(params![pattern, limit], |row| {
        Ok(Found {
            id: row.get(0)?,
            name: row.get(1)?,
            statement: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
