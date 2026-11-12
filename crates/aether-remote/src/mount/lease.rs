//! The publication-status lease: mounted traffic flows only while the last successful status
//! read showed no Funnel and is at most [`LEASE_AGE`] old (CONTRACTS.md §8).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::Publication;
use crate::RemoteHost;

/// How old a successful status read may be and still admit traffic.
pub const LEASE_AGE: Duration = Duration::from_secs(5);
/// How often the status is read again while a mounted connection is open.
pub const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// Longest one status read may take before it counts as failed.
const READ_LIMIT: Duration = LEASE_AGE;
/// Longest an admission waits for a status read of its own.
const ADMIT_WAIT: Duration = LEASE_AGE;
/// A refusal this recent answers admissions without another read, so a
/// stream of requests under a Funnel cannot become a stream of reads.
const BLOCKED_RETRY: Duration = Duration::from_secs(1);

/// Why mounted traffic is not flowing: the `mounts_blocked` of CONTRACTS.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// The listener is published through Tailscale Funnel.
    Funnel,
    /// The last status read failed, hung, or is too old.
    StatusUnknown,
}

impl Blocked {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Funnel => "funnel",
            Self::StatusUnknown => "status_unknown",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Funnel => {
                "mounted daemons are not served while this listener is published through \
                 Tailscale Funnel"
            }
            Self::StatusUnknown => {
                "mounted daemons are not served while the listener's publication status is \
                 unknown"
            }
        }
    }
}

#[derive(Debug)]
struct State {
    /// When the lease ends unless renewed; `None` while there is none.
    expires: Option<Instant>,
    /// Cancelled when the lease ends; every ticket of this lease holds it.
    epoch: CancellationToken,
    /// Why the last lease ended, and when.
    blocked: Option<(Blocked, Instant)>,
    /// Tickets alive: mounted connections and requests in flight.
    holders: usize,
    /// An admission is waiting for a read.
    wanted: bool,
    /// Status reads begun so far, and which of them renewed the lease: an admission while
    /// nothing is open needs a read begun after it asked.
    reads_begun: u64,
    renewed_by: u64,
}

impl State {
    fn live(&self, now: Instant) -> bool {
        !self.epoch.is_cancelled() && self.expires.is_some_and(|expires| now < expires)
    }
}

/// The lease every mounted connection is admitted under. One per listener.
#[derive(Debug)]
pub(crate) struct Lease {
    state: parking_lot::Mutex<State>,
    /// Wakes the guard: an admission wants a read, or a first holder arrived.
    wake: Notify,
    /// Bumped whenever a read settles or the lease ends.
    settled: watch::Sender<u64>,
}

impl Default for Lease {
    fn default() -> Self {
        Self {
            state: parking_lot::Mutex::new(State {
                expires: None,
                epoch: CancellationToken::new(),
                blocked: None,
                holders: 0,
                wanted: false,
                reads_begun: 0,
                renewed_by: 0,
            }),
            wake: Notify::new(),
            settled: watch::Sender::new(0),
        }
    }
}

/// One status read: when and in what order it began, and what it found.
struct Read {
    started: Instant,
    number: u64,
    found: Result<Publication, String>,
}

type Reading = Pin<Box<dyn Future<Output = Read> + Send>>;

