use super::*;

pub(super) type Arguments = Vec<Box<dyn rusqlite::ToSql>>;

impl SqliteStore {
    pub(super) fn query_events(
        &self,
        after: Option<u64>,
        before: Option<u64>,
        limit: usize,
        filter: &EventFilter,
        ascending: bool,
    ) -> Result<Vec<Event>> {
        if limit == 0 || before == Some(0) || after.is_some_and(|seq| seq >= i64::MAX as u64) {
            return Ok(Vec::new());
        }
        let mut args: Arguments = Vec::new();
        let predicate = filter_predicate(filter, &mut args);
        let filter_len = args.len();
        let after = after.unwrap_or(0);
        let sql = if filter.agent.is_some() && !filter.exclude_children {
            family_page_sql(filter, after, before, limit, ascending, &mut args)
        } else {
            page_sql(
                "seq, id, agent_id, body, type, parent_agent_id, run",
                &predicate,
                after,
                before,
                limit,
                ascending,
                &mut args,
            )
        };
        let mut conn = self.read_conn();
        // Both reads must see the same history, even if another connection is
        // pruning between statements. No write is performed by this transaction.
        let tx = conn.transaction()?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
        let events = read_events(&tx, &sql, &refs)?;
        let mut lower = after + 1;
        let mut upper = before.map_or(i64::MAX as u64, |seq| (seq - 1).min(i64::MAX as u64));
        if events.len() == limit
            && let Some(last) = events.last()
        {
            if ascending {
                upper = last.seq;
            } else {
                lower = last.seq;
            }
        }
        args.truncate(filter_len);
        args.push(Box::new(lower as i64));
        args.push(Box::new(upper as i64));
        let sql = format!(
            "SELECT MIN(first_seq), MAX(last_seq) FROM expired_event_spans WHERE {predicate} AND last_seq >= ?{} AND first_seq <= ?{}",
            filter_len + 1,
            filter_len + 2
        );
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
        let expired: (Option<i64>, Option<i64>) =
            tx.query_row(&sql, refs.as_slice(), |row| Ok((row.get(0)?, row.get(1)?)))?;
        if let (Some(first_seq), Some(last_seq)) = expired {
            return Err(StoreError::HistoryExpired {
                first_seq: first_seq as u64,
                last_seq: last_seq as u64,
            });
        }
        tx.commit()?;
        Ok(events)
    }
}

fn read_events(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<Event>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query(params)?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let owner: Option<String> = row.get(2)?;
        let body: String = row.get(3)?;
        let event: Event = super::event_blobs::decode(conn, &body)?;
        let kind: String = row.get(4)?;
        let parent: Option<String> = row.get(5)?;
        let run: Option<String> = row.get(6)?;
        if seq < 0
            || event.seq != seq as u64
            || event.id.to_string() != id
            || event.agent_id.map(|id| id.to_string()) != owner
            || event.kind.type_name() != kind
            || event.parent_agent_id.map(|id| id.to_string()) != parent
            || event.run.map(|id| id.to_string()) != run
        {
            return Err(StoreError::Corrupt(format!(
                "Event {seq} has a mismatched sequence, identity, type or ownership; run `aether doctor` and `aether diagnose`"
            )));
        }
        events.push(event);
    }
    Ok(events)
}

