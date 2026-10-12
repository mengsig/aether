//! Where unauthenticated connections wait: small, never holding a device session, and never stuck
//! full, since a newcomer displaces the longest-seated (docs/remote.md, Connection limits).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Occupant {
    since: Instant,
    displaced: CancellationToken,
}

#[derive(Debug, Default)]
struct Seats {
    next: u64,
    /// By arrival: the first is whoever has waited longest.
    taken: BTreeMap<u64, Occupant>,
}

/// A bounded waiting room. Full, it seats a newcomer by displacing its
/// longest-seated occupant, once that one has had `patience`.
#[derive(Debug)]
pub(crate) struct Lobby {
    capacity: usize,
    patience: Duration,
    seats: parking_lot::Mutex<Seats>,
    /// Wakes whoever waits in [`Lobby::enter`] when a seat is given up.
    vacated: tokio::sync::Notify,
}

impl Lobby {
    pub(crate) fn new(capacity: usize, patience: Duration) -> Arc<Self> {
        assert!(capacity > 0, "a lobby seats at least one");
        Arc::new(Self {
            capacity,
            patience,
            seats: parking_lot::Mutex::new(Seats::default()),
            vacated: tokio::sync::Notify::new(),
        })
    }

    /// A seat now, or when the longest-seated occupant may be displaced.
    pub(crate) fn try_enter(self: &Arc<Self>) -> Result<Seat, Instant> {
        let mut seats = self.seats.lock();
        let full = seats.taken.len() >= self.capacity;
        // Seats are kept by arrival, so the first has waited longest.
        if let Some(oldest) = seats.taken.first_entry().filter(|_| full) {
            let due = oldest.get().since + self.patience;
            if Instant::now() < due {
                return Err(due);
            }
            oldest.remove().displaced.cancel();
        }
        let id = seats.next;
        seats.next += 1;
        let displaced = CancellationToken::new();
        let occupant = Occupant {
            since: Instant::now(),
            displaced: displaced.clone(),
        };
        seats.taken.insert(id, occupant);
        assert!(seats.taken.len() <= self.capacity, "the lobby is bounded");
        Ok(Seat {
            id,
            lobby: Arc::clone(self),
            displaced,
        })
    }

    /// A seat as soon as one is free or may be taken from its occupant.
    pub(crate) async fn enter(self: &Arc<Self>) -> Seat {
        loop {
            let due = match self.try_enter() {
                Ok(seat) => return seat,
                Err(due) => due,
            };
            tokio::select! {
                _ = self.vacated.notified() => {}
                _ = tokio::time::sleep_until(due) => {}
            }
        }
    }

    fn leave(&self, id: u64) {
        // Absent when the seat was taken from its occupant.
        if self.seats.lock().taken.remove(&id).is_some() {
            self.vacated.notify_one();
        }
    }
}

/// One place in a [`Lobby`], given up when dropped.
#[derive(Debug)]
pub(crate) struct Seat {
    id: u64,
    lobby: Arc<Lobby>,
    displaced: CancellationToken,
}

impl Seat {
    /// Fires when a newcomer was given this seat: the connection must end.
    pub(crate) fn displaced(&self) -> &CancellationToken {
        &self.displaced
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.lobby.leave(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATIENCE: Duration = Duration::from_secs(1);

    #[tokio::test(start_paused = true)]
    async fn a_full_lobby_seats_a_newcomer_in_the_place_held_longest() {
        let lobby = Lobby::new(2, PATIENCE);
        let first = lobby.try_enter().expect("room");
        tokio::time::advance(Duration::from_millis(400)).await;
        let second = lobby.try_enter().expect("room");
        let due = lobby
            .try_enter()
            .expect_err("full, and nobody has waited long");
        assert_eq!(due - Instant::now(), Duration::from_millis(600));

        tokio::time::advance(Duration::from_millis(600)).await;
        let third = lobby.try_enter().expect("the first has had its time");
        assert!(first.displaced().is_cancelled());
        assert!(!second.displaced().is_cancelled() && !third.displaced().is_cancelled());
        assert!(lobby.try_enter().is_err(), "the second is not due yet");
        // The displaced occupant leaving frees nothing: its seat is taken.
        drop(first);
        assert!(lobby.try_enter().is_err());
        drop(third);
        let fourth = lobby.try_enter().expect("a seat given up is free at once");
        assert!(!second.displaced().is_cancelled() && !fourth.displaced().is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn entering_waits_for_a_seat_given_up_or_grown_old() {
        let lobby = Lobby::new(1, PATIENCE);
        let held = lobby.try_enter().expect("room");
        let waiting = tokio::spawn({
            let lobby = lobby.clone();
            async move { lobby.enter().await }
        });
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "the lobby is full");
        drop(held);
        let seated = waiting.await.expect("joined");
        assert!(!seated.displaced().is_cancelled());

        let started = Instant::now();
        let newcomer = lobby.enter().await;
        assert_eq!(started.elapsed(), PATIENCE, "from when it was seated");
        assert!(seated.displaced().is_cancelled(), "its time was up");
        assert!(!newcomer.displaced().is_cancelled());
    }
}
