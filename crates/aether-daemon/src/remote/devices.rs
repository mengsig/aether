//! Device records in the daemon's store, and the audit trail of every
//! attempt to authenticate as one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_protocol::remote::{DeviceId, DeviceRole};
use aether_remote::auth::{AuthError, DeviceRecord, DeviceStore, PUBLIC_KEY_BYTES};
use aether_runtime::Runtime;
use aether_store::{Commit, DeviceRow, Mutation};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Refused hellos recorded one by one in each window. A peer that can reach
/// the listener can fail as often as it likes; the event log must not grow with it.
const REFUSALS_AUDITED_PER_WINDOW: u32 = 5;
const REFUSAL_WINDOW: Duration = Duration::from_secs(600);

/// What a device row's body holds; the row's columns hold the rest.
#[derive(Serialize, Deserialize)]
struct Body {
    name: String,
    role: DeviceRole,
    /// The raw P-256 point, base64url without padding.
    public_key: String,
}

/// An audit event: a custom event with a `remote.` name.
pub(super) fn audit(by: Actor, name: &str, payload: serde_json::Value) -> EventDraft {
    debug_assert!(name.starts_with("remote."), "{name} is not a remote event");
    EventDraft::new(
        by,
        EventKind::Custom {
            name: name.to_owned(),
            payload,
        },
    )
}

/// The row that stores `record`.
pub(super) fn row(record: &DeviceRecord) -> Result<DeviceRow, String> {
    if record.public_key.len() != PUBLIC_KEY_BYTES {
        return Err(format!("device {} has no P-256 public key", record.id));
    }
    let body = Body {
        name: record.name.clone(),
        role: record.role,
        public_key: URL_SAFE_NO_PAD.encode(&record.public_key),
    };
    let body = serde_json::to_value(body).map_err(|e| format!("encoding a device: {e}"))?;
    Ok(DeviceRow {
        id: record.id,
        created_at: record.created_at,
        revoked_at: record.revoked_at,
        last_seen_at: record.last_seen_at,
        record: body,
    })
}

/// The record a stored row holds. A row this cannot read is corrupt.
pub(super) fn record(row: DeviceRow) -> Result<DeviceRecord, String> {
    let id = row.id;
    let body: Body = serde_json::from_value(row.record)
        .map_err(|e| format!("device {id} is stored in a form this daemon cannot read: {e}"))?;
    let public_key = URL_SAFE_NO_PAD
        .decode(&body.public_key)
        .ok()
        .filter(|key| key.len() == PUBLIC_KEY_BYTES)
        .ok_or_else(|| format!("device {id} is stored without a P-256 public key"))?;
    Ok(DeviceRecord {
        id,
        name: body.name,
        role: body.role,
        public_key,
        created_at: row.created_at,
        last_seen_at: row.last_seen_at,
        revoked_at: row.revoked_at,
    })
}

/// Which refused hellos reach the event log: the first few of each window.
/// The rest are counted, and the count rides on the next one recorded.
#[derive(Debug)]
struct RefusalLedger {
    window_started: Instant,
    /// Written to the log in this window.
    recorded: u32,
    /// Refused since the last one written, and not written themselves.
    unrecorded: u64,
}

impl RefusalLedger {
    fn new(now: Instant) -> Self {
        Self {
            window_started: now,
            recorded: 0,
            unrecorded: 0,
        }
    }

    /// Count a refusal at `now`. `Some(n)` says to record it, and that `n`
    /// refusals went unrecorded since the last one that was.
    fn note(&mut self, now: Instant) -> Option<u64> {
        if now.saturating_duration_since(self.window_started) >= REFUSAL_WINDOW {
            self.window_started = now;
            self.recorded = 0;
        }
        debug_assert!(self.recorded <= REFUSALS_AUDITED_PER_WINDOW);
        if self.recorded < REFUSALS_AUDITED_PER_WINDOW {
            self.recorded += 1;
            return Some(std::mem::take(&mut self.unrecorded));
        }
        if self.unrecorded == 0 {
            tracing::warn!(
                limit = REFUSALS_AUDITED_PER_WINDOW,
                window_secs = REFUSAL_WINDOW.as_secs(),
                "refused device hellos keep arriving; the rest of this window's are only counted"
            );
        }
        self.unrecorded = self.unrecorded.saturating_add(1);
        None
    }

    /// Put back a count whose event could not be written, with that event.
    fn carry(&mut self, unrecorded: u64) {
        self.unrecorded = self.unrecorded.saturating_add(unrecorded).saturating_add(1);
    }
}

/// The daemon's store, as the remote listener's authenticator reads it.
pub(super) struct RuntimeDevices {
    runtime: Arc<Runtime>,
    refusals: std::sync::Mutex<RefusalLedger>,
}

/// Manual: the runtime prints far more than a log line wants.
impl std::fmt::Debug for RuntimeDevices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDevices").finish_non_exhaustive()
    }
}

