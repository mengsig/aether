use super::write_tally::WriteTally;
use crate::{EngineSessionOwnership, ExpiredEngineSessionOwner, Result, StoreError};
use aether_core::agent::Agent;
use rusqlite::{OptionalExtension, Transaction, params};

pub(super) fn record(
    tx: &Transaction<'_>,
    record: EngineSessionOwnership,
    tally: &mut WriteTally,
) -> Result<()> {
    validate(&record).map_err(|why| {
        StoreError::OwnershipRefused(format!(
            "invalid {} session ownership evidence: {why}",
            record.engine
        ))
    })?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT body FROM engine_session_ownership WHERE engine=?1 AND session_id=?2",
            params![record.engine, record.session_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(body) = existing {
        let previous: EngineSessionOwnership = serde_json::from_str(&body)?;
        if previous.agent_id != record.agent_id || previous.log_paths != record.log_paths {
            return Err(StoreError::OwnershipRefused(format!(
                "{} session {} is already owned by agent {} with other evidence",
                record.engine, record.session_id, previous.agent_id
            )));
        }
        return Ok(());
    }
    tx.execute(
        "INSERT INTO engine_session_ownership(engine, session_id, body) VALUES (?1, ?2, ?3)",
        params![
            record.engine,
            record.session_id,
            tally.json("engine_session_ownership", &record)?
        ],
    )?;
    Ok(())
}

/// Each engine's evidence form. A new engine needs its own form here: an
/// unknown one is refused rather than trusted with the loosest rules.
fn validate(record: &EngineSessionOwnership) -> std::result::Result<(), String> {
    if record.expired_owner.is_some() {
        return Err("only history expiry may record an expired owner".into());
    }
    if record.session_id.trim().is_empty() {
        return Err("the session id is empty".into());
    }
    match record.engine.as_str() {
        "claude-code" | "codex" => {
            let unsafe_path = record.log_paths.iter().find(|path| {
                !path.is_absolute()
                    || path
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
            });
            match unsafe_path {
                Some(path) => Err(format!(
                    "log path {} is not absolute and normalized",
                    path.display()
                )),
                None => Ok(()),
            }
        }
        // `agy acp` names sessions by lowercase UUID and keeps them in a
        // database Aether cannot verify, so no log path may be claimed.
        "antigravity" if !is_lowercase_uuid(&record.session_id) => Err(format!(
            "session id {:?} is not a lowercase UUID",
            record.session_id
        )),
        "antigravity" if !record.log_paths.is_empty() => {
            Err("antigravity sessions have no verifiable log paths".into())
        }
        "antigravity" => Ok(()),
        other => Err(format!("engine {other:?} has no ownership evidence form")),
    }
}

fn is_lowercase_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

pub(super) fn preserve_expired_owners(tx: &Transaction<'_>, run: &str) -> Result<()> {
    let mut statement = tx.prepare(
        "SELECT ownership.engine, ownership.session_id, agents.body \
         FROM engine_session_ownership AS ownership \
         JOIN agents ON agents.id=json_extract(ownership.body,'$.agent_id') \
         WHERE agents.run=?1",
    )?;
    let owners = statement
        .query_map([run], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (engine, session_id, body) in owners {
        let agent: Agent = serde_json::from_str(&body)?;
        let evidence = ExpiredEngineSessionOwner {
            status: agent.status.kind(),
            updated_at: agent
                .updated_at
                .max(agent.status_since.unwrap_or(agent.updated_at)),
        };
        tx.execute(
            "UPDATE engine_session_ownership \
             SET body=json_set(body,'$.expired_owner',json(?3)) \
             WHERE engine=?1 AND session_id=?2",
            params![engine, session_id, serde_json::to_string(&evidence)?],
        )?;
    }
    Ok(())
}
