//! Repository aliases: an old repository name that resolves to the id its
//! memory now lives under, and the one-time move of what was filed under it.

use std::sync::atomic::Ordering;

use aether_core::event::Actor;
use aether_core::id::EventId;
use aether_core::memory::{MemoryRecord, MemoryScope};
use aether_core::repository::{RepositoryAlias, RepositoryAliasCounts};
use aether_core::time::now;
use rusqlite::{Connection, OptionalExtension, params};

use super::{SqliteStore, nested_scope_range, scope_key};
use crate::{RepositoryAliasMove, Result, StoreError};

impl SqliteStore {
    pub(super) fn resolve_repository_id(&self, repository: &str) -> Result<String> {
        let canonical = canonical_of(&self.read_conn(), repository)?;
        debug_assert!(
            canonical.as_deref() != Some(repository),
            "an alias of itself"
        );
        Ok(canonical.unwrap_or_else(|| repository.to_owned()))
    }

    pub(super) fn list_repository_aliases(&self) -> Result<Vec<RepositoryAlias>> {
        let conn = self.read_conn();
        let aliases: Vec<RepositoryAlias> = Self::read_many(
            &conn,
            "SELECT body FROM repository_aliases ORDER BY alias",
            &[],
        )?;
        if let Some(bad) = aliases.iter().find(|a| !a.is_valid()) {
            return Err(StoreError::Corrupt(format!(
                "repository alias `{}` -> `{}` breaks the alias invariants",
                bad.alias, bad.canonical
            )));
        }
        Ok(aliases)
    }

    pub(super) fn preview_alias(&self, alias: &RepositoryAlias) -> Result<RepositoryAliasMove> {
        assert!(
            alias.is_valid(),
            "a repository alias is validated before the store"
        );
        let conn = self.read_conn();
        let existing = registered(&conn, alias)?;
        let records = filed_under(&conn, &alias.alias)?;
        let counts = tally(&conn, alias, &records)?;
        debug_assert_eq!(counts.memories(), records.len());
        Ok(RepositoryAliasMove {
            counts,
            existing,
            events: Vec::new(),
        })
    }

    /// The checks, the count, the alias row, the move and the event share one
    /// transaction: a failure anywhere leaves every memory where it was.
    pub(super) fn register_alias(
        &self,
        alias: &RepositoryAlias,
        by: Actor,
    ) -> Result<RepositoryAliasMove> {
        assert!(
            alias.is_valid(),
            "a repository alias is validated before the store"
        );
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let existing = registered(&tx, alias)?;
        let records = filed_under(&tx, &alias.alias)?;
        let counts = tally(&tx, alias, &records)?;
        if !existing {
            tx.execute(
                "INSERT INTO repository_aliases(alias, canonical, created_at, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    alias.alias,
                    alias.canonical,
                    alias.created_at.to_rfc3339(),
                    serde_json::to_string(alias)?,
                ],
            )?;
        }
        move_memories(&tx, &alias.canonical, records)?;
        if !filed_under(&tx, &alias.alias)?.is_empty() {
            return Err(StoreError::Corrupt(format!(
                "memories are still filed under `{}` after moving them",
                alias.alias
            )));
        }
        if existing && counts.memories() == 0 {
            return Ok(RepositoryAliasMove {
                counts,
                existing,
                events: Vec::new(),
            });
        }
        let mut seq = Self::last_seq_in(&tx)?;
        let events = Self::write_events(
            &tx,
            &mut seq,
            std::iter::once((EventId::new(), alias.event(&counts, by), now())),
        )?;
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'last_seq'",
            [seq.to_string()],
        )?;
        tx.commit()?;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(RepositoryAliasMove {
            counts,
            existing,
            events,
        })
    }
}

fn canonical_of(conn: &Connection, alias: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT canonical FROM repository_aliases WHERE alias = ?1",
            [alias],
            |row| row.get(0),
        )
        .optional()?)
}

