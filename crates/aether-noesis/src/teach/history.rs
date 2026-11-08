//! A session's durable history (N4): the agents that ran it with the inputs
//! Noesis wrote to them, every question with its answer, every problem with
//! its conclusion, and why its last turn stalled. A sealed value leaves the
//! store here only inside the reveal of an answered question or a concluded
//! problem.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

use super::TeachError;
use super::db::{self, RawQuestion};
use super::records::{
    AskedQuestion, MissClass, Outcome, PosedProblem, PracticeMode, Problem, ProblemConclusion,
    RecordedAnswer, SessionAgent, SessionRole, SessionStall,
};
use super::service::reveal;

/// Record that `agent` now runs `session`, after every agent before it.
/// `internal` is the seq of the input Noesis wrote to start it, if any.
pub(super) fn add_agent(
    conn: &Connection,
    session: &str,
    agent: &str,
    role: SessionRole,
    internal: Option<u64>,
) -> Result<(), TeachError> {
    debug_assert!(!agent.is_empty(), "an agent id");
    let internal = db::json_text(&Vec::from_iter(internal))?;
    let added = conn.execute(
        "INSERT INTO session_agents(session, ord, agent, role, internal, started_at)
         SELECT ?1, coalesce(max(ord) + 1, 0), ?2, ?3, ?4, ?5
         FROM session_agents WHERE session = ?1",
        params![session, agent, role.as_str(), internal, db::now()],
    )?;
    debug_assert_eq!(added, 1, "one row per agent");
    Ok(())
}

/// The session's agents in the order they ran.
pub(super) fn agents(conn: &Connection, session: &str) -> Result<Vec<SessionAgent>, TeachError> {
    let mut statement = conn.prepare_cached(
        "SELECT agent, role, internal FROM session_agents WHERE session = ?1 ORDER BY ord",
    )?;
    let rows = statement.query_map([session], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut agents = Vec::new();
    for row in rows {
        let (agent, role, internal) = row?;
        agents.push(SessionAgent {
            agent,
            role: SessionRole::parse(&role).ok_or_else(|| db::corrupt("session agent role"))?,
            internal: db::from_json(&internal, "session agent inputs")?,
        });
    }
    Ok(agents)
}

/// An answer as `Teaching::answer` stores it; rows older than a field lack it.
#[derive(Deserialize)]
struct StoredAnswer {
    #[serde(default)]
    chosen: Vec<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    dont_know: bool,
    #[serde(default)]
    confidence: Option<u8>,
    #[serde(default)]
    thinking: Option<String>,
}

/// Every question of the session, oldest first. Only an answered one
/// carries its answer and the reveal [`reveal`] builds for it.
pub(super) fn questions(
    conn: &Connection,
    session: &str,
) -> Result<Vec<AskedQuestion>, TeachError> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {}, answer, correct, answered_at FROM questions
         WHERE session = ?1 ORDER BY posed_at, rowid",
        RawQuestion::COLUMNS
    ))?;
    let rows = statement.query_map([session], |row| {
        Ok((
            RawQuestion::read(row)?,
            row.get::<_, Option<String>>(RawQuestion::WIDTH)?,
            row.get::<_, Option<bool>>(RawQuestion::WIDTH + 1)?,
            row.get::<_, Option<String>>(RawQuestion::WIDTH + 2)?,
        ))
    })?;
    let mut asked = Vec::new();
    for row in rows {
        let (raw, answer, correct, answered_at) = row?;
        let stored = raw.decode()?;
        let (answer, result) = match (answer, answered_at) {
            (Some(answer), Some(answered_at)) => {
                let answer = recorded(&answer, answered_at)?;
                (Some(answer), Some(reveal(&stored, correct)))
            }
            (None, None) => (None, None),
            _ => return Err(db::corrupt("question answer")),
        };
        debug_assert_eq!(answer.is_some(), result.is_some(), "revealed iff answered");
        asked.push(AskedQuestion {
            question: stored.question,
            answer,
            result,
        });
    }
    Ok(asked)
}

fn recorded(answer: &str, answered_at: String) -> Result<RecordedAnswer, TeachError> {
    let stored: StoredAnswer = db::from_json(answer, "question answer")?;
    debug_assert!(
        !stored.dont_know || stored.confidence.is_none(),
        "don't know carries no confidence"
    );
    Ok(RecordedAnswer {
        chosen: stored.chosen,
        text: stored.text,
        dont_know: stored.dont_know,
        confidence: stored.confidence,
        thinking: stored.thinking,
        answered_at,
    })
}