impl RuntimeDevices {
    pub(super) fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            refusals: std::sync::Mutex::new(RefusalLedger::new(Instant::now())),
        }
    }

    fn refusals(&self) -> std::sync::MutexGuard<'_, RefusalLedger> {
        match self.refusals.lock() {
            Ok(ledger) => ledger,
            // The ledger is plain counters; a panic elsewhere cannot tear it.
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn store_failure(error: impl std::fmt::Display) -> AuthError {
    AuthError::Store(error.to_string())
}

impl DeviceStore for RuntimeDevices {
    fn device(&self, id: DeviceId) -> Result<Option<DeviceRecord>, AuthError> {
        let row = self.runtime.store().device(id).map_err(store_failure)?;
        let found = row.map(record).transpose().map_err(AuthError::Store)?;
        debug_assert!(found.as_ref().is_none_or(|device| device.id == id));
        Ok(found)
    }

    fn devices(&self, include_revoked: bool) -> Result<Vec<DeviceRecord>, AuthError> {
        let rows = self
            .runtime
            .store()
            .devices(include_revoked)
            .map_err(store_failure)?;
        let devices: Vec<DeviceRecord> = rows
            .into_iter()
            .map(record)
            .collect::<Result<_, _>>()
            .map_err(AuthError::Store)?;
        debug_assert!(include_revoked || devices.iter().all(|d| d.revoked_at.is_none()));
        Ok(devices)
    }

    fn accepted(&self, device: &DeviceRecord, origin: &str) -> Result<(), AuthError> {
        debug_assert!(
            device.revoked_at.is_none(),
            "a revoked device is never accepted"
        );
        let seen = Mutation::TouchDevice {
            id: device.id,
            at: aether_core::time::now(),
        };
        let event = audit(
            Actor::System,
            "remote.device_authenticated",
            json!({
                "device": device.id,
                "name": device.name,
                "role": device.role,
                "origin": origin,
            }),
        );
        let commit = Commit::new().mutate(seen).event(event);
        // Unrecorded means refused: no session without its audit entry.
        self.runtime.apply(commit).map_err(store_failure)?;
        Ok(())
    }

    fn refused(&self, device: DeviceId, origin: &str, reason: &AuthError) {
        // Anyone who reaches the listener can cause this line: never louder.
        tracing::debug!(%device, origin, %reason, "refused a device");
        let Some(unrecorded) = self.refusals().note(Instant::now()) else {
            return;
        };
        let payload = json!({
            "device": device,
            "origin": origin,
            "reason": reason.to_string(),
            "unrecorded": unrecorded,
        });
        let event = audit(Actor::System, "remote.device_refused", payload);
        // The hello is refused either way; a store that cannot record it is
        // reported where the operator looks for a failing store.
        if let Err(error) = self.runtime.apply(Commit::new().event(event)) {
            tracing::error!(%device, %error, "could not record a refused device hello");
            self.refusals().carry(unrecorded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enrolled() -> DeviceRecord {
        DeviceRecord {
            id: DeviceId::new(),
            name: "Kim's phone".into(),
            role: DeviceRole::Observer,
            public_key: vec![4; PUBLIC_KEY_BYTES],
            created_at: aether_core::time::now(),
            last_seen_at: None,
            revoked_at: Some(aether_core::time::now()),
        }
    }

    #[test]
    fn a_flood_of_refusals_is_counted_and_only_a_few_are_recorded() {
        let start = Instant::now();
        let mut ledger = RefusalLedger::new(start);
        let flood = 10_000u64;
        let recorded: Vec<u64> = (0..flood).filter_map(|_| ledger.note(start)).collect();
        assert_eq!(recorded, [0; REFUSALS_AUDITED_PER_WINDOW as usize]);
        let late = start + REFUSAL_WINDOW - Duration::from_secs(1);
        assert_eq!(ledger.note(late), None, "the window has not ended");

        let counted = flood + 1 - u64::from(REFUSALS_AUDITED_PER_WINDOW);
        let next_window = start + REFUSAL_WINDOW;
        assert_eq!(ledger.note(next_window), Some(counted), "none is lost");
        assert_eq!(ledger.note(next_window), Some(0), "and none counted twice");
    }

    #[test]
    fn a_refusal_that_could_not_be_written_is_counted_instead() {
        let start = Instant::now();
        let mut ledger = RefusalLedger::new(start);
        assert_eq!(ledger.note(start), Some(0));
        ledger.carry(0);
        assert_eq!(ledger.note(start), Some(1));
        ledger.carry(1);
        assert_eq!(ledger.note(start), Some(2), "it and the one it carried");
    }

    #[test]
    fn a_record_survives_the_store() {
        let device = enrolled();
        let stored = row(&device).expect("a row");
        assert!(
            stored
                .record
                .get("public_key")
                .is_some_and(|key| key.is_string())
        );
        assert_eq!(record(stored), Ok(device));
    }

    #[test]
    fn a_row_without_a_whole_key_is_refused_both_ways() {
        let mut short = enrolled();
        short.public_key.truncate(PUBLIC_KEY_BYTES - 1);
        assert!(row(&short).is_err());
        let mut stored = row(&enrolled()).expect("a row");
        stored.record["public_key"] = "AAAA".into();
        let refused = record(stored).expect_err("four bytes are no key");
        assert!(refused.contains("public key"), "{refused}");
    }
}