/// Whether `alias` is already registered to the same canonical id. Refuses a
/// different canonical, and either half of a chain: resolution is one hop.
fn registered(conn: &Connection, alias: &RepositoryAlias) -> Result<bool> {
    let refuse = |why: String| Err(StoreError::AliasRefused(why));
    if let Some(current) = canonical_of(conn, &alias.alias)? {
        if current != alias.canonical {
            return refuse(format!(
                "`{}` is already an alias of `{current}`; moved memories do not move back",
                alias.alias
            ));
        }
        return Ok(true);
    }
    if let Some(further) = canonical_of(conn, &alias.canonical)? {
        return refuse(format!(
            "`{}` is itself an alias of `{further}`; alias `{}` to `{further}` instead",
            alias.canonical, alias.alias
        ));
    }
    let aliased: Option<String> = conn
        .query_row(
            "SELECT alias FROM repository_aliases WHERE canonical = ?1 ORDER BY alias LIMIT 1",
            [&alias.alias],
            |row| row.get(0),
        )
        .optional()?;
    match aliased {
        Some(other) => refuse(format!(
            "`{}` is the canonical id of alias `{other}`, so it cannot become an alias itself",
            alias.alias
        )),
        None => Ok(false),
    }
}

/// Every memory filed under `alias`, in any state: its repository scope and
/// each of its repository-role scopes. The key column and the body must agree.
fn filed_under(conn: &Connection, alias: &str) -> Result<Vec<MemoryRecord>> {
    let scope = MemoryScope::Repository {
        id: alias.to_owned(),
    };
    let Some((lower, upper)) = nested_scope_range(&scope) else {
        return Err(StoreError::Corrupt(
            "a repository scope has no role range".into(),
        ));
    };
    let key = scope_key(&scope);
    let records: Vec<MemoryRecord> = SqliteStore::read_many(
        conn,
        "SELECT body FROM memories WHERE scope = ?1 OR (scope >= ?2 AND scope < ?3) ORDER BY rowid",
        &[&key, &lower, &upper],
    )?;
    if let Some(stray) = records
        .iter()
        .find(|r| repository_of(&r.scope) != Some(alias))
    {
        return Err(StoreError::Corrupt(format!(
            "memory {} is keyed under `{alias}` but its body names another scope",
            stray.id
        )));
    }
    Ok(records)
}

fn repository_of(scope: &MemoryScope) -> Option<&str> {
    match scope {
        MemoryScope::Repository { id } | MemoryScope::RepositoryRole { id, .. } => Some(id),
        _ => None,
    }
}

/// `scope` with its repository replaced by `canonical`; the role is kept.
fn rescoped(scope: &MemoryScope, canonical: &str) -> MemoryScope {
    match scope {
        MemoryScope::RepositoryRole { role, .. } => MemoryScope::RepositoryRole {
            id: canonical.to_owned(),
            role: role.clone(),
        },
        _ => MemoryScope::Repository {
            id: canonical.to_owned(),
        },
    }
}

fn tally(
    conn: &Connection,
    alias: &RepositoryAlias,
    records: &[MemoryRecord],
) -> Result<RepositoryAliasCounts> {
    let mut counts = RepositoryAliasCounts::default();
    for record in records {
        match record.scope {
            MemoryScope::Repository { .. } => counts.repository += 1,
            _ => counts.repository_role += 1,
        }
        let target = scope_key(&rescoped(&record.scope, &alias.canonical));
        if record.is_current() && holds_current_text(conn, &target, &record.text)? {
            counts.duplicates += 1;
        }
    }
    counts.agents = conn.query_row(
        "SELECT COUNT(*) FROM agents WHERE json_extract(body, '$.scope.repository') = ?1",
        [&alias.alias],
        |row| row.get::<_, i64>(0),
    )? as usize;
    debug_assert!(counts.duplicates <= counts.memories());
    Ok(counts)
}

fn holds_current_text(conn: &Connection, scope: &str, text: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memories WHERE scope = ?1 AND text = ?2
             AND superseded_by IS NULL AND forgotten_at IS NULL)",
        params![scope, text],
        |row| row.get(0),
    )?)
}

/// Rescope each record, key column and body together. Text, provenance, ids,
/// supersession and curation are carried over as they are.
fn move_memories(conn: &Connection, canonical: &str, records: Vec<MemoryRecord>) -> Result<()> {
    for mut record in records {
        record.scope = rescoped(&record.scope, canonical);
        debug_assert_eq!(repository_of(&record.scope), Some(canonical));
        let changed = conn.execute(
            "UPDATE memories SET scope = ?1, body = ?2 WHERE id = ?3",
            params![
                scope_key(&record.scope),
                serde_json::to_string(&record)?,
                record.id.to_string(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Corrupt(format!(
                "memory {} vanished while it was being moved",
                record.id
            )));
        }
    }
    Ok(())
}
