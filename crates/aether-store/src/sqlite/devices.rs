//! Paired remote devices: one row each, kept after revocation.

use aether_core::id::DeviceId;
use aether_core::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use super::SqliteStore;
use crate::{DeviceRow, Result, StoreError};

const COLUMNS: &str = "id, created_at, revoked_at, last_seen_at, body";

fn timestamp(raw: &str) -> Result<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|at| at.with_timezone(&chrono::Utc))
        .map_err(|e| StoreError::Corrupt(format!("device timestamp `{raw}`: {e}")))
}

type RawRow = (String, String, Option<String>, Option<String>, String);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn decode((id, created_at, revoked_at, last_seen_at, body): RawRow) -> Result<DeviceRow> {
    let id: DeviceId = id
        .parse()
        .map_err(|_| StoreError::Corrupt(format!("device id `{id}`")))?;
    Ok(DeviceRow {
        id,
        created_at: timestamp(&created_at)?,
        revoked_at: revoked_at.as_deref().map(timestamp).transpose()?,
        last_seen_at: last_seen_at.as_deref().map(timestamp).transpose()?,
        record: serde_json::from_str(&body)?,
    })
}

impl SqliteStore {
    pub(super) fn put_device(tx: &Transaction<'_>, device: &DeviceRow) -> Result<()> {
        tx.execute(
            "INSERT INTO devices(id, created_at, revoked_at, body) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET revoked_at = excluded.revoked_at, body = excluded.body",
            params![
                device.id.to_string(),
                device.created_at.to_rfc3339(),
                device.revoked_at.map(|at| at.to_rfc3339()),
                serde_json::to_string(&device.record)?,
            ],
        )?;
        Ok(())
    }

    pub(super) fn touch_device(tx: &Transaction<'_>, id: DeviceId, at: Timestamp) -> Result<()> {
        let changed = tx.execute(
            "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1",
            params![id.to_string(), at.to_rfc3339()],
        )?;
        if changed != 1 {
            return Err(StoreError::Corrupt(format!(
                "no device {id} to mark as seen"
            )));
        }
        Ok(())
    }

    pub(super) fn read_device(conn: &Connection, id: DeviceId) -> Result<Option<DeviceRow>> {
        let sql = format!("SELECT {COLUMNS} FROM devices WHERE id = ?1");
        conn.query_row(&sql, [id.to_string()], raw)
            .optional()?
            .map(decode)
            .transpose()
    }

    pub(super) fn read_devices(conn: &Connection, include_revoked: bool) -> Result<Vec<DeviceRow>> {
        let filter = if include_revoked {
            ""
        } else {
            "WHERE revoked_at IS NULL"
        };
        let sql = format!("SELECT {COLUMNS} FROM devices {filter} ORDER BY created_at ASC, id ASC");
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map([], raw)?;
        rows.map(|row| decode(row?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Commit, Mutation, Store};

    fn row(name: &str, created_at: Timestamp) -> DeviceRow {
        DeviceRow {
            id: DeviceId::new(),
            created_at,
            revoked_at: None,
            last_seen_at: None,
            record: serde_json::json!({ "name": name }),
        }
    }

    fn put(store: &SqliteStore, device: &DeviceRow) {
        store
            .commit(Commit::new().mutate(Mutation::PutDevice(device.clone())))
            .expect("put");
    }

    #[test]
    fn devices_list_oldest_first_and_hide_revoked_unless_asked() {
        let store = SqliteStore::open_in_memory().expect("store");
        let start = aether_core::time::now();
        let old = row("old", start);
        let mut gone = row("gone", start + chrono::Duration::seconds(1));
        let new = row("new", start + chrono::Duration::seconds(2));
        for device in [&new, &gone, &old] {
            put(&store, device);
        }
        gone.revoked_at = Some(start + chrono::Duration::seconds(3));
        put(&store, &gone);

        let names = |rows: Vec<DeviceRow>| -> Vec<String> {
            rows.iter()
                .map(|row| row.record["name"].as_str().expect("name").to_owned())
                .collect()
        };
        assert_eq!(names(store.devices(false).expect("live")), ["old", "new"]);
        assert_eq!(
            names(store.devices(true).expect("all")),
            ["old", "gone", "new"]
        );
        let read = store.device(gone.id).expect("read").expect("kept");
        assert_eq!(
            read.revoked_at.map(|at| at.timestamp()),
            gone.revoked_at.map(|at| at.timestamp())
        );
        assert_eq!(store.device(DeviceId::new()).expect("read"), None);
    }

    #[test]
    fn a_put_never_moves_last_seen_and_a_touch_needs_the_device() {
        let store = SqliteStore::open_in_memory().expect("store");
        let mut device = row("phone", aether_core::time::now());
        put(&store, &device);
        let seen = aether_core::time::now();
        store
            .commit(Commit::new().mutate(Mutation::TouchDevice {
                id: device.id,
                at: seen,
            }))
            .expect("touch");
        device.record = serde_json::json!({ "name": "renamed" });
        put(&store, &device);
        let read = store.device(device.id).expect("read").expect("present");
        assert_eq!(read.record["name"], "renamed");
        assert_eq!(
            read.last_seen_at.map(|at| at.timestamp_millis()),
            Some(seen.timestamp_millis()),
            "a rename racing a hello must not erase when the device was seen"
        );
        let missing = store.commit(Commit::new().mutate(Mutation::TouchDevice {
            id: DeviceId::new(),
            at: seen,
        }));
        assert!(
            matches!(missing, Err(StoreError::Corrupt(_))),
            "{missing:?}"
        );
    }
}
