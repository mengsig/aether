//! Durable replay admission, committed before a webhook can dispatch work.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use super::signature::Stamp;

/// Accepted clock difference in either direction.
pub const REPLAY_WINDOW: Duration = Duration::from_secs(300);
/// Retain entries beyond the timestamp window for small clock corrections.
const CLOCK_SKEW: i64 = 30;
const MAX_REMEMBERED: i64 = 100_000;

/// Why a stamped delivery was not admitted. Each is a refusal the caller can
/// act on, so each says what to do differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Stale { by: i64 },
    Ahead { by: i64 },
    Replayed,
    Saturated,
    Unavailable,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let window = REPLAY_WINDOW.as_secs();
        match self {
            Refused::Stale { by } => write!(
                f,
                "the delivery is {by}s old and the window is {window}s; sign and send it fresh"
            ),
            Refused::Ahead { by } => write!(
                f,
                "the delivery is timestamped {by}s ahead of this clock and the window is \
                 {window}s; check the sender's clock"
            ),
            Refused::Replayed => write!(f, "this delivery was already accepted"),
            Refused::Saturated => write!(
                f,
                "too many deliveries are being remembered at once; retry with a fresh stamp"
            ),
            Refused::Unavailable => write!(f, "the replay guard is unavailable"),
        }
    }
}

/// Persistent, per-route delivery keys and signed nonces. Never evict a live key.
#[derive(Debug)]
pub struct ReplayGuard {
    connection: Mutex<Connection>,
}

impl ReplayGuard {
    /// Open the private replay database in the caller's durable state directory.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        prepare_file(path).context("preparing webhook replay state")?;
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .context("opening webhook replay state")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection
            .execute_batch(
                "PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS deliveries (
                 route TEXT NOT NULL,
                 delivery_key TEXT NOT NULL,
                 nonce TEXT NOT NULL,
                 expires INTEGER NOT NULL,
                 PRIMARY KEY (route, delivery_key),
                 UNIQUE (route, nonce)
             );
             CREATE INDEX IF NOT EXISTS deliveries_expiry ON deliveries(expires);",
            )
            .context("initializing webhook replay state")?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Commit admission before dispatch. A storage failure always refuses delivery.
    pub fn admit(
        &self,
        path: &str,
        stamp: &Stamp,
        signature: &str,
        delivery_id: Option<&str>,
        now: i64,
    ) -> Result<(), Refused> {
        validate_time(stamp.timestamp, now)?;
        let key = delivery_key(stamp, signature, delivery_id);
        let mut connection = self.connection.lock().map_err(|_| Refused::Unavailable)?;
        remember(&mut connection, path, stamp, &key, now, MAX_REMEMBERED)
            .map_err(|_| Refused::Unavailable)?
    }
}

fn prepare_file(path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .context("webhook replay path needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "webhook replay state must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn validate_time(timestamp: i64, now: i64) -> Result<(), Refused> {
    let difference = timestamp.abs_diff(now);
    if difference <= REPLAY_WINDOW.as_secs() {
        return Ok(());
    }
    let by = difference.min(i64::MAX as u64) as i64;
    if timestamp < now {
        return Err(Refused::Stale { by });
    }
    Err(Refused::Ahead { by })
}

fn delivery_key(stamp: &Stamp, signature: &str, delivery_id: Option<&str>) -> String {
    if let Some(id) = delivery_id {
        return format!("id:{}", hex::encode(Sha256::digest(id.as_bytes())));
    }
    let payload = format!(
        "{}.{}",
        stamp.timestamp,
        signature.trim().to_ascii_lowercase()
    );
    format!(
        "signature:{}",
        hex::encode(Sha256::digest(payload.as_bytes()))
    )
}

fn remember(
    connection: &mut Connection,
    path: &str,
    stamp: &Stamp,
    key: &str,
    now: i64,
    limit: i64,
) -> rusqlite::Result<Result<(), Refused>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute("DELETE FROM deliveries WHERE expires < ?1", [now])?;
    let replayed: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM deliveries
         WHERE route = ?1 AND (delivery_key = ?2 OR nonce = ?3))",
        params![path, key, stamp.nonce],
        |row| row.get(0),
    )?;
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM deliveries WHERE route = ?1",
        [path],
        |row| row.get(0),
    )?;
    let outcome = if replayed {
        Err(Refused::Replayed)
    } else if count >= limit {
        Err(Refused::Saturated)
    } else {
        // Arrival plus two windows also preserves nonce suppression for future-dated stamps.
        let expires = now.saturating_add(2 * REPLAY_WINDOW.as_secs() as i64 + CLOCK_SKEW);
        transaction.execute(
            "INSERT INTO deliveries (route, delivery_key, nonce, expires) VALUES (?1, ?2, ?3, ?4)",
            params![path, key, stamp.nonce, expires],
        )?;
        Ok(())
    };
    transaction.commit()?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
