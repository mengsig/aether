//! The graph's rows in `noesis.db` (N1 tables): load it whole, write a merge
//! as a diff inside the caller's transaction, coverage, deletions and the
//! filtered snapshot `noesis_graph` reads. Nothing here touches a learner table.

use std::collections::{BTreeMap, BTreeSet};

use aether_core::noesis::{Chain, Proposition, Provenance};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::{ChainNode, Graph, Node, Origin};

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("noesis.db holds a malformed graph row: {0}")]
    Corrupt(String),
}

/// The timestamp format of every `noesis.db` row.
pub fn now_text() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_time(text: &str) -> Result<DateTime<Utc>, GraphError> {
    DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| GraphError::Corrupt(format!("timestamp `{text}`")))
}

fn origins(conn: &Connection, sql: &str) -> Result<BTreeMap<String, BTreeSet<Origin>>, GraphError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        let origin = Origin {
            source: row.get(1)?,
            message_ref: row.get(2)?,
        };
        Ok((row.get::<_, String>(0)?, origin))
    })?;
    let mut by_owner: BTreeMap<String, BTreeSet<Origin>> = BTreeMap::new();
    for row in rows {
        let (owner, origin) = row?;
        by_owner.entry(owner).or_default().insert(origin);
    }
    Ok(by_owner)
}

/// The whole graph as the merge sees it.
pub fn load(conn: &Connection) -> Result<Graph, GraphError> {
    let mut graph = Graph::default();
    let mut statement = conn.prepare("SELECT id, name, statement FROM propositions")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    for row in rows {
        let (id, name, statement): (String, String, String) = row?;
        let node = Node {
            name,
            statement,
            aliases: BTreeSet::new(),
            provenance: BTreeSet::new(),
        };
        graph.nodes.insert(id, node);
    }
    let mut aliases = conn.prepare("SELECT proposition, alias FROM proposition_aliases")?;
    let rows = aliases.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?;
    for row in rows {
        let (id, alias) = row?;
        let node = graph
            .nodes
            .get_mut(&id)
            .ok_or_else(|| orphan("alias", &id))?;
        node.aliases.insert(alias);
    }
    let sql = "SELECT proposition, source, message_ref FROM proposition_provenance";
    for (id, set) in origins(conn, sql)? {
        let node = graph
            .nodes
            .get_mut(&id)
            .ok_or_else(|| orphan("provenance", &id))?;
        node.provenance = set;
    }
    load_chains(conn, &mut graph)?;
    graph.check_structure().map_err(GraphError::Corrupt)?;
    Ok(graph)
}

fn orphan(what: &str, id: &str) -> GraphError {
    GraphError::Corrupt(format!("{what} of a missing proposition {id}"))
}

fn load_chains(conn: &Connection, graph: &mut Graph) -> Result<(), GraphError> {
    let mut statement = conn.prepare("SELECT id, conclusion FROM chains")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    for row in rows {
        let (id, conclusion): (String, String) = row?;
        let chain = ChainNode {
            premises: BTreeSet::new(),
            conclusion,
            provenance: BTreeSet::new(),
        };
        graph.chains.insert(id, chain);
    }
    let mut premises = conn.prepare("SELECT chain, premise FROM chain_premises")?;
    let rows = premises.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?;
    for row in rows {
        let (id, premise) = row?;
        let chain = graph.chains.get_mut(&id).ok_or_else(|| orphan_chain(&id))?;
        chain.premises.insert(premise);
    }
    let sql = "SELECT chain, source, message_ref FROM chain_provenance";
    for (id, set) in origins(conn, sql)? {
        graph
            .chains
            .get_mut(&id)
            .ok_or_else(|| orphan_chain(&id))?
            .provenance = set;
    }
    Ok(())
}

fn orphan_chain(id: &str) -> GraphError {
    GraphError::Corrupt(format!("a row of a missing chain {id}"))
}

/// Write `after` over `before` (the graph the merge read, in the same
/// transaction): new rows are inserted, dropped chains deleted. A merge
/// never renames or rewords an existing proposition.
pub fn write_diff(
    conn: &Connection,
    before: &Graph,
    after: &Graph,
    now: &str,
) -> Result<(), GraphError> {
    assert!(before.nodes.keys().all(|id| after.nodes.contains_key(id)));
    for (id, node) in &after.nodes {
        let old = before.nodes.get(id);
        if let Some(old) = old {
            assert_eq!((&old.name, &old.statement), (&node.name, &node.statement));
        } else {
            conn.execute(
                "INSERT INTO propositions (id, name, statement, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, node.name, node.statement, now],
            )?;
        }
        for alias in node
            .aliases
            .iter()
            .filter(|a| old.is_none_or(|o| !o.aliases.contains(*a)))
        {
            conn.execute(
                "INSERT INTO proposition_aliases (proposition, alias) VALUES (?1, ?2)",
                params![id, alias],
            )?;
        }
        let known = old.map(|o| &o.provenance);
        insert_origins(
            conn,
            "proposition_provenance",
            "proposition",
            id,
            &node.provenance,
            known,
        )?;
    }
    for id in before
        .chains
        .keys()
        .filter(|id| !after.chains.contains_key(*id))
    {
        conn.execute("DELETE FROM chains WHERE id = ?1", [id])?;
    }
    for (id, chain) in &after.chains {
        let old = before.chains.get(id);
        if old.is_none() {
            insert_chain(conn, id, chain, now)?;
        }
        let known = old.map(|o| &o.provenance);
        insert_origins(
            conn,
            "chain_provenance",
            "chain",
            id,
            &chain.provenance,
            known,
        )?;
    }
    Ok(())
}

