//! What is waiting to be sent: bounded, coalesced per device and topic, held back per device while
//! its push service fails. Every method takes the time; nothing here sleeps or sends.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use aether_core::id::ApprovalId;
use aether_protocol::remote::DeviceId;
use tokio::time::Instant;

use super::notice::Notice;

/// The most slots the outbox holds, across all devices.
pub(crate) const CAPACITY: usize = 256;
/// The most notices one device may have waiting, so that a device whose push
/// service is down cannot crowd out the others.
pub(crate) const DEVICE_CAPACITY: usize = 32;
/// A new notice waits this long, so events of one commit collapse into one.
pub(crate) const SETTLE: Duration = Duration::from_millis(400);
/// After a send, the same topic stays quiet this long: a burst is one more
/// notification at the end, not one per event.
pub(crate) const QUIET: Duration = Duration::from_secs(5);
/// How long a notice is worth sending. Matches the `TTL` a push service gets.
pub(crate) const LIFETIME: Duration = Duration::from_secs(super::TTL_SECS as u64);
/// The first retry waits up to this; each failure doubles it.
pub(crate) const BACKOFF_BASE: Duration = Duration::from_secs(5);
/// No backoff step is longer than this.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);

/// The wait after `failures` consecutive failures. `jitter` in `[0, 1]`
/// picks a point in the upper half of the step, so retries spread out.
pub(crate) fn backoff(failures: u32, jitter: f64) -> Duration {
    assert!(failures >= 1, "a backoff follows a failure");
    assert!((0.0..=1.0).contains(&jitter), "jitter is a fraction");
    let doublings = (failures - 1).min(16);
    let step = BACKOFF_BASE.saturating_mul(1 << doublings).min(BACKOFF_MAX);
    step.mul_f64(0.5 + jitter / 2.0)
}

/// One notice on its way to one device.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub device: DeviceId,
    pub notice: Arc<Notice>,
    expires: Instant,
}

/// How a send ended, as far as the schedule cares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Settled {
    Delivered,
    /// Not delivered and not worth repeating.
    Dropped,
    /// The subscription is dead: everything for the device goes.
    Gone,
    /// The device replaced its subscription mid-send: send again, to the new
    /// one, with no failure counted against it.
    Resend,
    /// Try again later; `retry_after` is the service's own request.
    Retry {
        retry_after: Option<Duration>,
        jitter: f64,
    },
}

/// What happened to a notice that was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Offered {
    Queued,
    /// It took the place of an older notice on the same topic.
    Replaced,
    /// It was queued, and the oldest waiting notice was dropped to make room.
    Evicted,
    /// Every slot is busy sending; the notice was dropped.
    Full,
}

#[derive(Debug)]
struct Waiting {
    notice: Arc<Notice>,
    expires: Instant,
}

/// One device's notices on one topic.
#[derive(Debug)]
struct Slot {
    waiting: Option<Waiting>,
    /// Nothing leaves before this: the settle delay or the quiet window.
    not_before: Instant,
    in_flight: bool,
}

impl Slot {
    /// Put back a notice that was not delivered, to be sent again.
    fn requeue(&mut self, job: Job) {
        self.in_flight = false;
        // A newer notice may have taken the slot meanwhile; it stays.
        self.waiting.get_or_insert(Waiting {
            notice: job.notice,
            expires: job.expires,
        });
    }
}

/// How a device's push service has been answering.
#[derive(Debug)]
struct DeviceState {
    /// A send to this device is under way; one at a time.
    busy: bool,
    failures: u32,
    hold_until: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct Outbox {
    slots: HashMap<(DeviceId, String), Slot>,
    devices: HashMap<DeviceId, DeviceState>,
}

impl Outbox {
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Queue `notice` for `device`, replacing what waits on the same topic.
    pub(crate) fn offer(&mut self, device: DeviceId, notice: Arc<Notice>, now: Instant) -> Offered {
        let key = (device, notice.topic.clone());
        let waiting = Waiting {
            notice,
            expires: now + LIFETIME,
        };
        if let Some(slot) = self.slots.get_mut(&key) {
            let replaced = slot.waiting.replace(waiting).is_some();
            if !replaced && !slot.in_flight {
                slot.not_before = slot.not_before.max(now + SETTLE);
            }
            return if replaced {
                Offered::Replaced
            } else {
                Offered::Queued
            };
        }
        self.sweep(now);
        let evicted = match self.make_room(device) {
            Ok(evicted) => evicted,
            Err(Full) => return Offered::Full,
        };
        let slot = Slot {
            waiting: Some(waiting),
            not_before: now + SETTLE,
            in_flight: false,
        };
        self.slots.insert(key, slot);
        assert!(self.slots.len() <= CAPACITY, "the outbox stays bounded");
        if evicted {
            Offered::Evicted
        } else {
            Offered::Queued
        }
    }

