//! The submission ledger's row: read and rewritten inside the transaction
//! that records or drops the receipts it counts.

use super::SqliteStore;
use crate::{Result, StoreError};
use aether_core::idempotency::SubmissionLedger;
use aether_core::time::{Timestamp, now};
use rusqlite::{Connection, OptionalExtension};

const LEDGER_KEY: &str = "submissions.ledger";

/// The newest receipt a sweep dropped: its ordinal and its `at`, as stored.
pub(super) type NewestDropped = (u64, String);

/// An ordinal as SQLite holds it. Receipts are numbered from 1, so a value
/// outside `i64` or below zero was not written by this store.
pub(super) fn ordinal_to_sql(ordinal: u64) -> Result<i64> {
    i64::try_from(ordinal)
        .map_err(|_| StoreError::Corrupt(format!("receipt ordinal {ordinal} overflows the table")))
}

pub(super) fn ordinal_from_sql(raw: i64) -> Result<u64> {
    u64::try_from(raw)
        .map_err(|_| StoreError::Corrupt(format!("submissions.ordinal is negative: {raw}")))
}

impl SqliteStore {
    /// Give this store a ledger of its own at open. One that is missing or
    /// names another store vouches for none of the receipts already here.
    pub(super) fn ensure_submission_ledger(conn: &Connection) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let store: String = tx.query_row(
            "SELECT value FROM meta WHERE key = 'instance_id'",
            [],
            |r| r.get(0),
        )?;
        if store.is_empty() {
            return Err(StoreError::Corrupt(
                "meta.instance_id is empty; run `aether doctor` and `aether diagnose`".into(),
            ));
        }
        let found = Self::read_ledger(&tx)?;
        if found.as_ref().is_some_and(|ledger| ledger.store == store) {
            return Ok(());
        }
        let numbered = ordinal_from_sql(tx.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) FROM submissions",
            [],
            |r| r.get(0),
        )?)?;
        let floor = found.as_ref().map_or(0, |ledger| ledger.recorded);
        if let Some(foreign) = &found {
            tracing::warn!(
                ledger_store = %foreign.store,
                %store,
                "the idempotency ledger belongs to another store; starting a new one that \
                 vouches for no receipt recorded so far"
            );
        }
        let ledger = SubmissionLedger::start(&store, floor.max(numbered), now());
        debug_assert_eq!(ledger.recorded, ledger.forgotten);
        Self::write_ledger(&tx, &ledger)?;
        tx.commit()?;
        Ok(())
    }

    fn read_ledger(tx: &Connection) -> Result<Option<SubmissionLedger>> {
        let raw: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key = ?1", [LEDGER_KEY], |r| {
                r.get(0)
            })
            .optional()?;
        raw.map(|raw| {
            let ledger: SubmissionLedger = serde_json::from_str(&raw).map_err(|e| {
                StoreError::Corrupt(format!("meta.{LEDGER_KEY} does not parse: {e}; run `aether doctor` and `aether diagnose`"))
            })?;
            validate_ledger(&ledger)?;
            Ok(ledger)
        })
        .transpose()
    }

    pub(super) fn ledger_in(tx: &Connection) -> Result<SubmissionLedger> {
        let ledger = Self::read_ledger(tx)?.ok_or_else(|| {
            StoreError::Corrupt(format!(
                "missing meta.{LEDGER_KEY}; run `aether doctor` and `aether diagnose`"
            ))
        })?;
        Ok(ledger)
    }

    fn write_ledger(tx: &Connection, ledger: &SubmissionLedger) -> Result<()> {
        debug_assert!(ledger.forgotten <= ledger.recorded, "{ledger:?}");
        debug_assert!(!ledger.epoch.is_empty() && !ledger.store.is_empty());
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![LEDGER_KEY, serde_json::to_string(ledger)?],
        )?;
        Ok(())
    }

    /// Number the receipt this transaction is about to insert.
    pub(super) fn next_submission_ordinal(tx: &Connection) -> Result<u64> {
        let mut ledger = Self::ledger_in(tx)?;
        if ledger.recorded >= i64::MAX as u64 {
            return Err(StoreError::Corrupt(
                "submission ordinals are exhausted; run `aether doctor` and `aether diagnose`"
                    .into(),
            ));
        }
        let ordinal = ledger.next_ordinal();
        Self::write_ledger(tx, &ledger)?;
        Ok(ordinal)
    }

    /// Advance the watermark past what this transaction's sweep dropped.
    pub(super) fn forget_submissions(tx: &Connection, newest: &NewestDropped) -> Result<()> {
        let (ordinal, at) = newest;
        let at: Timestamp = chrono::DateTime::parse_from_rfc3339(at)
            .map_err(|e| StoreError::Corrupt(format!("submissions.at `{at}` is not a time: {e}")))?
            .with_timezone(&chrono::Utc);
        let mut ledger = Self::ledger_in(tx)?;
        if *ordinal > ledger.recorded {
            return Err(StoreError::Corrupt(format!(
                "dropped receipt {ordinal} is numbered past the {} recorded",
                ledger.recorded
            )));
        }
        ledger.forget(*ordinal, at);
        Self::write_ledger(tx, &ledger)
    }
}

fn validate_ledger(ledger: &SubmissionLedger) -> Result<()> {
    if ledger.store.is_empty() || ledger.epoch.is_empty() {
        return Err(StoreError::Corrupt(
            "submission ledger has an empty identity; run `aether doctor` and `aether diagnose`"
                .into(),
        ));
    }
    if ledger.forgotten > ledger.recorded || ledger.recorded > i64::MAX as u64 {
        return Err(StoreError::Corrupt(format!(
            "submission ledger has invalid ordinals (forgotten {}, recorded {}); run `aether doctor` and `aether diagnose`",
            ledger.forgotten, ledger.recorded
        )));
    }
    Ok(())
}