fn insert_chain(
    conn: &Connection,
    id: &str,
    chain: &ChainNode,
    now: &str,
) -> Result<(), GraphError> {
    debug_assert!(!chain.premises.is_empty());
    conn.execute(
        "INSERT INTO chains (id, conclusion, created_at) VALUES (?1, ?2, ?3)",
        params![id, chain.conclusion, now],
    )?;
    for premise in &chain.premises {
        conn.execute(
            "INSERT INTO chain_premises (chain, premise) VALUES (?1, ?2)",
            params![id, premise],
        )?;
    }
    Ok(())
}

fn insert_origins(
    conn: &Connection,
    table: &str,
    owner_column: &str,
    owner: &str,
    origins: &BTreeSet<Origin>,
    known: Option<&BTreeSet<Origin>>,
) -> Result<(), GraphError> {
    let sql =
        format!("INSERT INTO {table} ({owner_column}, source, message_ref) VALUES (?1, ?2, ?3)");
    for origin in origins
        .iter()
        .filter(|o| known.is_none_or(|k| !k.contains(*o)))
    {
        conn.execute(&sql, params![owner, origin.source, origin.message_ref])?;
    }
    Ok(())
}

/// One message (or source chunk) the extraction has read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Covered {
    pub source: String,
    pub message_ref: String,
    pub fingerprint: String,
    pub tokens: u64,
}

/// Record `covered` as read; written last, in the merge's transaction.
pub fn write_coverage(conn: &Connection, covered: &[Covered], now: &str) -> Result<(), GraphError> {
    for entry in covered {
        assert_eq!(
            entry.fingerprint.len(),
            64,
            "a coverage fingerprint is a SHA-256"
        );
        let tokens = i64::try_from(entry.tokens).unwrap_or(i64::MAX);
        conn.execute(
            "INSERT INTO coverage (source, message_ref, fingerprint, tokens, at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source, message_ref) DO UPDATE SET
                fingerprint = excluded.fingerprint, tokens = excluded.tokens, at = excluded.at",
            params![
                entry.source,
                entry.message_ref,
                entry.fingerprint,
                tokens,
                now
            ],
        )?;
    }
    Ok(())
}

/// The fingerprint recorded for each covered message of `source`.
pub fn coverage_of(
    conn: &Connection,
    source: &str,
) -> Result<BTreeMap<String, String>, GraphError> {
    let mut statement =
        conn.prepare_cached("SELECT message_ref, fingerprint FROM coverage WHERE source = ?1")?;
    let rows = statement.query_map([source], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Delete a proposition; its chains go with it. Whether it existed.
pub fn delete_proposition(conn: &Connection, id: &str) -> Result<bool, GraphError> {
    let deleted = conn.execute("DELETE FROM propositions WHERE id = ?1", [id])?;
    debug_assert!(deleted <= 1, "ids are unique");
    let left: u32 = conn.query_row(
        "SELECT count(*) FROM chain_premises WHERE premise = ?1",
        [id],
        |row| row.get(0),
    )?;
    assert_eq!(left, 0, "no chain survives its premise");
    Ok(deleted == 1)
}

pub use snapshot::{Snapshot, SnapshotFilter, snapshot};

mod snapshot;
mod sources;

pub use sources::{SourceRow, delete_source, insert_source, source};

pub(crate) fn optional_text(
    conn: &Connection,
    sql: &str,
    key: &str,
) -> Result<Option<String>, GraphError> {
    Ok(conn.query_row(sql, [key], |row| row.get(0)).optional()?)
}

pub(crate) fn wire_provenance(origins: &BTreeSet<Origin>) -> Vec<Provenance> {
    origins
        .iter()
        .map(|origin| Provenance {
            source: origin.source.clone(),
            message_ref: (!origin.message_ref.is_empty()).then(|| origin.message_ref.clone()),
        })
        .collect()
}

pub(crate) fn wire_proposition(
    id: &str,
    node: &Node,
    created_at: &str,
) -> Result<Proposition, GraphError> {
    Ok(Proposition {
        id: id.to_string(),
        name: node.name.clone(),
        statement: node.statement.clone(),
        aliases: node.aliases.iter().cloned().collect(),
        provenance: wire_provenance(&node.provenance),
        created_at: parse_time(created_at)?,
    })
}

pub(crate) fn wire_chain(
    id: &str,
    chain: &ChainNode,
    created_at: &str,
) -> Result<Chain, GraphError> {
    Ok(Chain {
        id: id.to_string(),
        premises: chain.premises.iter().cloned().collect(),
        conclusion: chain.conclusion.clone(),
        provenance: wire_provenance(&chain.provenance),
        created_at: parse_time(created_at)?,
    })
}