    /// Free a slot for a new notice to `device` if a bound is reached.
    /// Says whether a waiting notice was dropped for it.
    fn make_room(&mut self, device: DeviceId) -> Result<bool, Full> {
        let of_device = self
            .slots
            .keys()
            .filter(|(owner, _)| *owner == device)
            .count();
        let scope = if of_device >= DEVICE_CAPACITY {
            Some(device)
        } else if self.slots.len() >= CAPACITY {
            None
        } else {
            return Ok(false);
        };
        let oldest = self
            .slots
            .iter()
            .filter(|((owner, _), slot)| !slot.in_flight && scope.is_none_or(|d| d == *owner))
            .min_by_key(|(_, slot)| slot.waiting.as_ref().map(|w| w.expires))
            .map(|(key, _)| key.clone());
        let key = oldest.ok_or(Full)?;
        let removed = self.slots.remove(&key);
        Ok(removed.is_some_and(|slot| slot.waiting.is_some()))
    }

    /// Drop what no longer matters: expired notices, finished quiet windows
    /// and the state of devices nothing is queued for.
    fn sweep(&mut self, now: Instant) {
        self.slots.retain(|_, slot| {
            if slot.waiting.as_ref().is_some_and(|w| w.expires <= now) {
                slot.waiting = None;
            }
            slot.in_flight || slot.waiting.is_some() || slot.not_before > now
        });
        let queued: HashSet<DeviceId> = self.slots.keys().map(|(owner, _)| *owner).collect();
        self.devices.retain(|device, state| {
            state.busy || state.hold_until > now || queued.contains(device)
        });
    }

    /// When a waiting notice of `device` may leave, if the device is free.
    fn ready_at(&self, device: DeviceId, slot: &Slot) -> Option<Instant> {
        if slot.in_flight || slot.waiting.is_none() {
            return None;
        }
        match self.devices.get(&device) {
            Some(state) if state.busy => None,
            Some(state) => Some(slot.not_before.max(state.hold_until)),
            None => Some(slot.not_before),
        }
    }

    /// The next notice that is due, marked as being sent.
    pub(crate) fn take_due(&mut self, now: Instant) -> Option<Job> {
        self.sweep(now);
        let key = self
            .slots
            .iter()
            .filter_map(|(key, slot)| Some((key, self.ready_at(key.0, slot)?)))
            .filter(|(_, at)| *at <= now)
            .min_by_key(|(_, at)| *at)
            .map(|(key, _)| key.clone())?;
        let slot = self.slots.get_mut(&key)?;
        let waiting = slot.waiting.take()?;
        slot.in_flight = true;
        let state = self.devices.entry(key.0).or_insert(DeviceState {
            busy: false,
            failures: 0,
            hold_until: now,
        });
        assert!(!state.busy, "one send per device at a time");
        state.busy = true;
        Some(Job {
            device: key.0,
            notice: waiting.notice,
            expires: waiting.expires,
        })
    }

    /// When the worker should look again, if anything is waiting.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter_map(|(key, slot)| self.ready_at(key.0, slot))
            .min()
    }

    /// Record how the send of `job` ended and schedule what follows.
    pub(crate) fn settle(&mut self, job: Job, outcome: Settled, now: Instant) {
        if outcome == Settled::Gone {
            self.forget(job.device);
            return;
        }
        // The device was forgotten or the outbox cleared while this was in flight.
        let Some(state) = self.devices.get_mut(&job.device) else {
            return;
        };
        state.busy = false;
        let key = (job.device, job.notice.topic.clone());
        match outcome {
            Settled::Delivered => {
                state.failures = 0;
                if let Some(slot) = self.slots.get_mut(&key) {
                    slot.in_flight = false;
                    slot.not_before = now + QUIET;
                }
            }
            Settled::Retry {
                retry_after,
                jitter,
            } => {
                state.failures = state.failures.saturating_add(1);
                let wait = backoff(state.failures, jitter).max(retry_after.unwrap_or_default());
                state.hold_until = now + wait.min(LIFETIME);
                if let Some(slot) = self.slots.get_mut(&key) {
                    slot.requeue(job);
                }
            }
            Settled::Resend => {
                if let Some(slot) = self.slots.get_mut(&key) {
                    slot.requeue(job);
                }
            }
            Settled::Dropped => {
                if let Some(slot) = self.slots.get_mut(&key) {
                    slot.in_flight = false;
                }
            }
            Settled::Gone => unreachable!("a dead subscription was handled above"),
        }
    }

    /// Drop a queued notification for an approval that was just decided.
    pub(crate) fn retract(&mut self, approval: ApprovalId) {
        for slot in self.slots.values_mut() {
            if slot
                .waiting
                .as_ref()
                .is_some_and(|w| w.notice.payload.approval == Some(approval))
            {
                slot.waiting = None;
            }
        }
    }

    /// Drop everything queued for `device` and what was known of its service.
    pub(crate) fn forget(&mut self, device: DeviceId) {
        self.slots.retain(|(owner, _), _| *owner != device);
        self.devices.remove(&device);
    }

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.devices.clear();
    }
}

/// No slot can be freed: all of them are being sent.
#[derive(Debug)]
struct Full;

#[cfg(test)]
mod tests;
