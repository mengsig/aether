//! Proof that a submission with no receipt never ran: receipts are numbered,
//! and the store remembers the highest number it dropped (docs/remote.md).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// Longest epoch accepted in a horizon: a ULID, with room to change the id.
const MAX_EPOCH_BYTES: usize = 64;

/// A store's durable account of the receipts it has recorded and dropped,
/// written in the same transaction as the record or sweep it describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionLedger {
    /// The store this ledger was started for. A ledger that names another
    /// store came with a copied or replaced database and proves nothing here.
    pub store: String,
    /// Minted whenever the ledger is started, so a horizon handed out under
    /// an earlier ledger cannot be measured against this one.
    pub epoch: String,
    /// When the ledger was started.
    pub since: Timestamp,
    /// Ordinal of the newest receipt recorded.
    pub recorded: u64,
    /// Highest ordinal evicted or expired, or whose fate was already
    /// unknowable when the ledger started. Never decreases.
    pub forgotten: u64,
    /// `at` of the newest receipt dropped, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgotten_at: Option<Timestamp>,
}

impl SubmissionLedger {
    /// A new ledger for `store`. Nothing numbered at or below `floor` is
    /// provable: whatever happened to those receipts happened unrecorded.
    pub fn start(store: &str, floor: u64, at: Timestamp) -> Self {
        assert!(!store.is_empty(), "a ledger belongs to a named store");
        Self {
            store: store.to_owned(),
            epoch: ulid::Ulid::generate().to_string(),
            since: at,
            recorded: floor,
            forgotten: floor,
            forgotten_at: None,
        }
    }

    /// Number the receipt being recorded.
    pub fn next_ordinal(&mut self) -> u64 {
        assert!(self.forgotten <= self.recorded, "{self:?}");
        assert!(self.recorded < u64::MAX, "receipt ordinals are exhausted");
        self.recorded += 1;
        self.recorded
    }

    /// Note that the receipt numbered `ordinal`, stamped `at`, was dropped.
    pub fn forget(&mut self, ordinal: u64, at: Timestamp) {
        assert!(
            ordinal <= self.recorded,
            "dropped receipt {ordinal} was never recorded: {self:?}"
        );
        self.forgotten = self.forgotten.max(ordinal);
        self.forgotten_at = Some(self.forgotten_at.map_or(at, |newest| newest.max(at)));
    }

    /// What a client keeps when it first sends a key.
    pub fn horizon(&self) -> SubmissionHorizon {
        SubmissionHorizon {
            epoch: self.epoch.clone(),
            recorded: self.recorded,
        }
    }

    /// Every receipt stamped after this is still held. For people to read:
    /// the decision is [`Self::proves_absence`], which uses no clock.
    pub fn retained_since(&self) -> Timestamp {
        self.forgotten_at
            .map_or(self.since, |newest| newest.max(self.since))
    }

    /// Whether a key first sent under `horizon`, with no receipt now, never
    /// ran. Look for the receipt *before* reading the ledger, never after.
    pub fn proves_absence(
        &self,
        horizon: Option<&SubmissionHorizon>,
    ) -> Result<(), UnprovenAbsence> {
        debug_assert!(self.forgotten <= self.recorded, "{self:?}");
        let Some(horizon) = horizon else {
            return Err(UnprovenAbsence::NoHorizon);
        };
        if horizon.epoch != self.epoch {
            return Err(UnprovenAbsence::ForeignHorizon);
        }
        // The client was told of more receipts than this store has recorded:
        // the store was rolled back, and what it lost may include the key.
        if horizon.recorded > self.recorded {
            return Err(UnprovenAbsence::HorizonAhead);
        }
        if self.forgotten > horizon.recorded {
            return Err(UnprovenAbsence::Forgotten);
        }
        Ok(())
    }
}

/// Why an absent receipt does not prove its key was never executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnprovenAbsence {
    /// The request named no horizon, so nothing bounds when the key was sent.
    NoHorizon,
    /// The horizon was issued under another ledger: the store was replaced,
    /// restored from before the ledger existed, or its identity changed.
    ForeignHorizon,
    /// The horizon counts more receipts than the store has recorded: the
    /// store was rolled back to an earlier state.
    HorizonAhead,
    /// Receipts recorded after the horizon have been evicted or expired.
    Forgotten,
}

/// How many receipts a store had recorded, under which ledger, when a client
/// was last told. Opaque on the wire: `<epoch>:<recorded>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionHorizon {
    pub epoch: String,
    pub recorded: u64,
}

impl fmt::Display for SubmissionHorizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.recorded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("horizon is not one this daemon issued: expected `<epoch>:<count>` as given in a welcome")]
pub struct InvalidHorizon;