/// One page of an agent's events and its children's. Each indexed branch is
/// bounded first and merged on `seq` alone: OR, or a UNION of bodies, sorts them all.
pub(super) fn family_page_sql(
    filter: &EventFilter,
    after: u64,
    before: Option<u64>,
    limit: usize,
    ascending: bool,
    args: &mut Arguments,
) -> String {
    debug_assert!(filter.agent.is_some() && !filter.exclude_children);
    let first = args.len();
    let own = filter_predicate_with_agent(filter, args, Some("agent_id"));
    let child = filter_predicate_with_agent(filter, args, Some("parent_agent_id"));
    let own = page_sql("seq", &own, after, before, limit, ascending, args);
    let child = page_sql("seq", &child, after, before, limit, ascending, args);
    args.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));
    debug_assert!(
        args.len() > first + 4,
        "both branches bind an agent and bounds"
    );
    let order = if ascending { "ASC" } else { "DESC" };
    format!(
        "SELECT seq, id, agent_id, body, type, parent_agent_id, run FROM events WHERE seq IN (SELECT seq FROM ({own}) UNION SELECT seq FROM ({child})) ORDER BY seq {order} LIMIT ?{}",
        args.len()
    )
}

/// One page of `columns` from events matching `predicate`, newest or oldest
/// first. Pushes its bounds after the filter's arguments.
pub(super) fn page_sql(
    columns: &str,
    predicate: &str,
    after: u64,
    before: Option<u64>,
    limit: usize,
    ascending: bool,
    args: &mut Arguments,
) -> String {
    args.push(Box::new(after as i64));
    let after_arg = args.len();
    let upper = if let Some(before) = before.filter(|seq| *seq <= i64::MAX as u64) {
        args.push(Box::new(before as i64));
        format!(" AND seq < ?{}", args.len())
    } else {
        String::new()
    };
    args.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));
    let order = if ascending { "ASC" } else { "DESC" };
    format!(
        "SELECT {columns} FROM events WHERE {predicate} AND seq > ?{after_arg}{upper} ORDER BY seq {order} LIMIT ?{}",
        args.len()
    )
}

/// Both event payloads and expiry spans use exactly the same filter semantics.
pub(super) fn filter_predicate(filter: &EventFilter, args: &mut Arguments) -> String {
    filter_predicate_with_agent(filter, args, None)
}

fn filter_predicate_with_agent(
    filter: &EventFilter,
    args: &mut Arguments,
    agent_column: Option<&str>,
) -> String {
    let mut sql = String::from("1 = 1");
    if let Some(agent) = filter.agent {
        args.push(Box::new(agent.to_string()));
        let n = args.len();
        match agent_column {
            Some("parent_agent_id") => {
                let types = child_types(filter, args);
                sql.push_str(&format!(" AND parent_agent_id = ?{n}{types}"));
            }
            Some(column) => sql.push_str(&format!(" AND {column} = ?{n}")),
            None if filter.exclude_children => sql.push_str(&format!(" AND agent_id = ?{n}")),
            None => {
                let types = child_types(filter, args);
                sql.push_str(&format!(
                    " AND (agent_id = ?{n} OR (parent_agent_id = ?{n}{types}))"
                ));
            }
        }
    }
    for (column, value) in [
        ("task_id", filter.task.map(|id| id.to_string())),
        ("run", filter.run.map(|id| id.to_string())),
    ] {
        if let Some(value) = value {
            args.push(Box::new(value));
            sql.push_str(&format!(" AND {column} = ?{}", args.len()));
        }
    }
    for (column, values) in [("family", &filter.families), ("type", &filter.types)] {
        if !values.is_empty() {
            sql.push_str(&format!(" AND {}", in_list(column, values, args)));
        }
    }
    sql
}

/// The clause narrowing the children's events to `child_types`, if any.
fn child_types(filter: &EventFilter, args: &mut Arguments) -> String {
    if filter.child_types.is_empty() {
        return String::new();
    }
    format!(" AND {}", in_list("type", &filter.child_types, args))
}

/// `column IN (...)` over `values`, bound as arguments.
fn in_list(column: &str, values: &[String], args: &mut Arguments) -> String {
    debug_assert!(!values.is_empty(), "an empty IN list matches nothing");
    let placeholders: Vec<_> = values
        .iter()
        .map(|value| {
            args.push(Box::new(value.clone()));
            format!("?{}", args.len())
        })
        .collect();
    format!("{column} IN ({})", placeholders.join(","))
}
