//! Session reads and deletion for the daemon's session commands, and the
//! hand-over of undelivered inputs between short-lived [`Teaching`] values.

use std::collections::{HashMap, VecDeque};

use rusqlite::{OptionalExtension, params};

use super::TeachError;
use super::db;
use super::engine::{Engine, Input};
use super::records::SessionView;
use super::service::Teaching;

/// Inputs whose delivery failed, by agent, kept between calls by the host.
pub type Undelivered = HashMap<String, VecDeque<Input>>;

/// One page of sessions, oldest first, and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPage {
    pub sessions: Vec<SessionView>,
    pub next_after: Option<String>,
}

impl<E: Engine> Teaching<'_, E> {
    /// Hand this loop the inputs an earlier one could not deliver.
    pub fn restore_undelivered(&mut self, queue: Undelivered) {
        debug_assert!(self.undelivered.is_empty(), "restored once, before use");
        self.undelivered = queue;
    }

    /// The inputs still undelivered, for the next loop over the same store.
    pub fn take_undelivered(&mut self) -> Undelivered {
        std::mem::take(&mut self.undelivered)
    }

    /// Sessions in creation order from just after `after`; `limit` is 1 to 200.
    pub fn list_sessions(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SessionPage, TeachError> {
        if !(1..=200).contains(&limit) {
            return Err(TeachError::Invalid("limit is 1 to 200".into()));
        }
        let conn = self.conn();
        let from: i64 = match after {
            None => 0,
            Some(id) => conn
                .query_row("SELECT rowid FROM sessions WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .optional()?
                .ok_or_else(|| TeachError::NotFound {
                    what: "session",
                    id: id.to_string(),
                })?,
        };
        let wanted = i64::try_from(limit + 1).map_err(|_| TeachError::Invalid("limit".into()))?;
        let mut statement = conn
            .prepare_cached("SELECT id FROM sessions WHERE rowid > ?1 ORDER BY rowid LIMIT ?2")?;
        let ids = statement
            .query_map(params![from, wanted], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let more = ids.len() > limit;
        let mut sessions = Vec::with_capacity(limit.min(ids.len()));
        for id in ids.iter().take(limit) {
            sessions.push(self.session_view(&db::session(conn, id)?)?);
        }
        let next_after = more
            .then(|| sessions.last().map(|s| s.id.clone()))
            .flatten();
        debug_assert!(sessions.len() <= limit && (next_after.is_none() || sessions.len() == limit));
        Ok(SessionPage {
            sessions,
            next_after,
        })
    }

    /// Delete the session's rows (plans, questions, problems and messages go
    /// with it) and forget its agent's undelivered inputs. Returns its agent.
    /// Learner evidence is append-only history and stays.
    pub fn delete_session(&mut self, session: &str) -> Result<String, TeachError> {
        let row = db::session(self.conn(), session)?;
        let deleted = self
            .conn()
            .execute("DELETE FROM sessions WHERE id = ?1", [session])?;
        assert_eq!(deleted, 1, "the session was read just above");
        self.undelivered.remove(&row.agent);
        debug_assert!(db::open_question(self.conn(), session)?.is_none());
        Ok(row.agent)
    }

    /// Every session with an answer committed but not yet delivered.
    pub fn sessions_owing_answers(&self) -> Result<Vec<String>, TeachError> {
        let mut statement = self.conn().prepare_cached(
            "SELECT DISTINCT session FROM questions
             WHERE answered_at IS NOT NULL AND json_extract(answer, '$.delivered') = 0
             ORDER BY session",
        )?;
        let sessions = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        debug_assert!(sessions.windows(2).all(|pair| pair[0] < pair[1]));
        Ok(sessions)
    }
}