/// A `problems` row with its encoded columns and optional conclusion.
struct RawProblem {
    id: String,
    statement: String,
    mode: String,
    targets: String,
    solution: String,
    posed_at: String,
    outcome: Option<String>,
    rung: Option<u8>,
    miss_class: Option<String>,
    retest: Option<bool>,
    concluded_at: Option<String>,
}

impl RawProblem {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            statement: row.get(1)?,
            mode: row.get(2)?,
            targets: row.get(3)?,
            solution: row.get(4)?,
            posed_at: row.get(5)?,
            outcome: row.get(6)?,
            rung: row.get(7)?,
            miss_class: row.get(8)?,
            retest: row.get(9)?,
            concluded_at: row.get(10)?,
        })
    }

    /// The problem, and its conclusion only once it has one.
    fn decode(self) -> Result<PosedProblem, TeachError> {
        let problem = Problem {
            mode: PracticeMode::parse(&self.mode).ok_or_else(|| db::corrupt("problem mode"))?,
            targets: db::from_json(&self.targets, "problem targets")?,
            id: self.id,
            statement: self.statement,
            posed_at: self.posed_at,
        };
        let conclusion = match (self.outcome, self.concluded_at) {
            (Some(outcome), Some(concluded_at)) => Some(ProblemConclusion {
                outcome: Outcome::parse(&outcome).ok_or_else(|| db::corrupt("problem outcome"))?,
                rung: self.rung,
                miss_class: self
                    .miss_class
                    .map(|class| MissClass::parse(&class).ok_or_else(|| db::corrupt("miss class")))
                    .transpose()?,
                retest: self.retest.unwrap_or(false),
                solution: self.solution,
                concluded_at,
            }),
            (None, None) => None,
            _ => return Err(db::corrupt("problem conclusion")),
        };
        Ok(PosedProblem {
            problem,
            conclusion,
        })
    }
}

/// Every problem of the session, oldest first. Only a concluded one carries
/// its conclusion and, with it, its solution.
pub(super) fn problems(conn: &Connection, session: &str) -> Result<Vec<PosedProblem>, TeachError> {
    let mut statement = conn.prepare_cached(
        "SELECT id, statement, mode, targets, solution, posed_at, outcome, rung, miss_class,
                retest, concluded_at
         FROM problems WHERE session = ?1 ORDER BY posed_at, rowid",
    )?;
    let rows = statement.query_map([session], RawProblem::read)?;
    let mut posed = Vec::new();
    for row in rows {
        posed.push(row?.decode()?);
    }
    debug_assert!(
        posed.iter().filter(|p| p.conclusion.is_none()).count() <= 1,
        "at most one problem is active"
    );
    Ok(posed)
}

/// Record why the session's turn stalled, replacing an earlier stall.
pub(super) fn set_stall(
    conn: &Connection,
    session: &str,
    stall: &SessionStall,
) -> Result<(), TeachError> {
    debug_assert!(!stall.message.is_empty(), "a stall says what to do");
    let changed = conn.execute(
        "UPDATE sessions SET stalled = ?2 WHERE id = ?1",
        params![session, db::json_text(stall)?],
    )?;
    if changed != 1 {
        return Err(TeachError::NotFound {
            what: "session",
            id: session.to_string(),
        });
    }
    Ok(())
}

/// Forget the session's stall once a turn ran; false when it had none.
pub(super) fn clear_stall(conn: &Connection, session: &str) -> Result<bool, TeachError> {
    let changed = conn.execute(
        "UPDATE sessions SET stalled = NULL WHERE id = ?1 AND stalled IS NOT NULL",
        [session],
    )?;
    debug_assert!(changed <= 1, "session ids are unique");
    Ok(changed == 1)
}

pub(super) fn stall(conn: &Connection, session: &str) -> Result<Option<SessionStall>, TeachError> {
    let stored: Option<Option<String>> = conn
        .query_row(
            "SELECT stalled FROM sessions WHERE id = ?1",
            [session],
            |row| row.get(0),
        )
        .optional()?;
    match stored.flatten() {
        Some(text) => Ok(Some(db::from_json(&text, "session stall")?)),
        None => Ok(None),
    }
}