impl Lease {
    /// Read the status on demand and every [`REFRESH_EVERY`] while anything
    /// holds a ticket, and end the lease the moment it expires, until `stop`.
    pub(crate) async fn guard(self: Arc<Self>, host: Arc<dyn RemoteHost>, stop: CancellationToken) {
        let mut reading: Option<Reading> = None;
        let mut last_read: Option<Instant> = None;
        loop {
            let (expires, refresh) = self.timers(last_read, reading.is_some());
            tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                _ = at(expires) => self.expire(),
                read = settle(&mut reading) => {
                    reading = None;
                    self.settle(read);
                }
                _ = at(refresh) => {
                    if !self.needs_read() {
                        continue;
                    }
                    let started = Instant::now();
                    last_read = Some(started);
                    let number = self.begin_read();
                    reading = Some(read_status(host.clone(), started, number));
                }
                _ = self.wake.notified() => {}
            }
        }
        self.end(Blocked::StatusUnknown);
    }

    /// When the lease expires, and when the next read is due.
    fn timers(
        &self,
        last_read: Option<Instant>,
        reading: bool,
    ) -> (Option<Instant>, Option<Instant>) {
        let state = self.state.lock();
        let refresh = match (reading, state.wanted, state.holders, last_read) {
            (true, _, _, _) => None,
            (false, true, _, _) | (false, false, 1.., None) => Some(Instant::now()),
            (false, false, 1.., Some(last)) => Some(last + REFRESH_EVERY),
            (false, false, 0, _) => None,
        };
        let expires = state.expires.filter(|_| !state.epoch.is_cancelled());
        (expires, refresh)
    }

    /// Whether a read is still wanted: the holder that scheduled it may have gone since.
    fn needs_read(&self) -> bool {
        let state = self.state.lock();
        state.wanted || state.holders > 0
    }

    /// An expired lease ends. Unheld, it merely lapsed: the next admission reads again at once.
    fn expire(&self) {
        let (due, held) = {
            let state = self.state.lock();
            let due = state
                .expires
                .is_some_and(|expires| expires <= Instant::now());
            (due, state.holders > 0)
        };
        if !due {
            return;
        }
        match held {
            true => self.end(Blocked::StatusUnknown),
            false => self.lapse(),
        }
    }

    fn lapse(&self) {
        {
            let mut state = self.state.lock();
            assert!(state.holders == 0, "a held lease ends with a verdict");
            state.expires = None;
            state.epoch.cancel();
        }
        self.settled.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Number the read about to begin; whoever wanted one is answered by it.
    fn begin_read(&self) -> u64 {
        let mut state = self.state.lock();
        state.wanted = false;
        state.reads_begun += 1;
        state.reads_begun
    }

    fn settle(&self, read: Read) {
        assert!(read.number > 0, "reads are numbered from one");
        match read.found {
            Ok(Publication::Private) => self.renew(read.started, read.number),
            Ok(Publication::Funnel) => self.end(Blocked::Funnel),
            Err(error) => {
                tracing::warn!(%error, "could not read the listener's publication status");
                self.end(Blocked::StatusUnknown);
            }
        }
    }

    /// A successful read taken at `started` keeps traffic flowing until it is too old.
    fn renew(&self, started: Instant, number: u64) {
        let expires = started + LEASE_AGE;
        {
            let mut state = self.state.lock();
            if expires <= Instant::now() {
                drop(state);
                return self.end(Blocked::StatusUnknown);
            }
            if state.epoch.is_cancelled() {
                state.epoch = CancellationToken::new();
            }
            assert!(
                state.expires.is_none_or(|old| old <= expires),
                "reads settle in order"
            );
            state.expires = Some(expires);
            assert!(number > state.renewed_by, "reads settle in order");
            state.renewed_by = number;
            state.blocked = None;
        }
        self.settled.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// End the lease now: every ticket's connection closes, nothing more is forwarded.
    pub(crate) fn end(&self, why: Blocked) {
        let was_live = {
            let mut state = self.state.lock();
            let was_live = state.expires.take().is_some();
            state.epoch.cancel();
            state.blocked = Some((why, Instant::now()));
            was_live
        };
        if was_live {
            tracing::warn!(
                blocked = why.as_str(),
                "mounted traffic stopped: {}",
                why.message()
            );
        }
        self.settled.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A ticket under a live lease. With nothing mounted open, only a status read begun
    /// after this call admits; with something open, the lease its refreshes keep does.
    pub(crate) async fn admit(self: &Arc<Self>) -> Result<Ticket, Blocked> {
        let deadline = Instant::now() + ADMIT_WAIT;
        let asked = self.state.lock().reads_begun;
        let mut settled = self.settled.subscribe();
        loop {
            settled.borrow_and_update();
            match self.try_admit(asked) {
                Ok(ticket) => return Ok(ticket),
                Err(Some(why)) => return Err(why),
                Err(None) => self.wake.notify_one(),
            }
            match tokio::time::timeout_at(deadline, settled.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => return Err(Blocked::StatusUnknown),
            }
        }
    }

    /// A ticket now; else the fresh reason to refuse, or `None` to wait for a read.
    /// `asked` is how many reads had begun when the admission was asked for.
    fn try_admit(self: &Arc<Self>, asked: u64) -> Result<Ticket, Option<Blocked>> {
        let now = Instant::now();
        let mut state = self.state.lock();
        debug_assert!(asked <= state.reads_begun, "reads are only ever counted up");
        let read_since = state.renewed_by > asked;
        if state.live(now) && (state.holders > 0 || read_since) {
            state.holders += 1;
            let first = state.holders == 1;
            let epoch = state.epoch.clone();
            drop(state);
            if first {
                self.wake.notify_one();
            }
            return Ok(Ticket {
                lease: self.clone(),
                epoch,
            });
        }
        if let Some((why, at)) = state.blocked.filter(|(_, at)| now < *at + BLOCKED_RETRY) {
            debug_assert!(at <= now, "a verdict is from the past");
            return Err(Some(why));
        }
        state.wanted = true;
        Err(None)
    }
}

/// One mounted connection's or request's place under the lease.
#[derive(Debug)]
pub(crate) struct Ticket {
    lease: Arc<Lease>,
    epoch: CancellationToken,
}

/// A ticket's standing without its place: what the sockets under it check on every byte.
#[derive(Debug, Clone)]
pub(crate) struct Standing {
    lease: Arc<Lease>,
    epoch: CancellationToken,
}

impl Standing {
    /// Whether a byte may be forwarded right now, even if the expiry timer has yet to fire.
    pub(crate) fn live(&self) -> bool {
        let state = self.lease.state.lock();
        !self.epoch.is_cancelled() && state.live(Instant::now())
    }
}

impl Ticket {
    /// Whether a byte may be forwarded right now. Checked before each one is.
    pub(crate) fn live(&self) -> bool {
        self.standing().live()
    }

    pub(crate) fn standing(&self) -> Standing {
        Standing {
            lease: self.lease.clone(),
            epoch: self.epoch.clone(),
        }
    }

    /// Fires when the lease this ticket was admitted under ends.
    pub(crate) fn ended(&self) -> tokio_util::sync::WaitForCancellationFuture<'_> {
        self.epoch.cancelled()
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.lease.state.lock();
        assert!(state.holders > 0, "every ticket was counted");
        state.holders -= 1;
    }
}

fn read_status(host: Arc<dyn RemoteHost>, started: Instant, number: u64) -> Reading {
    Box::pin(async move {
        let found = match tokio::time::timeout(READ_LIMIT, host.publication()).await {
            Ok(found) => found,
            Err(_) => Err(format!(
                "the status read took over {}s",
                READ_LIMIT.as_secs()
            )),
        };
        Read {
            started,
            number,
            found,
        }
    })
}

async fn settle(reading: &mut Option<Reading>) -> Read {
    match reading {
        Some(read) => read.await,
        None => std::future::pending().await,
    }
}

async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
