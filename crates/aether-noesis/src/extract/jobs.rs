//! Extraction job rows in `noesis.db` (`jobs`, N1) as the wire `Job`.

use aether_core::model::Usage;
use aether_core::noesis::{Job, JobKind, JobState, JobStats};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::ExtractError;
use crate::graph::store::{GraphError, now_text};

fn kind_name(kind: JobKind) -> &'static str {
    match kind {
        JobKind::ExtractSession => "extract_session",
        JobKind::ExtractSource => "extract_source",
        JobKind::ExtractEngineering => "extract_engineering",
    }
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Merging => "merging",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn corrupt(what: &str) -> ExtractError {
    ExtractError::Graph(GraphError::Corrupt(what.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, ExtractError> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .or_else(|_| serde_json::from_str(text))
        .map_err(|_| corrupt(what))
}

fn time(text: &str) -> Result<DateTime<Utc>, ExtractError> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| corrupt("a job timestamp"))
}

const COLUMNS: &str = "id, kind, target, state, stats, error, usage, created_at, updated_at";

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<[Option<String>; 9]> {
    let mut out: [Option<String>; 9] = Default::default();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = row.get(i)?;
    }
    Ok(out)
}

fn job_of(columns: [Option<String>; 9]) -> Result<Job, ExtractError> {
    let [
        id,
        kind,
        target,
        state,
        stats,
        error,
        usage,
        created,
        updated,
    ] = columns;
    let required =
        |value: Option<String>| value.ok_or_else(|| corrupt("a job without a required column"));
    let stats: Option<JobStats> = match stats {
        Some(text) => Some(serde_json::from_str(&text).map_err(|_| corrupt("job stats"))?),
        None => None,
    };
    let usage: Usage = serde_json::from_str(&required(usage)?).map_err(|_| corrupt("job usage"))?;
    Ok(Job {
        id: required(id)?,
        kind: decode(&required(kind)?, "a job kind")?,
        target: required(target)?,
        state: decode(&required(state)?, "a job state")?,
        stats,
        error,
        usage,
        created_at: time(&required(created)?)?,
        updated_at: time(&required(updated)?)?,
    })
}

/// A new queued job on `target`.
pub fn create(conn: &Connection, kind: JobKind, target: &str) -> Result<Job, ExtractError> {
    assert!(!target.is_empty(), "a job reads a session or a source");
    let id = format!("j_{}", ulid::Ulid::generate());
    let now = now_text();
    let usage = serde_json::to_string(&Usage::default()).map_err(|_| corrupt("usage"))?;
    conn.execute(
        "INSERT INTO jobs (id, kind, target, state, usage, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?5)",
        params![id, kind_name(kind), target, usage, now],
    )?;
    get(conn, &id)
}

pub fn get(conn: &Connection, id: &str) -> Result<Job, ExtractError> {
    let sql = format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1");
    let row = conn.query_row(&sql, [id], read).optional()?;
    job_of(row.ok_or_else(|| ExtractError::UnknownJob(id.to_string()))?)
}

/// Jobs, newest first, optionally in one state.
pub fn list(conn: &Connection, state: Option<JobState>) -> Result<Vec<Job>, ExtractError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM jobs WHERE ?1 IS NULL OR state = ?1
         ORDER BY created_at DESC, id DESC LIMIT 500"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([state.map(state_name)], read)?;
    rows.map(|row| job_of(row?)).collect()
}

/// A job that has not ended on `target`, if any.
pub fn live_on(conn: &Connection, target: &str) -> Result<Option<Job>, ExtractError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM jobs WHERE target = ?1
         AND state IN ('queued', 'running', 'merging') ORDER BY created_at LIMIT 1"
    );
    let row = conn.query_row(&sql, [target], read).optional()?;
    row.map(job_of).transpose()
}

/// Move `job` to `state`, recording `error`, `stats` and `usage` as given.
pub fn update(conn: &Connection, job: &Job) -> Result<Job, ExtractError> {
    let stored = get(conn, &job.id)?;
    if stored.state.is_terminal() {
        return Err(ExtractError::Conflict(format!(
            "job {} has already ended",
            job.id
        )));
    }
    let stats = job
        .stats
        .map(|s| serde_json::to_string(&s))
        .transpose()
        .map_err(|_| corrupt("stats"))?;
    let usage = serde_json::to_string(&job.usage).map_err(|_| corrupt("usage"))?;
    let changed = conn.execute(
        "UPDATE jobs SET state = ?2, stats = ?3, error = ?4, usage = ?5, updated_at = ?6 WHERE id = ?1",
        params![job.id, state_name(job.state), stats, job.error, usage, now_text()],
    )?;
    assert_eq!(changed, 1, "the job was read above");
    get(conn, &job.id)
}

/// End the job as cancelled if it is still queued; a running one is asked
/// through its cancel flag instead. Returns the job as it now stands.
pub fn cancel_queued(conn: &Connection, id: &str) -> Result<Job, ExtractError> {
    let mut job = get(conn, id)?;
    if job.state == JobState::Queued {
        job.state = JobState::Cancelled;
        job = update(conn, &job)?;
    }
    Ok(job)
}

/// Jobs a stopped daemon left unfinished end as failed: nothing runs them
/// now, and extracting again resumes from the last committed coverage.
pub fn fail_interrupted(conn: &Connection) -> Result<Vec<Job>, ExtractError> {
    let sql = format!("SELECT {COLUMNS} FROM jobs WHERE state IN ('queued', 'running', 'merging')");
    let mut statement = conn.prepare(&sql)?;
    let live: Vec<Job> = statement
        .query_map([], read)?
        .map(|row| job_of(row?))
        .collect::<Result<_, _>>()?;
    let mut failed = Vec::new();
    for mut job in live {
        job.state = JobState::Failed;
        job.error = Some("interrupted when the service stopped; extract again to resume".into());
        failed.push(update(conn, &job)?);
    }
    Ok(failed)
}
