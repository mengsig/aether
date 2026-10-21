//! Web Push subscriptions: at most one per paired device. The body is opaque
//! here: the endpoint and keys as the push layer serialized them.

use aether_core::id::DeviceId;
use aether_core::time::Timestamp;
use rusqlite::{OptionalExtension, Row, params};

use super::SqliteStore;
use crate::{Result, StoreError};

/// One device's subscription as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscriptionRow {
    pub device: DeviceId,
    pub updated_at: Timestamp,
    /// Endpoint, keys and wanted kinds, as the push layer serialized them.
    pub body: serde_json::Value,
}

type RawRow = (String, String, String);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn decode((device, updated_at, body): RawRow) -> Result<PushSubscriptionRow> {
    let device: DeviceId = device
        .parse()
        .map_err(|_| StoreError::Corrupt(format!("push subscription device `{device}`")))?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at)
        .map(|at| at.with_timezone(&chrono::Utc))
        .map_err(|e| StoreError::Corrupt(format!("push subscription time `{updated_at}`: {e}")))?;
    Ok(PushSubscriptionRow {
        device,
        updated_at,
        body: serde_json::from_str(&body)?,
    })
}

impl SqliteStore {
    pub fn push_subscription(&self, device: DeviceId) -> Result<Option<PushSubscriptionRow>> {
        let conn = self.read_conn();
        conn.query_row(
            "SELECT device, updated_at, body FROM push_subscriptions WHERE device = ?1",
            [device.to_string()],
            raw,
        )
        .optional()?
        .map(decode)
        .transpose()
    }

    /// Every subscription, in device id order.
    pub fn push_subscriptions(&self) -> Result<Vec<PushSubscriptionRow>> {
        let conn = self.read_conn();
        let mut statement = conn
            .prepare("SELECT device, updated_at, body FROM push_subscriptions ORDER BY device")?;
        let rows = statement.query_map([], raw)?;
        let mut subscriptions = Vec::new();
        for row in rows {
            subscriptions.push(decode(row?)?);
        }
        Ok(subscriptions)
    }

    /// Insert the device's subscription, or replace the one it has.
    pub fn put_push_subscription(&self, subscription: &PushSubscriptionRow) -> Result<()> {
        let body = serde_json::to_string(&subscription.body)?;
        let conn = self.conn.lock();
        let changed = conn.execute(
            "INSERT INTO push_subscriptions(device, updated_at, body) VALUES (?1, ?2, ?3)
             ON CONFLICT(device) DO UPDATE SET updated_at = excluded.updated_at, body = excluded.body",
            params![
                subscription.device.to_string(),
                subscription.updated_at.to_rfc3339(),
                body
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::Corrupt(
                "Push subscription was not saved exactly once; run `aether doctor`".into(),
            ));
        }
        Ok(())
    }

    /// Delete the device's subscription. Says whether there was one.
    pub fn remove_push_subscription(&self, device: DeviceId) -> Result<bool> {
        let conn = self.conn.lock();
        let removed = conn.execute(
            "DELETE FROM push_subscriptions WHERE device = ?1",
            [device.to_string()],
        )?;
        if removed > 1 {
            return Err(StoreError::Corrupt(
                "Duplicate push subscriptions were removed; run `aether doctor`".into(),
            ));
        }
        Ok(removed == 1)
    }

