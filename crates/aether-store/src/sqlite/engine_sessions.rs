use super::write_tally::WriteTally;
use crate::{EngineSessionOwnership, ExpiredEngineSessionOwner, Result, StoreError};
use aether_core::agent::Agent;
use rusqlite::{OptionalExtension, Transaction, params};

pub(super) fn record(
    tx: &Transaction<'_>,
    record: EngineSessionOwnership,
    tally: &mut WriteTally,
) -> Result<()> {
    if record.expired_owner.is_some()
        || !matches!(record.engine.as_str(), "claude-code" | "codex")
        || record.session_id.trim().is_empty()
        || record.log_paths.iter().any(|path| {
            !path.is_absolute()
                || path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
        })
    {
        return Err(StoreError::Corrupt(
            "invalid engine session ownership evidence".into(),
        ));
    }
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
            return Err(StoreError::Corrupt(
                "conflicting engine session ownership evidence".into(),
            ));
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