impl FromStr for SubmissionHorizon {
    type Err = InvalidHorizon;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (epoch, recorded) = text.rsplit_once(':').ok_or(InvalidHorizon)?;
        if epoch.is_empty() || epoch.len() > MAX_EPOCH_BYTES {
            return Err(InvalidHorizon);
        }
        if !epoch.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(InvalidHorizon);
        }
        // Digits only: `u64::from_str` alone would also accept a leading `+`.
        if recorded.is_empty() || !recorded.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidHorizon);
        }
        Ok(Self {
            epoch: epoch.to_owned(),
            recorded: recorded.parse().map_err(|_| InvalidHorizon)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::now;

    fn recorded(ledger: &mut SubmissionLedger, count: u64) {
        for _ in 0..count {
            ledger.next_ordinal();
        }
    }

    #[test]
    fn an_absent_key_sent_after_everything_forgotten_was_never_executed() {
        let mut ledger = SubmissionLedger::start("store-a", 0, now());
        recorded(&mut ledger, 5);
        let horizon = ledger.horizon();
        assert_eq!(ledger.proves_absence(Some(&horizon)), Ok(()));
        // Receipts recorded before the horizon may go without voiding it.
        ledger.forget(5, now());
        assert_eq!(ledger.proves_absence(Some(&horizon)), Ok(()));
        // One recorded after it going is exactly the case to refuse.
        recorded(&mut ledger, 1);
        ledger.forget(6, now());
        assert_eq!(
            ledger.proves_absence(Some(&horizon)),
            Err(UnprovenAbsence::Forgotten)
        );
        assert_eq!(ledger.proves_absence(Some(&ledger.horizon())), Ok(()));
    }

    #[test]
    fn a_horizon_from_elsewhere_or_from_the_future_proves_nothing() {
        let mut ledger = SubmissionLedger::start("store-a", 0, now());
        recorded(&mut ledger, 3);
        assert_eq!(ledger.proves_absence(None), Err(UnprovenAbsence::NoHorizon));
        let other = SubmissionLedger::start("store-a", 3, now());
        assert_ne!(other.epoch, ledger.epoch, "every start mints an epoch");
        assert_eq!(
            ledger.proves_absence(Some(&other.horizon())),
            Err(UnprovenAbsence::ForeignHorizon)
        );
        let ahead = SubmissionHorizon {
            epoch: ledger.epoch.clone(),
            recorded: 4,
        };
        assert_eq!(
            ledger.proves_absence(Some(&ahead)),
            Err(UnprovenAbsence::HorizonAhead)
        );
    }

    /// A ledger started over a table it did not number cannot vouch for it.
    #[test]
    fn a_started_ledger_vouches_for_nothing_at_or_below_its_floor() {
        let ledger = SubmissionLedger::start("store-a", 40, now());
        assert_eq!((ledger.recorded, ledger.forgotten), (40, 40));
        let before = SubmissionHorizon {
            epoch: ledger.epoch.clone(),
            recorded: 39,
        };
        assert_eq!(
            ledger.proves_absence(Some(&before)),
            Err(UnprovenAbsence::Forgotten)
        );
        assert_eq!(ledger.proves_absence(Some(&ledger.horizon())), Ok(()));
    }

    #[test]
    fn forgetting_only_ever_moves_forward() {
        let at = now();
        let mut ledger = SubmissionLedger::start("store-a", 0, at);
        recorded(&mut ledger, 9);
        ledger.forget(7, at + chrono::Duration::seconds(9));
        ledger.forget(2, at + chrono::Duration::seconds(1));
        assert_eq!(ledger.forgotten, 7);
        assert_eq!(ledger.retained_since(), at + chrono::Duration::seconds(9));
        let fresh = SubmissionLedger::start("store-a", 0, at);
        assert_eq!(
            fresh.retained_since(),
            at,
            "nothing dropped: since the start"
        );
    }

    #[test]
    fn a_horizon_round_trips_and_refuses_what_it_did_not_print() {
        let ledger = SubmissionLedger::start("store-a", 12, now());
        let horizon = ledger.horizon();
        assert_eq!(horizon.to_string().parse(), Ok(horizon));
        for bad in [
            "",
            "12",
            ":12",
            "epoch:",
            "epoch:+1",
            "epoch:-1",
            "epoch:1.0",
            "ep och:1",
            "epoch:99999999999999999999999",
        ] {
            assert_eq!(
                bad.parse::<SubmissionHorizon>(),
                Err(InvalidHorizon),
                "{bad:?}"
            );
        }
        assert!(
            format!("{}:1", "e".repeat(MAX_EPOCH_BYTES + 1))
                .parse::<SubmissionHorizon>()
                .is_err()
        );
    }
}
