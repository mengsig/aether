use crate::{EngineSessionOwnership, Result, StoreError};
use rusqlite::{OptionalExtension, Transaction, params};

pub(super) fn record(tx: &Transaction<'_>, record: EngineSessionOwnership) -> Result<()> {
    if !matches!(record.engine.as_str(), "claude-code" | "codex")
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
            serde_json::to_string(&record)?
        ],
    )?;
    Ok(())
}