    /// Delete the device's subscription only while its body is still `body`.
    /// Says whether it was: one written since stays where it is.
    pub fn remove_push_subscription_if(
        &self,
        device: DeviceId,
        body: &serde_json::Value,
    ) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let stored: Option<String> = tx
            .query_row(
                "SELECT body FROM push_subscriptions WHERE device = ?1",
                [device.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(stored) = stored else {
            return Ok(false);
        };
        if serde_json::from_str::<serde_json::Value>(&stored)? != *body {
            return Ok(false);
        }
        let removed = tx.execute(
            "DELETE FROM push_subscriptions WHERE device = ?1",
            [device.to_string()],
        )?;
        if removed != 1 {
            return Err(StoreError::Corrupt(
                "Push subscription changed during removal; run `aether doctor`".into(),
            ));
        }
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::time::now;

    fn row(device: DeviceId, endpoint: &str) -> PushSubscriptionRow {
        PushSubscriptionRow {
            device,
            // SQLite keeps the text; compare at the precision RFC 3339 prints.
            updated_at: chrono::DateTime::parse_from_rfc3339(&now().to_rfc3339())
                .expect("a timestamp")
                .with_timezone(&chrono::Utc),
            body: serde_json::json!({"endpoint": endpoint, "kinds": ["approval"]}),
        }
    }

    #[test]
    fn a_device_has_one_subscription_that_is_replaced_and_removed() {
        let store = SqliteStore::open_in_memory().expect("a store");
        let (first, second) = (DeviceId::new(), DeviceId::new());
        assert_eq!(store.push_subscription(first).expect("reading"), None);
        assert_eq!(store.push_subscriptions().expect("listing"), []);

        let original = row(first, "https://push.example/a");
        store.put_push_subscription(&original).expect("writing");
        store
            .put_push_subscription(&row(second, "https://push.example/b"))
            .expect("writing");
        assert_eq!(
            store.push_subscription(first).expect("reading"),
            Some(original)
        );

        let replacement = row(first, "https://push.example/c");
        store
            .put_push_subscription(&replacement)
            .expect("replacing");
        let listed = store.push_subscriptions().expect("listing");
        assert_eq!(listed.len(), 2, "one row per device");
        assert_eq!(
            store.push_subscription(first).expect("reading"),
            Some(replacement)
        );

        assert!(store.remove_push_subscription(first).expect("removing"));
        assert!(
            !store
                .remove_push_subscription(first)
                .expect("removing again")
        );
        let left: Vec<DeviceId> = store
            .push_subscriptions()
            .expect("listing")
            .iter()
            .map(|subscription| subscription.device)
            .collect();
        assert_eq!(left, [second]);
    }

    #[test]
    fn a_subscription_is_removed_conditionally_only_while_it_is_the_one_named() {
        let store = SqliteStore::open_in_memory().expect("a store");
        let (device, other) = (DeviceId::new(), DeviceId::new());
        let original = row(device, "https://push.example/a");
        let unrelated = row(other, "https://push.example/a");
        assert!(
            !store
                .remove_push_subscription_if(device, &original.body)
                .expect("nothing stored"),
            "nothing to remove"
        );
        store.put_push_subscription(&original).expect("writing");
        store.put_push_subscription(&unrelated).expect("writing");

        let replacement = row(device, "https://push.example/b");
        store
            .put_push_subscription(&replacement)
            .expect("replacing");
        assert!(
            !store
                .remove_push_subscription_if(device, &original.body)
                .expect("comparing"),
            "the replaced body no longer names the row"
        );
        assert_eq!(
            store.push_subscription(device).expect("reading"),
            Some(replacement.clone())
        );

        assert!(
            store
                .remove_push_subscription_if(device, &replacement.body)
                .expect("removing")
        );
        assert_eq!(
            store.push_subscriptions().expect("listing"),
            [unrelated],
            "another device with the same body keeps its row"
        );
    }

    #[test]
    fn subscriptions_survive_a_reopen_and_are_visible_to_the_reader() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("aether.db");
        let device = DeviceId::new();
        let stored = row(device, "https://push.example/a");
        {
            let store = SqliteStore::open(&path).expect("a store");
            store.put_push_subscription(&stored).expect("writing");
            assert_eq!(
                store.push_subscription(device).expect("reading"),
                Some(stored.clone())
            );
        }
        let reopened = SqliteStore::open(&path).expect("reopening");
        assert_eq!(reopened.push_subscriptions().expect("listing"), [stored]);
    }

    #[test]
    fn a_row_that_does_not_parse_is_reported_as_corruption() {
        let store = SqliteStore::open_in_memory().expect("a store");
        store
            .conn
            .lock()
            .execute(
                "INSERT INTO push_subscriptions(device, updated_at, body) VALUES ('nobody', 'never', '{}')",
                [],
            )
            .expect("a raw row");
        assert!(matches!(
            store.push_subscriptions(),
            Err(StoreError::Corrupt(_))
        ));
    }
}
